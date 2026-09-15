use std::collections::HashMap;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ffmpeg_next as ffmpeg;
use thiserror::Error;

use crate::capture_macos::CaptureFrame;

pub const DEFAULT_BITRATE: u32 = 8_000_000;
pub const MIN_LAN_BITRATE: u32 = 50_000_000;
pub const MAX_LAN_BITRATE: u32 = 150_000_000;
pub const ENCODE_QUEUE_DEPTH: usize = 3;
const OUTPUT_QUEUE_DEPTH: usize = 3;

fn output_channel() -> (
    OutputSender,
    mpsc::Receiver<Result<EncodedFrame, EncodeError>>,
) {
    let (sender, receiver) = mpsc::sync_channel(OUTPUT_QUEUE_DEPTH);
    (
        OutputSender {
            sender,
            stop: Arc::new(OutputStop::default()),
        },
        receiver,
    )
}

#[derive(Default)]
struct OutputStop {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl OutputStop {
    fn cancel(&self) {
        *self.stopped.lock().expect("output stop poisoned") = true;
        self.wake.notify_all();
    }

    fn is_stopped(&self) -> bool {
        *self.stopped.lock().expect("output stop poisoned")
    }
}

struct OutputSender {
    sender: SyncSender<Result<EncodedFrame, EncodeError>>,
    stop: Arc<OutputStop>,
}

impl OutputSender {
    fn send(
        &self,
        mut frame: Result<EncodedFrame, EncodeError>,
    ) -> Result<(), mpsc::SendError<Result<EncodedFrame, EncodeError>>> {
        let mut stopped = self.stop.stopped.lock().expect("output stop poisoned");
        while !*stopped {
            match self.sender.try_send(frame) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(frame)) => return Err(mpsc::SendError(frame)),
                Err(TrySendError::Full(pending)) => frame = pending,
            }
            // std's public receiver cannot notify us when space opens. Retain
            // this exact compressed frame until space opens or stop wakes us.
            stopped = self
                .stop
                .wake
                .wait_timeout(stopped, Duration::from_millis(5))
                .expect("output stop poisoned")
                .0;
        }
        Err(mpsc::SendError(frame))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub frames_per_second: u32,
    pub bitrate: u32,
    pub key_frame_interval: u32,
}

impl EncoderConfig {
    pub fn compatibility(width: u32, height: u32, frames_per_second: u32) -> Self {
        Self {
            width,
            height,
            frames_per_second,
            bitrate: DEFAULT_BITRATE,
            key_frame_interval: frames_per_second.max(1),
        }
    }

    pub fn lan(width: u32, height: u32, frames_per_second: u32, bitrate: u32) -> Self {
        Self {
            width,
            height,
            frames_per_second,
            bitrate: bitrate.clamp(MIN_LAN_BITRATE, MAX_LAN_BITRATE),
            key_frame_interval: frames_per_second.max(1),
        }
    }
}

#[derive(Debug)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub is_key_frame: bool,
    pub capture_at: Instant,
    pub encode_started_at: Instant,
    pub encode_completed_at: Instant,
}

#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("FFmpeg initialization failed: {0}")]
    Ffmpeg(#[from] ffmpeg::Error),
    #[error("hevc_videotoolbox encoder is unavailable")]
    EncoderUnavailable,
    #[error("invalid encoder dimensions or frame rate")]
    InvalidConfiguration,
    #[error("encoder worker stopped")]
    WorkerStopped,
    #[error("encoder queue is full; frame dropped")]
    QueueFull,
    #[error("captured frame dimensions or stride do not match the encoder")]
    FrameShape,
    #[error("encoder returned malformed HEVC data")]
    MalformedHevc,
}

pub(crate) enum Command {
    Frame(CaptureFrame),
    ForceKeyFrame,
    UpdateBitrate(u32),
    Stop,
}

pub struct VideoToolboxEncoder {
    pub(crate) sender: SyncSender<Command>,
    stop: Arc<OutputStop>,
    worker: Option<thread::JoinHandle<()>>,
}

impl VideoToolboxEncoder {
    pub fn start(
        mut config: EncoderConfig,
    ) -> Result<(Self, mpsc::Receiver<Result<EncodedFrame, EncodeError>>), EncodeError> {
        if config.frames_per_second == 0 {
            config.frames_per_second = 60;
        }
        if config.key_frame_interval == 0 {
            config.key_frame_interval = config.frames_per_second;
        }
        if config.width == 0 || config.height == 0 {
            return Err(EncodeError::InvalidConfiguration);
        }
        ffmpeg::init()?;
        if ffmpeg::encoder::find_by_name("hevc_videotoolbox").is_none() {
            return Err(EncodeError::EncoderUnavailable);
        }

        let (command_tx, command_rx) = mpsc::sync_channel(ENCODE_QUEUE_DEPTH);
        let (output_tx, output_rx) = output_channel();
        let stop = Arc::clone(&output_tx.stop);
        let worker = thread::Builder::new()
            .name("maho-host-videotoolbox".into())
            .spawn(move || {
                let result = EncoderWorker::new(config).and_then(|mut worker| {
                    while let Ok(command) = command_rx.recv() {
                        if output_tx.stop.is_stopped() {
                            return Ok(());
                        }
                        match command {
                            Command::Frame(frame) => match worker.encode(frame) {
                                Ok(frames) => {
                                    for frame in frames {
                                        if output_tx.send(Ok(frame)).is_err() {
                                            return Ok(());
                                        }
                                    }
                                }
                                Err(error) => {
                                    if output_tx.send(Err(error)).is_err() {
                                        return Ok(());
                                    }
                                }
                            },
                            Command::ForceKeyFrame => worker.force_key_frame = true,
                            Command::UpdateBitrate(bitrate) => worker.update_bitrate(bitrate),
                            Command::Stop => break,
                        }
                    }
                    worker.flush(&output_tx)
                });
                if let Err(error) = result {
                    let _ = output_tx.send(Err(error));
                }
            })
            .map_err(|_| EncodeError::WorkerStopped)?;
        Ok((
            Self {
                sender: command_tx,
                stop,
                worker: Some(worker),
            },
            output_rx,
        ))
    }

    pub fn submit(&self, frame: CaptureFrame) -> Result<(), EncodeError> {
        match self.sender.try_send(Command::Frame(frame)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(EncodeError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(EncodeError::WorkerStopped),
        }
    }

    pub fn force_key_frame(&self) -> Result<(), EncodeError> {
        self.sender
            .send(Command::ForceKeyFrame)
            .map_err(|_| EncodeError::WorkerStopped)
    }

    pub fn update_bitrate(&self, bitrate: u32) -> Result<(), EncodeError> {
        self.sender
            .send(Command::UpdateBitrate(bitrate))
            .map_err(|_| EncodeError::WorkerStopped)
    }

    /// Cancel pending output and join, even if the output receiver is full.
    /// Compressed output is never discarded while the encoder is active.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for VideoToolboxEncoder {
    fn drop(&mut self) {
        self.stop.cancel();
        // Wake an idle worker, but never wait for space in a full command queue.
        let _ = self.sender.try_send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            if let Err(error) = worker.join() {
                tracing::error!(?error, "VideoToolbox worker panicked");
            }
        }
    }
}

struct EncoderWorker {
    encoder: ffmpeg::encoder::Video,
    config: EncoderConfig,
    next_pts: i64,
    force_key_frame: bool,
    timing_by_pts: HashMap<i64, (Instant, Instant)>,
    parameter_sets: Vec<Vec<u8>>,
}

impl EncoderWorker {
    fn new(config: EncoderConfig) -> Result<Self, EncodeError> {
        let codec = ffmpeg::encoder::find_by_name("hevc_videotoolbox")
            .ok_or(EncodeError::EncoderUnavailable)?;
        let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()?;
        encoder.set_width(config.width);
        encoder.set_height(config.height);
        encoder.set_format(ffmpeg::format::Pixel::BGRA);
        encoder.set_color_range(ffmpeg::color::Range::JPEG);
        encoder.set_time_base((1, config.frames_per_second as i32));
        encoder.set_frame_rate(Some((config.frames_per_second as i32, 1)));
        encoder.set_gop(config.key_frame_interval);
        encoder.set_max_b_frames(0);
        encoder.set_bit_rate(config.bitrate as usize);
        encoder.set_max_bit_rate(config.bitrate as usize);
        encoder.set_tolerance(config.bitrate as usize);
        encoder.set_flags(ffmpeg::codec::Flags::LOW_DELAY);

        let mut options = ffmpeg::Dictionary::new();
        options.set("realtime", "1");
        options.set("allow_sw", "1");
        options.set("profile", "main");
        options.set("max_ref_frames", "1");
        options.set("prio_speed", "1");
        options.set("constant_bit_rate", "0");
        let encoder = encoder.open_as_with(codec, options)?;

        Ok(Self {
            encoder,
            config,
            next_pts: 0,
            force_key_frame: true,
            timing_by_pts: HashMap::new(),
            parameter_sets: Vec::new(),
        })
    }

    fn encode(&mut self, frame: CaptureFrame) -> Result<Vec<EncodedFrame>, EncodeError> {
        if frame.bytes_per_row < frame.width as usize * 4
            || frame.bgra.len() < frame.bytes_per_row * frame.height as usize
        {
            return Err(EncodeError::FrameShape);
        }
        if frame.width != self.config.width || frame.height != self.config.height {
            // Desktop resolution changed: rebuild the encoder instead of
            // failing the worker. The fresh encoder forces a keyframe so the
            // client receives parameter sets for the new size.
            *self = EncoderWorker::new(EncoderConfig {
                width: frame.width,
                height: frame.height,
                ..self.config
            })?;
        }
        let encode_started_at = Instant::now();
        let pts = self.next_pts;
        self.next_pts += 1;

        let mut av_frame = ffmpeg::frame::Video::new(
            ffmpeg::format::Pixel::BGRA,
            self.config.width,
            self.config.height,
        );
        av_frame.set_color_range(ffmpeg::color::Range::JPEG);
        let destination_stride = av_frame.stride(0);
        for row in 0..self.config.height as usize {
            let source_start = row * frame.bytes_per_row;
            let destination_start = row * destination_stride;
            let row_bytes = self.config.width as usize * 4;
            av_frame.data_mut(0)[destination_start..destination_start + row_bytes]
                .copy_from_slice(&frame.bgra[source_start..source_start + row_bytes]);
        }
        av_frame.set_pts(Some(pts));
        if self.force_key_frame {
            av_frame.set_kind(ffmpeg::picture::Type::I);
            self.force_key_frame = false;
        }
        self.timing_by_pts
            .insert(pts, (frame.captured_at, encode_started_at));
        match self.encoder.send_frame(&av_frame) {
            Ok(()) => {}
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {
                let mut output = self.receive_available()?;
                self.encoder.send_frame(&av_frame)?;
                output.extend(self.receive_available()?);
                return Ok(output);
            }
            Err(error) => return Err(EncodeError::Ffmpeg(error)),
        }
        self.receive_available()
    }

    fn receive_available(&mut self) -> Result<Vec<EncodedFrame>, EncodeError> {
        let mut output = Vec::new();
        loop {
            match self.receive_one() {
                Ok(frame) => output.push(frame),
                Err(EncodeError::Ffmpeg(ffmpeg::Error::Other { errno }))
                    if errno == ffmpeg::error::EAGAIN =>
                {
                    break
                }
                Err(EncodeError::Ffmpeg(ffmpeg::Error::Eof)) => break,
                Err(error) => return Err(error),
            }
        }
        Ok(output)
    }

    fn receive_one(&mut self) -> Result<EncodedFrame, EncodeError> {
        let mut packet = ffmpeg::Packet::empty();
        self.encoder.receive_packet(&mut packet)?;
        let packet_pts = packet.pts().unwrap_or(self.next_pts - 1);
        let (capture_at, encode_started_at) = self
            .timing_by_pts
            .remove(&packet_pts)
            .unwrap_or((Instant::now(), Instant::now()));
        let is_key_frame = packet.is_key();
        let raw = packet.data().ok_or(EncodeError::MalformedHevc)?;
        let data = normalize_hevc_packet(raw, is_key_frame, &mut self.parameter_sets)?;
        Ok(EncodedFrame {
            data,
            is_key_frame,
            capture_at,
            encode_started_at,
            encode_completed_at: Instant::now(),
        })
    }

    fn update_bitrate(&mut self, bitrate: u32) {
        let bitrate = bitrate.max(1_000_000);
        self.encoder.set_bit_rate(bitrate as usize);
        self.encoder.set_max_bit_rate(bitrate as usize);
        unsafe {
            let value = std::ffi::CString::new(bitrate.to_string()).expect("numeric bitrate");
            let key = c"b";
            let _ = ffmpeg::sys::av_opt_set(
                self.encoder.as_mut_ptr().cast(),
                key.as_ptr(),
                value.as_ptr(),
                0,
            );
        }
    }

    fn flush(&mut self, output: &OutputSender) -> Result<(), EncodeError> {
        self.encoder.send_eof()?;
        for frame in self.receive_available()? {
            if output.send(Ok(frame)).is_err() {
                break;
            }
        }
        Ok(())
    }
}

/// FFmpeg's VideoToolbox encoder may expose Annex-B or 4-byte length-prefixed
/// output depending on its build. The v3 wire always carries AVCC-style NALUs.
fn normalize_hevc_packet(
    packet: &[u8],
    is_key_frame: bool,
    parameter_sets: &mut Vec<Vec<u8>>,
) -> Result<Vec<u8>, EncodeError> {
    let nalus = split_nalus(packet)?;
    for nalu in &nalus {
        let kind = hevc_nalu_type(nalu).ok_or(EncodeError::MalformedHevc)?;
        if !matches!(kind, 32..=34) {
            continue;
        }
        // Replace the cached set of this type instead of appending. A re-issued
        // VPS/SPS/PPS supersedes the previous one; keeping both would grow the
        // cache for the session's lifetime and prepend conflicting parameter
        // sets with identical ids into every keyframe access unit.
        match parameter_sets
            .iter_mut()
            .find(|known| hevc_nalu_type(known) == Some(kind))
        {
            Some(stored) => {
                if stored.as_slice() != *nalu {
                    *stored = nalu.to_vec();
                }
            }
            None => parameter_sets.push(nalu.to_vec()),
        }
    }

    let mut output = Vec::with_capacity(packet.len() + 256);
    if is_key_frame {
        for parameter_set in parameter_sets
            .iter()
            .filter(|nalu| hevc_nalu_type(nalu).is_some_and(|kind| matches!(kind, 32..=34)))
        {
            append_length_prefixed(&mut output, parameter_set)?;
        }
    }
    for nalu in nalus {
        if is_key_frame && hevc_nalu_type(nalu).is_some_and(|kind| matches!(kind, 32..=34)) {
            continue;
        }
        append_length_prefixed(&mut output, nalu)?;
    }
    Ok(output)
}

fn split_nalus(packet: &[u8]) -> Result<Vec<&[u8]>, EncodeError> {
    if let Ok(nalus) = split_avcc(packet) {
        return Ok(nalus);
    }
    split_annex_b(packet)
}

fn split_avcc(packet: &[u8]) -> Result<Vec<&[u8]>, EncodeError> {
    let mut offset = 0;
    let mut nalus = Vec::new();
    while offset < packet.len() {
        if packet.len() - offset < 4 {
            return Err(EncodeError::MalformedHevc);
        }
        let length = u32::from_be_bytes(packet[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        if length < 2 || packet.len() - offset < length {
            return Err(EncodeError::MalformedHevc);
        }
        nalus.push(&packet[offset..offset + length]);
        offset += length;
    }
    if nalus.is_empty() {
        return Err(EncodeError::MalformedHevc);
    }
    Ok(nalus)
}

fn split_annex_b(packet: &[u8]) -> Result<Vec<&[u8]>, EncodeError> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 <= packet.len() {
        if packet[index..].starts_with(&[0, 0, 0, 1]) {
            starts.push((index, 4));
            index += 4;
        } else if packet[index..].starts_with(&[0, 0, 1]) {
            starts.push((index, 3));
            index += 3;
        } else {
            index += 1;
        }
    }
    if starts.is_empty() {
        return Err(EncodeError::MalformedHevc);
    }
    let mut nalus = Vec::new();
    for (position, (start, start_len)) in starts.iter().copied().enumerate() {
        let end = starts
            .get(position + 1)
            .map_or(packet.len(), |(next, _)| *next);
        let mut nalu_data = &packet[start + start_len..end];
        // Annex B start codes are frequently preceded by trailing_zero_8bits
        // or alignment padding; those zero bytes are not part of the NAL unit
        // and break decoder parameter-set parsing when copied into AVCC.
        while let Some((&0, rest)) = nalu_data.split_last() {
            nalu_data = rest;
        }
        if !nalu_data.is_empty() {
            nalus.push(nalu_data);
        }
    }
    if nalus.is_empty() {
        return Err(EncodeError::MalformedHevc);
    }
    Ok(nalus)
}

fn append_length_prefixed(output: &mut Vec<u8>, nalu: &[u8]) -> Result<(), EncodeError> {
    let length = u32::try_from(nalu.len()).map_err(|_| EncodeError::MalformedHevc)?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(nalu);
    Ok(())
}

fn hevc_nalu_type(nalu: &[u8]) -> Option<u8> {
    nalu.first().map(|byte| (byte >> 1) & 0x3f)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keep the same capacity assertion runnable against both the unbounded
    // baseline and bounded constructor; this adapter is test-only.
    trait Probe {
        fn probe(&self, frame: EncodedFrame) -> bool;
    }
    impl Probe for mpsc::Sender<Result<EncodedFrame, EncodeError>> {
        fn probe(&self, frame: EncodedFrame) -> bool {
            self.send(Ok(frame)).is_ok()
        }
    }
    impl Probe for SyncSender<Result<EncodedFrame, EncodeError>> {
        fn probe(&self, frame: EncodedFrame) -> bool {
            self.try_send(Ok(frame)).is_ok()
        }
    }
    impl Probe for OutputSender {
        fn probe(&self, frame: EncodedFrame) -> bool {
            self.sender.try_send(Ok(frame)).is_ok()
        }
    }
    fn frame(index: u8) -> EncodedFrame {
        let now = Instant::now();
        EncodedFrame {
            data: vec![index],
            is_key_frame: index == 0,
            capture_at: now,
            encode_started_at: now,
            encode_completed_at: now,
        }
    }

    #[test]
    fn output_queue_bounds_retention_without_dropping_references() {
        let (sender, receiver) = output_channel();
        for index in 0..OUTPUT_QUEUE_DEPTH {
            assert!(sender.probe(frame(index as u8)));
        }
        assert!(!sender.probe(frame(99)));
        let retained: Vec<_> = receiver
            .try_iter()
            .map(|result| result.unwrap().data[0])
            .collect();
        assert_eq!(retained, vec![0, 1, 2]);
    }

    #[test]
    fn dropping_output_receiver_releases_publisher() {
        let (sender, receiver) = output_channel();
        for index in 0..OUTPUT_QUEUE_DEPTH {
            assert!(sender.probe(frame(index as u8)));
        }
        assert!(!sender.probe(frame(99)));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let disconnected = sender.send(Ok(frame(3))).is_err();
            exit_tx.send(disconnected).unwrap();
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        drop(receiver);
        assert!(exit_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn encoder_stop_joins_worker_after_output_receiver_drops() {
        let (output_tx, output_rx) = output_channel();
        let stop = Arc::clone(&output_tx.stop);
        for index in 0..OUTPUT_QUEUE_DEPTH {
            assert!(output_tx.probe(frame(index as u8)));
        }
        assert!(!output_tx.probe(frame(99)));
        let (sender, commands) = mpsc::sync_channel(ENCODE_QUEUE_DEPTH);
        for _ in 0..ENCODE_QUEUE_DEPTH {
            sender.send(Command::ForceKeyFrame).unwrap();
        }
        let (ready_tx, ready_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            assert!(output_tx.send(Ok(frame(3))).is_err());
            drop(commands);
            exit_tx.send(()).unwrap();
        });
        let encoder = VideoToolboxEncoder {
            sender,
            stop,
            worker: Some(worker),
        };
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        drop(output_rx);
        let (done_tx, done_rx) = mpsc::channel();
        let stopper = thread::spawn(move || {
            encoder.stop();
            done_tx.send(()).unwrap();
        });
        exit_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        stopper.join().unwrap();
    }

    fn nalu(kind: u8, payload: u8) -> Vec<u8> {
        vec![kind << 1, 1, payload]
    }

    #[test]
    fn encoder_stop_joins_with_full_output_and_retained_receiver() {
        // Given: the real public queue is full and the consumer is gated.
        let (output_tx, output_rx) = output_channel();
        let stop = Arc::clone(&output_tx.stop);
        for index in 0..OUTPUT_QUEUE_DEPTH {
            assert!(output_tx.probe(frame(index as u8)));
        }
        assert!(!output_tx.probe(frame(99)));
        let (sender, commands) = mpsc::sync_channel(ENCODE_QUEUE_DEPTH);
        for _ in 0..ENCODE_QUEUE_DEPTH {
            sender.send(Command::ForceKeyFrame).unwrap();
        }
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let _ = output_tx.send(Ok(frame(3)));
            drop(commands);
        });
        let encoder = VideoToolboxEncoder {
            sender,
            stop,
            worker: Some(worker),
        };
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        // When: stop is requested without dropping or draining the receiver.
        let stopper = thread::spawn(move || {
            encoder.stop();
            done_tx.send(()).unwrap();
        });
        let stopped = done_rx.recv_timeout(std::time::Duration::from_secs(2));
        // Release even on RED so no test worker is left blocked.
        drop(output_rx);
        stopper.join().unwrap();
        // Then: stop joined while the full receiver was still retained.
        assert!(
            stopped.is_ok(),
            "encoder stop waited for the output consumer: {stopped:?}"
        );
    }

    #[test]
    fn annex_b_trailing_zero_padding_is_trimmed() {
        // Given Annex B NALUs separated by trailing_zero_8bits padding.
        let packet = [0, 0, 0, 1, 0x40, 1, 2, 0, 0, 0, 0, 1, 0x26, 3];
        // When splitting, the padding must not join the preceding NAL unit.
        let nalus = split_annex_b(&packet).unwrap();
        assert_eq!(nalus, vec![&[0x40u8, 1, 2][..], &[0x26u8, 3][..]]);
    }

    #[test]
    fn nalu_splitting_borrows_packet_storage() {
        // Given: an AVCC P frame with no parameter-set ownership requirement.
        let packet = [0, 0, 0, 3, 2, 1, 99];
        // When: splitting for wire normalization.
        let nalus = split_avcc(&packet).unwrap();
        // Then: do not allocate and copy each compressed payload.
        assert_eq!(nalus[0].as_ptr(), packet[4..].as_ptr());
    }

    #[test]
    fn annex_b_is_converted_to_avcc_and_parameter_sets_prepend_keyframes() {
        let vps = nalu(32, 1);
        let sps = nalu(33, 2);
        let pps = nalu(34, 3);
        let idr = nalu(19, 4);
        let mut annex_b = Vec::new();
        for unit in [&vps, &sps, &pps, &idr] {
            annex_b.extend_from_slice(&[0, 0, 0, 1]);
            annex_b.extend_from_slice(unit);
        }
        let mut parameters = Vec::new();
        let encoded = normalize_hevc_packet(&annex_b, true, &mut parameters).unwrap();
        assert_eq!(split_avcc(&encoded).unwrap(), vec![vps, sps, pps, idr]);
    }

    #[test]
    fn reissued_parameter_sets_replace_the_cached_set() {
        // Given a keyframe that established VPS/SPS/PPS.
        let vps = nalu(32, 1);
        let sps = nalu(33, 2);
        let pps = nalu(34, 3);
        let idr = nalu(19, 4);
        let mut annex_b = Vec::new();
        for unit in [&vps, &sps, &pps, &idr] {
            annex_b.extend_from_slice(&[0, 0, 0, 1]);
            annex_b.extend_from_slice(unit);
        }
        let mut parameters = Vec::new();
        normalize_hevc_packet(&annex_b, true, &mut parameters).unwrap();

        // When the encoder re-issues an updated SPS and PPS on a later keyframe.
        let new_sps = nalu(33, 7);
        let new_pps = nalu(34, 8);
        let mut updated = Vec::new();
        for unit in [&new_sps, &new_pps, &idr] {
            updated.extend_from_slice(&[0, 0, 0, 1]);
            updated.extend_from_slice(unit);
        }
        let encoded = normalize_hevc_packet(&updated, true, &mut parameters).unwrap();

        // Then the cache holds one set per type and the keyframe carries no
        // stale, conflicting parameter sets.
        assert_eq!(
            parameters,
            vec![vps.clone(), new_sps.clone(), new_pps.clone()]
        );
        assert_eq!(
            split_avcc(&encoded).unwrap(),
            vec![vps, new_sps, new_pps, idr]
        );
    }

    #[test]
    fn lan_bitrate_is_clamped_to_requested_range() {
        assert_eq!(EncoderConfig::lan(1, 1, 60, 1).bitrate, MIN_LAN_BITRATE);
        assert_eq!(
            EncoderConfig::lan(1, 1, 60, u32::MAX).bitrate,
            MAX_LAN_BITRATE
        );
        assert_eq!(
            EncoderConfig::compatibility(1, 1, 60).bitrate,
            DEFAULT_BITRATE
        );
    }
}
