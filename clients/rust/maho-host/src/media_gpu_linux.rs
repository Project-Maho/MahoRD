#[path = "capture_linux_dmabuf.rs"]
pub mod capture_linux_dmabuf;
#[path = "vaapi_linux.rs"]
pub mod vaapi_linux;

use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::mpsc::{self, SyncSender},
    sync::Arc,
    time::{Duration, Instant},
};

use ffmpeg_next as ffmpeg;

use self::capture_linux_dmabuf::{DmabufCapturedFrame, LinuxDmabufCapture};
use self::vaapi_linux::{LibVa, VaapiPipeline};
use super::*;
use crate::encode_linux::{
    EncoderConfig as LinuxEncoderConfig, LinuxVideoEncoder, VideoCodec as LinuxVideoCodec,
};

pub struct GpuMediaSource {
    cpu: LinuxMediaSource,
}

impl GpuMediaSource {
    pub fn new(cpu: LinuxMediaSource) -> Self {
        Self { cpu }
    }
}

impl MediaSource for GpuMediaSource {
    fn start(&self, sender: SyncSender<MediaEvent>) -> Result<Box<dyn MediaHandle>, SessionError> {
        match try_start_gpu(&self.cpu, sender.clone()) {
            Ok(handle) => {
                crate::host_log("media pipeline: gpu dmabuf+vaapi-vpp hevc");
                Ok(handle)
            }
            Err(reason) => {
                crate::host_log(&format!("media pipeline: cpu fallback: {reason}"));
                self.cpu.start(sender)
            }
        }
    }
}

pub(super) fn gpu_source(cpu: LinuxMediaSource) -> Arc<dyn MediaSource> {
    Arc::new(GpuMediaSource::new(cpu))
}

struct GpuMediaHandle {
    workers: native_pipeline::Workers<DmabufCapturedFrame>,
}

impl MediaHandle for GpuMediaHandle {
    fn force_key_frame(&self) -> Result<(), SessionError> {
        self.workers
            .handoff
            .control(true, None)
            .then_some(())
            .ok_or(SessionError::MediaStopped)
    }

    fn update_bitrate(&self, bitrate: u32) -> Result<(), SessionError> {
        self.workers
            .handoff
            .control(false, Some(bitrate))
            .then_some(())
            .ok_or(SessionError::MediaStopped)
    }

    fn stop(&mut self) {
        self.workers.stop();
    }
}

pub struct SlotDescriptor {
    pub fd: OwnedFd,
    pub stride: u32,
    pub offset: u32,
    pub format: u32,
}

struct LinuxSessionGpuEncoder {
    encoder: LinuxVideoEncoder,
    vpp: VaapiPipeline,
    frames_buf: *mut ffmpeg::ffi::AVBufferRef,
    slot_descs: Vec<SlotDescriptor>,
    imported_slots: Vec<u32>,
    release_tx: mpsc::Sender<usize>,
    times: native_pipeline::FrameTimes,
}

impl LinuxSessionGpuEncoder {
    fn output(
        &mut self,
        frames: Vec<crate::encode_linux::EncodedFrame>,
    ) -> Result<Vec<VideoFrame>, String> {
        let encode_completed_at = Instant::now();
        frames
            .into_iter()
            .map(|encoded| {
                let (capture_at, encode_started_at) =
                    self.times.take(encoded.pts).ok_or_else(|| {
                        format!("encoder output has unknown input PTS {}", encoded.pts)
                    })?;
                Ok(VideoFrame {
                    data: encoded.data,
                    is_key_frame: encoded.is_key_frame,
                    capture_at,
                    encode_started_at,
                    encode_completed_at,
                })
            })
            .collect()
    }
}

impl native_pipeline::Encoder<DmabufCapturedFrame> for LinuxSessionGpuEncoder {
    type Output = VideoFrame;
    type Error = String;

    fn force_keyframe(&mut self) {
        self.encoder.force_key_frame();
    }

    fn bitrate(&mut self, bitrate: u32) -> Result<Vec<VideoFrame>, String> {
        let frames = self
            .encoder
            .update_bitrate(bitrate as usize)
            .map_err(|error| format!("encoder bitrate: {error}"))?;
        self.output(frames)
    }

    fn encode(&mut self, frame: DmabufCapturedFrame) -> Result<Vec<VideoFrame>, String> {
        let ring_idx = frame.ring_index;
        if ring_idx >= self.slot_descs.len() {
            let _ = self.release_tx.send(ring_idx);
            return Err(format!("ring slot index {} out of range", ring_idx));
        }

        if self.imported_slots[ring_idx] == vaapi_linux::VA_INVALID_ID {
            let desc = &self.slot_descs[ring_idx];
            let surf = self
                .vpp
                .import_drm_bo(desc.fd.as_raw_fd(), desc.stride, desc.offset, desc.format)
                .map_err(|e| {
                    let _ = self.release_tx.send(ring_idx);
                    format!("import drm bo: {e}")
                })?;
            self.imported_slots[ring_idx] = surf;
        }

        let mut dst_frame = ffmpeg::frame::Video::empty();
        // SAFETY: frames_buf is the valid AVBufferRef allocated for hw_frames_ctx, and dst_frame is a valid empty frame.
        let status = unsafe {
            ffmpeg::ffi::av_hwframe_get_buffer(self.frames_buf, dst_frame.as_mut_ptr(), 0)
        };
        if status < 0 {
            let _ = self.release_tx.send(ring_idx);
            return Err(format!(
                "av_hwframe_get_buffer failed: {}",
                ffmpeg::Error::from(status)
            ));
        }

        // SAFETY: dst_frame is a successfully allocated hardware frame; data[3] holds the VASurfaceID integer.
        let dst_surface = unsafe { (*dst_frame.as_ptr()).data[3] as usize as u32 };
        if let Err(e) = self
            .vpp
            .process_vpp(self.imported_slots[ring_idx], dst_surface)
        {
            let _ = self.release_tx.send(ring_idx);
            return Err(format!("process_vpp failed: {e}"));
        }

        let _ = self.release_tx.send(ring_idx);

        self.times.submitted(frame.captured_at, Instant::now());
        let encoded = self
            .encoder
            .encode_vaapi_frame(dst_frame)
            .map_err(|e| format!("encode_vaapi_frame: {e}"))?;

        self.output(encoded)
    }
}

fn try_start_gpu(
    source: &LinuxMediaSource,
    sender: SyncSender<MediaEvent>,
) -> Result<Box<dyn MediaHandle>, String> {
    let effective = reprobe_output_name(source.output_name.as_deref());
    let fps = source.fps.max(1);

    let (capture_ready_tx, capture_ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    let (encode_ready_tx, encode_ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    let (release_tx, release_rx) = mpsc::channel::<usize>();
    let (slot_desc_tx, slot_desc_rx) = mpsc::sync_channel::<Vec<SlotDescriptor>>(1);

    let mut workers = native_pipeline::Workers::<DmabufCapturedFrame>::new();

    {
        let sender = sender.clone();
        let effective_name = effective.clone();
        workers
            .spawn("maho-linux-dmabuf-capture", true, move |handoff| {
                let interval = Duration::from_micros(1_000_000 / u64::from(fps));
                let mut capture = match LinuxDmabufCapture::connect(
                    effective_name.as_deref(),
                    handoff.cancellation(),
                    Instant::now() + Duration::from_secs(5),
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = capture_ready_tx.send(Err(format!("dmabuf capture init: {e}")));
                        return;
                    }
                };

                let mut descs = Vec::with_capacity(capture.ring.len());
                for slot in &capture.ring {
                    // SAFETY: slot.gbm.fd is a valid open file descriptor owned by GBM bo.
                    let dup_raw = unsafe { libc::dup(slot.gbm.fd) };
                    if dup_raw < 0 {
                        let err = std::io::Error::last_os_error();
                        let _ = capture_ready_tx.send(Err(format!("dup gbm bo fd: {err}")));
                        return;
                    }
                    // SAFETY: dup_raw is a freshly duplicated, valid file descriptor exclusively owned here.
                    let fd = unsafe { OwnedFd::from_raw_fd(dup_raw) };
                    descs.push(SlotDescriptor {
                        fd,
                        stride: slot.gbm.stride,
                        offset: slot.gbm.offset,
                        format: capture.offer.format,
                    });
                }
                let _ = slot_desc_tx.send(descs);
                let _ = capture_ready_tx.send(Ok(()));

                while !handoff.is_stopped() {
                    while let Ok(released_slot) = release_rx.try_recv() {
                        capture.release_slot(released_slot);
                    }

                    let started = Instant::now();
                    let slot = loop {
                        if handoff.is_stopped() {
                            return;
                        }
                        if let Some(slot) = capture.next_free_slot() {
                            break slot;
                        }
                        match release_rx.recv_timeout(Duration::from_millis(5)) {
                            Ok(released_slot) => {
                                capture.release_slot(released_slot);
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                handoff.stop();
                                return;
                            }
                        }
                    };

                    match capture.capture_frame(
                        slot,
                        handoff.cancellation(),
                        Duration::from_millis(50),
                    ) {
                        Ok(frame) => {
                            if !handoff.publish(frame) {
                                break;
                            }
                            handoff.pace_until(started + interval);
                        }
                        Err(e) => {
                            capture.release_slot(slot);
                            if !handoff.is_stopped() {
                                native_media_error(&handoff, &sender, format!("capture: {e}"));
                            }
                            return;
                        }
                    }
                }
            })
            .map_err(|e| format!("spawn capture: {e}"))?;
    }

    let config = LinuxEncoderConfig {
        width: source.width,
        height: source.height,
        bitrate: source.bitrate as usize,
        fps,
        keyframe_interval: fps,
        preferred_codec: LinuxVideoCodec::Hevc,
    };

    {
        let sender = sender.clone();
        workers
            .spawn("maho-linux-dmabuf-encode", true, move |handoff| {
                let init_res = (|| -> Result<LinuxSessionGpuEncoder, String> {
                    let libva = LibVa::load().map_err(|e| format!("libva load: {e}"))?;
                    let encoder =
                        LinuxVideoEncoder::new(config).map_err(|e| format!("encoder init: {e}"))?;
                    let (device_buf, frames_buf) = encoder
                        .vaapi_hwctx()
                        .ok_or_else(|| "encoder missing vaapi hwctx".to_string())?;

                    let vpp = VaapiPipeline::new(libva, device_buf, config.width, config.height)
                        .map_err(|e| format!("vpp init: {e}"))?;

                    let slot_descs = slot_desc_rx
                        .recv_timeout(Duration::from_secs(5))
                        .map_err(|_| "timeout waiting for slot descriptors".to_string())?;

                    let slot_count = slot_descs.len();
                    Ok(LinuxSessionGpuEncoder {
                        encoder,
                        vpp,
                        frames_buf,
                        slot_descs,
                        imported_slots: vec![vaapi_linux::VA_INVALID_ID; slot_count],
                        release_tx,
                        times: native_pipeline::FrameTimes::new(),
                    })
                })();

                let session_encoder = match init_res {
                    Ok(enc) => {
                        let _ = encode_ready_tx.send(Ok(()));
                        enc
                    }
                    Err(e) => {
                        let _ = encode_ready_tx.send(Err(e));
                        return;
                    }
                };

                let result = native_pipeline::run_encoder(
                    &handoff,
                    || Ok(session_encoder),
                    |frame| sender.send(MediaEvent::Video(frame)).is_ok(),
                );
                if let Err(error) = result {
                    native_media_error(&handoff, &sender, error);
                }
            })
            .map_err(|e| format!("spawn encode: {e}"))?;
    }

    if source.capture_audio {
        let sender = sender.clone();
        workers
            .spawn("maho-linux-dmabuf-audio", false, move |handoff| {
                use crate::audio_linux::LinuxAudioCapture;
                let mut capture = match LinuxAudioCapture::open_cancellable(handoff.cancellation())
                {
                    Ok(capture) => capture,
                    Err(error) => {
                        warn!(%error, "Audio capture unavailable");
                        return;
                    }
                };
                let slot = Arc::new(native_pipeline::LatestAudio::default());
                let mut samples = [0.0_f32; 1920];
                while !handoff.is_stopped() {
                    match capture.read_interleaved_f32_cancellable(
                        &mut samples,
                        handoff.cancellation(),
                        Duration::from_millis(50),
                    ) {
                        Ok(0) => {}
                        Ok(count) => {
                            let captured_at = Instant::now();
                            let mut pcm = Vec::with_capacity(count * 4);
                            for sample in &samples[..count] {
                                pcm.extend_from_slice(&sample.to_le_bytes());
                            }
                            if slot.publish(native_pipeline::AudioBlock { pcm, captured_at }) {
                                match sender.try_send(MediaEvent::NativeAudio(Arc::clone(&slot))) {
                                    Ok(()) => {}
                                    Err(mpsc::TrySendError::Full(_)) => {
                                        slot.notification_rejected();
                                    }
                                    Err(mpsc::TrySendError::Disconnected(_)) => {
                                        handoff.stop();
                                        break;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            if !handoff.is_stopped() {
                                warn!(%error, "Audio capture ended");
                            }
                            break;
                        }
                    }
                }
            })
            .map_err(|e| format!("spawn audio: {e}"))?;
    }

    workers.handoff.activate();

    match capture_ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            workers.stop();
            return Err(e);
        }
        Err(_) => {
            workers.stop();
            return Err("capture init timeout".into());
        }
    }

    match encode_ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            workers.stop();
            return Err(e);
        }
        Err(_) => {
            workers.stop();
            return Err("encode init timeout".into());
        }
    }

    Ok(Box::new(GpuMediaHandle { workers }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workers_activation_and_init_handshake_contract() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let mut workers = native_pipeline::Workers::<DmabufCapturedFrame>::new();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let exited = Arc::new(AtomicBool::new(false));

        let exited_clone = Arc::clone(&exited);
        workers
            .spawn("test-init", true, move |_handoff| {
                let _ = ready_tx.send(Ok(()));
                exited_clone.store(true, Ordering::Release);
            })
            .unwrap();

        assert!(ready_rx.recv_timeout(Duration::from_millis(20)).is_err());

        workers.handoff.activate();
        assert!(ready_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_ok());

        workers.stop();
        assert!(exited.load(Ordering::Acquire), "worker must exit upon stop");
    }

    #[test]
    fn workers_failure_stops_and_joins_without_leaks() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let mut workers = native_pipeline::Workers::<DmabufCapturedFrame>::new();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let exited = Arc::new(AtomicBool::new(false));

        let exited_clone = Arc::clone(&exited);
        workers
            .spawn("test-fail", true, move |_handoff| {
                let _ = ready_tx.send(Err("device failure".into()));
                exited_clone.store(true, Ordering::Release);
            })
            .unwrap();

        workers.handoff.activate();
        let err = ready_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap_err();
        assert_eq!(err, "device failure");

        workers.stop();
        assert!(
            exited.load(Ordering::Acquire),
            "worker must exit upon failure stop"
        );
    }
}
