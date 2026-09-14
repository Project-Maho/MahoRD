//! FFmpeg video encoding for the Linux host.
//!
//! The preferred encoders are `hevc_vaapi` and `h264_vaapi`. They use libva
//! through FFmpeg's VAAPI hardware-device and hardware-frame contexts. BGRA
//! capture frames are converted to NV12 and uploaded to VAAPI surfaces. If no
//! VAAPI encoder/device can be opened, `libx264` is opened with `ultrafast` +
//! `zerolatency`, no B-frames, and a one-frame-thread policy.
//!
//! Output is always AVCC-style: every NAL unit has a four-byte big-endian
//! length prefix. FFmpeg's Annex-B output is converted when necessary. On
//! keyframes the cached VPS/SPS/PPS (HEVC) or SPS/PPS (H.264) are prepended,
//! matching the macOS VideoToolbox contract.
//!
//! Runtime QA on the deployment host must exercise the actual GPU. For Arch,
//! install `ffmpeg`, `libva`, and the vendor VA driver (`libva-mesa-driver` or
//! `intel-media-driver`), then confirm `vainfo` and `ffmpeg -encoders` expose
//! the selected codec.

use std::{ffi::CString, ptr};

use ffmpeg_next as ffmpeg;
use thiserror::Error;

use ffmpeg::{
    codec,
    format::Pixel,
    frame,
    software::scaling::{context::Context as ScaleContext, flag::Flags as ScaleFlags},
    Dictionary, Packet,
};

#[cfg(test)]
thread_local! {
    static SOFTWARE_FRAME_ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn allocate_software_frame(format: Pixel, width: u32, height: u32) -> frame::Video {
    let frame = frame::Video::new(format, width, height);
    #[cfg(test)]
    SOFTWARE_FRAME_ALLOCATIONS.with(|count| count.set(count.get() + 1));
    frame
}

fn make_frame_writable(frame: &mut frame::Video) -> Result<(), EncodeError> {
    // SAFETY: Video owns a live AVFrame. The exclusive borrow prevents Rust aliases;
    // FFmpeg detaches shared AVBufferRefs before the caller mutates retained pixels.
    let status = unsafe { ffmpeg::ffi::av_frame_make_writable(frame.as_mut_ptr()) };
    if status < 0 {
        return Err(EncodeError::Ffmpeg(ffmpeg::Error::from(status)));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Hevc,
    H264,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderBackend {
    Nvenc,
    Vaapi,
    X264,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub bitrate: usize,
    pub fps: u32,
    pub keyframe_interval: u32,
    pub preferred_codec: VideoCodec,
}

impl EncoderConfig {
    pub fn validate(self) -> Result<Self, EncodeError> {
        if self.width == 0 || self.height == 0 || self.fps == 0 || self.bitrate == 0 {
            return Err(EncodeError::InvalidConfig);
        }
        // The NV12 conversion reads and writes pixel pairs per 2x2 block.
        if self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(EncodeError::InvalidConfig);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub is_key_frame: bool,
    pub codec: VideoCodec,
    pub pts: i64,
}

#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("encoder dimensions must be non-zero and even; FPS and bitrate must be non-zero")]
    InvalidConfig,
    #[error("BGRA frame length/stride does not match the configured dimensions")]
    InvalidFrame,
    #[error("no VAAPI encoder or libx264 fallback is available")]
    EncoderUnavailable,
    #[error("FFmpeg error: {0}")]
    Ffmpeg(#[from] ffmpeg::Error),
    #[error("encoded packet is neither valid Annex-B nor AVCC")]
    InvalidBitstream,
    #[error("encoded packet is missing its input presentation timestamp")]
    MissingOutputTimestamp,
}

struct VaapiResources {
    device: *mut ffmpeg::ffi::AVBufferRef,
    frames: *mut ffmpeg::ffi::AVBufferRef,
}

impl Drop for VaapiResources {
    fn drop(&mut self) {
        unsafe {
            ffmpeg::ffi::av_buffer_unref(&mut self.frames);
            ffmpeg::ffi::av_buffer_unref(&mut self.device);
        }
    }
}

struct OpenEncoder {
    encoder: ffmpeg::codec::encoder::video::Encoder,
    backend: EncoderBackend,
    codec: VideoCodec,
    scaler: ScaleContext,
    software: frame::Video,
    source: frame::Video,
    vaapi: Option<VaapiResources>,
}

pub struct LinuxVideoEncoder {
    config: EncoderConfig,
    open: OpenEncoder,
    next_pts: i64,
    force_keyframe: bool,
    parameter_sets: Vec<Vec<u8>>,
}

impl LinuxVideoEncoder {
    pub fn new(config: EncoderConfig) -> Result<Self, EncodeError> {
        let config = config.validate()?;
        ffmpeg::init()?;

        let attempts = match config.preferred_codec {
            VideoCodec::Hevc => [VideoCodec::Hevc, VideoCodec::H264],
            VideoCodec::H264 => [VideoCodec::H264, VideoCodec::Hevc],
        };
        let mut last_error = None;
        // Priority 1: Hardware NVENC on NVIDIA
        for codec in attempts {
            match open_nvenc(config, codec) {
                Ok(open) => {
                    tracing::info!(backend = ?open.backend, codec = ?open.codec, "Initialized Linux NVENC encoder");
                    return Ok(Self {
                        config,
                        open,
                        next_pts: 0,
                        force_keyframe: true,
                        parameter_sets: Vec::new(),
                    });
                }
                Err(error) => last_error = Some(error),
            }
        }
        // Priority 2: VAAPI
        for codec in attempts {
            match open_vaapi(config, codec) {
                Ok(open) => {
                    return Ok(Self {
                        config,
                        open,
                        next_pts: 0,
                        force_keyframe: true,
                        parameter_sets: Vec::new(),
                    });
                }
                Err(error) => last_error = Some(error),
            }
        }
        match open_x264(config) {
            Ok(open) => Ok(Self {
                config,
                open,
                next_pts: 0,
                force_keyframe: true,
                parameter_sets: Vec::new(),
            }),
            Err(_) => Err(last_error.unwrap_or(EncodeError::EncoderUnavailable)),
        }
    }

    pub fn backend(&self) -> EncoderBackend {
        self.open.backend
    }

    pub fn codec(&self) -> VideoCodec {
        self.open.codec
    }

    pub fn force_key_frame(&mut self) {
        self.force_keyframe = true;
    }

    pub fn update_bitrate(&mut self, bitrate: usize) -> Result<Vec<EncodedFrame>, EncodeError> {
        let config = EncoderConfig {
            bitrate,
            ..self.config
        }
        .validate()?;
        if bitrate == self.config.bitrate {
            return Ok(Vec::new());
        }
        // Reopen before draining: post-open setters are not honored by all codecs.
        let replacement = match self.open.backend {
            EncoderBackend::Nvenc => open_nvenc(config, self.open.codec)?,
            EncoderBackend::Vaapi => open_vaapi(config, self.open.codec)?,
            EncoderBackend::X264 => open_x264(config)?,
        };
        let pending = self.drain()?;
        self.open = replacement;
        self.config = config;
        self.parameter_sets.clear();
        self.force_keyframe = true;
        Ok(pending)
    }

    /// Encodes one tightly packed or padded BGRA frame.
    pub fn encode_bgra(
        &mut self,
        bgra: &[u8],
        stride: usize,
    ) -> Result<Vec<EncodedFrame>, EncodeError> {
        let row_bytes = self.config.width as usize * 4;
        let required = stride
            .checked_mul(self.config.height as usize)
            .ok_or(EncodeError::InvalidFrame)?;
        if stride < row_bytes || bgra.len() < required {
            return Err(EncodeError::InvalidFrame);
        }

        let pts = self.next_pts;
        self.next_pts += 1;
        let software = &mut self.open.software;
        make_frame_writable(software)?;

        if self.open.backend == EncoderBackend::Nvenc || self.open.backend == EncoderBackend::Vaapi
        {
            let width = self.config.width as usize;
            let height = self.config.height as usize;
            let y_stride = software.stride(0);
            let uv_stride = software.stride(1);
            let dst_y = software.data_mut(0);

            for y in 0..height {
                let s_row = &bgra[y * stride..y * stride + width * 4];
                let dy_row = &mut dst_y[y * y_stride..y * y_stride + width];
                for (x, dst_pixel) in dy_row.iter_mut().enumerate() {
                    let p = x * 4;
                    let b = s_row[p] as i32;
                    let g = s_row[p + 1] as i32;
                    let r = s_row[p + 2] as i32;
                    *dst_pixel = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8;
                }
            }

            let dst_uv = software.data_mut(1);
            let uv_height = height / 2;
            for uv_y in 0..uv_height {
                let y = uv_y * 2;
                let s_row = &bgra[y * stride..y * stride + width * 4];
                let duv_row = &mut dst_uv[uv_y * uv_stride..uv_y * uv_stride + width];
                for x in (0..width).step_by(2) {
                    let p0 = x * 4;
                    let p1 = (x + 1) * 4;
                    let r_avg = (s_row[p0 + 2] as i32 + s_row[p1 + 2] as i32) >> 1;
                    let g_avg = (s_row[p0 + 1] as i32 + s_row[p1 + 1] as i32) >> 1;
                    let b_avg = (s_row[p0] as i32 + s_row[p1] as i32) >> 1;
                    duv_row[x] = (((-38 * r_avg - 74 * g_avg + 112 * b_avg + 128) >> 8) + 128)
                        .clamp(0, 255) as u8;
                    duv_row[x + 1] = (((112 * r_avg - 94 * g_avg - 18 * b_avg + 128) >> 8) + 128)
                        .clamp(0, 255) as u8;
                }
            }
        } else {
            // For x264 software fallback (YUV420P)
            let source = &mut self.open.source;
            let source_stride = source.stride(0);
            for row in 0..self.config.height as usize {
                let input_start = row * stride;
                let output_start = row * source_stride;
                source.data_mut(0)[output_start..output_start + row_bytes]
                    .copy_from_slice(&bgra[input_start..input_start + row_bytes]);
            }
            self.open.scaler.run(source, software)?;
        }
        software.set_pts(Some(pts));
        if self.force_keyframe {
            software.set_kind(ffmpeg::picture::Type::I);
        } else {
            software.set_kind(ffmpeg::picture::Type::None);
        }

        let hardware = if let Some(vaapi) = &self.open.vaapi {
            let mut hardware = frame::Video::empty();
            unsafe {
                let status =
                    ffmpeg::ffi::av_hwframe_get_buffer(vaapi.frames, hardware.as_mut_ptr(), 0);
                if status < 0 {
                    return Err(EncodeError::Ffmpeg(ffmpeg::Error::from(status)));
                }
                let status = ffmpeg::ffi::av_hwframe_transfer_data(
                    hardware.as_mut_ptr(),
                    software.as_ptr(),
                    0,
                );
                if status < 0 {
                    return Err(EncodeError::Ffmpeg(ffmpeg::Error::from(status)));
                }
            }
            hardware.set_pts(Some(pts));
            hardware.set_kind(software.kind());
            Some(hardware)
        } else {
            None
        };

        // EAGAIN from avcodec_send_frame means the output queue is full: drain
        // it and resubmit the same frame instead of failing the pipeline.
        let mut output = Vec::new();
        match self.submit_frame(hardware.as_ref()) {
            Ok(()) => {}
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::ffi::EAGAIN => {
                output = self.receive_packets()?;
                self.submit_frame(hardware.as_ref())?;
            }
            Err(error) => return Err(EncodeError::Ffmpeg(error)),
        }

        self.force_keyframe = false;
        output.extend(self.receive_packets()?);
        Ok(output)
    }

    /// Submits either the VAAPI surface or the software conversion frame.
    fn submit_frame(&mut self, hardware: Option<&frame::Video>) -> Result<(), ffmpeg::Error> {
        match hardware {
            Some(hardware) => self.open.encoder.send_frame(hardware),
            None => self.open.encoder.send_frame(&self.open.software),
        }
    }

    pub fn drain(&mut self) -> Result<Vec<EncodedFrame>, EncodeError> {
        self.open.encoder.send_eof()?;
        self.receive_packets()
    }

    fn receive_packets(&mut self) -> Result<Vec<EncodedFrame>, EncodeError> {
        let mut output = Vec::new();
        loop {
            let mut packet = Packet::empty();
            match self.open.encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    let bytes = packet.data().ok_or(EncodeError::InvalidBitstream)?;
                    let nal_units = parse_nal_units(bytes)?;
                    let is_key = packet.is_key();
                    if is_key {
                        let discovered = extract_parameter_sets(self.open.codec, &nal_units);
                        if !discovered.is_empty() {
                            self.parameter_sets = discovered;
                        }
                    }
                    let mut avcc = Vec::new();
                    if is_key {
                        append_unique_parameter_sets(&mut avcc, &self.parameter_sets, &nal_units)?;
                    }
                    for nalu in &nal_units {
                        append_avcc_nalu(&mut avcc, nalu)?;
                    }
                    output.push(EncodedFrame {
                        data: avcc,
                        is_key_frame: is_key,
                        codec: self.open.codec,
                        pts: packet_timestamp(&packet)?,
                    });
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::ffi::EAGAIN => break,
                Err(ffmpeg::Error::Eof) => break,
                Err(error) => return Err(EncodeError::Ffmpeg(error)),
            }
        }
        Ok(output)
    }
}

fn packet_timestamp(packet: &Packet) -> Result<i64, EncodeError> {
    packet.pts().ok_or(EncodeError::MissingOutputTimestamp)
}

fn base_video_context(
    config: EncoderConfig,
    codec: ffmpeg::Codec,
    pixel_format: Pixel,
) -> Result<ffmpeg::codec::encoder::video::Video, EncodeError> {
    let mut encoder = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()?;
    encoder.set_width(config.width);
    encoder.set_height(config.height);
    encoder.set_format(pixel_format);
    encoder.set_time_base((1, config.fps as i32));
    encoder.set_frame_rate(Some((config.fps as i32, 1)));
    encoder.set_bit_rate(config.bitrate);
    encoder.set_max_bit_rate(config.bitrate);
    encoder.set_gop(config.keyframe_interval.max(1));
    encoder.set_max_b_frames(0);
    encoder.set_threading(codec::threading::Config::count(1));
    Ok(encoder)
}

fn open_nvenc(config: EncoderConfig, video_codec: VideoCodec) -> Result<OpenEncoder, EncodeError> {
    let name = match video_codec {
        VideoCodec::Hevc => "hevc_nvenc",
        VideoCodec::H264 => "h264_nvenc",
    };
    let codec = codec::encoder::find_by_name(name).ok_or(EncodeError::EncoderUnavailable)?;
    let encoder = base_video_context(config, codec, Pixel::NV12)?;
    let mut options = Dictionary::new();
    options.set("preset", "p1");
    options.set("tune", "ull");
    options.set("zerolatency", "1");
    options.set("forced-idr", "1");
    options.set("repeat_headers", "1");

    options.set("bf", "0");
    let encoder = encoder.open_as_with(codec, options)?;
    let scaler = ScaleContext::get(
        Pixel::BGRA,
        config.width,
        config.height,
        Pixel::NV12,
        config.width,
        config.height,
        ScaleFlags::FAST_BILINEAR,
    )?;
    Ok(OpenEncoder {
        encoder,
        backend: EncoderBackend::Nvenc,
        codec: video_codec,
        scaler,
        software: allocate_software_frame(Pixel::NV12, config.width, config.height),
        source: frame::Video::empty(),
        vaapi: None,
    })
}

fn open_vaapi(config: EncoderConfig, video_codec: VideoCodec) -> Result<OpenEncoder, EncodeError> {
    let name = match video_codec {
        VideoCodec::Hevc => "hevc_vaapi",
        VideoCodec::H264 => "h264_vaapi",
    };
    let codec = codec::encoder::find_by_name(name).ok_or(EncodeError::EncoderUnavailable)?;
    let mut encoder = base_video_context(config, codec, Pixel::VAAPI)?;

    let mut device = ptr::null_mut();
    let device_path = std::env::var("MAHO_VAAPI_DEVICE")
        .ok()
        .map(CString::new)
        .transpose()
        .map_err(|_| EncodeError::InvalidConfig)?;
    let status = unsafe {
        ffmpeg::ffi::av_hwdevice_ctx_create(
            &mut device,
            ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            device_path
                .as_ref()
                .map_or(ptr::null(), |path| path.as_ptr()),
            ptr::null_mut(),
            0,
        )
    };
    if status < 0 {
        return Err(EncodeError::Ffmpeg(ffmpeg::Error::from(status)));
    }

    let frames = unsafe { ffmpeg::ffi::av_hwframe_ctx_alloc(device) };
    if frames.is_null() {
        unsafe { ffmpeg::ffi::av_buffer_unref(&mut device) };
        return Err(EncodeError::EncoderUnavailable);
    }
    unsafe {
        let frames_context = (*frames).data.cast::<ffmpeg::ffi::AVHWFramesContext>();
        (*frames_context).format = Pixel::VAAPI.into();
        (*frames_context).sw_format = Pixel::NV12.into();
        (*frames_context).width = config.width as i32;
        (*frames_context).height = config.height as i32;
        (*frames_context).initial_pool_size = 16;
        let status = ffmpeg::ffi::av_hwframe_ctx_init(frames);
        if status < 0 {
            let mut frames = frames;
            ffmpeg::ffi::av_buffer_unref(&mut frames);
            ffmpeg::ffi::av_buffer_unref(&mut device);
            return Err(EncodeError::Ffmpeg(ffmpeg::Error::from(status)));
        }
        (*encoder.as_mut_ptr()).hw_frames_ctx = ffmpeg::ffi::av_buffer_ref(frames);
    }

    let mut options = Dictionary::new();
    options.set("rc_mode", "CBR");
    options.set("idr_interval", "0");
    options.set("bf", "0");
    let encoder = match encoder.open_as_with(codec, options) {
        Ok(encoder) => encoder,
        Err(error) => {
            let resources = VaapiResources { device, frames };
            drop(resources);
            return Err(EncodeError::Ffmpeg(error));
        }
    };
    let scaler = ScaleContext::get(
        Pixel::BGRA,
        config.width,
        config.height,
        Pixel::NV12,
        config.width,
        config.height,
        ScaleFlags::FAST_BILINEAR,
    )?;
    Ok(OpenEncoder {
        encoder,
        backend: EncoderBackend::Vaapi,
        codec: video_codec,
        scaler,
        software: allocate_software_frame(Pixel::NV12, config.width, config.height),
        source: frame::Video::empty(),
        vaapi: Some(VaapiResources { device, frames }),
    })
}

fn open_x264(config: EncoderConfig) -> Result<OpenEncoder, EncodeError> {
    let codec = codec::encoder::find_by_name("libx264").ok_or(EncodeError::EncoderUnavailable)?;
    let encoder = base_video_context(config, codec, Pixel::YUV420P)?;
    let mut options = Dictionary::new();
    options.set("preset", "ultrafast");
    options.set("tune", "zerolatency");
    options.set("bf", "0");
    options.set("sc_threshold", "0");
    options.set("forced-idr", "1");
    options.set("repeat_headers", "1");
    options.set("annexb", "1");
    let encoder = encoder.open_as_with(codec, options)?;
    let scaler = ScaleContext::get(
        Pixel::BGRA,
        config.width,
        config.height,
        Pixel::YUV420P,
        config.width,
        config.height,
        ScaleFlags::FAST_BILINEAR,
    )?;
    Ok(OpenEncoder {
        encoder,
        backend: EncoderBackend::X264,
        codec: VideoCodec::H264,
        scaler,
        software: allocate_software_frame(Pixel::YUV420P, config.width, config.height),
        source: allocate_software_frame(Pixel::BGRA, config.width, config.height),
        vaapi: None,
    })
}

fn append_avcc_nalu(output: &mut Vec<u8>, nalu: &[u8]) -> Result<(), EncodeError> {
    let length = u32::try_from(nalu.len()).map_err(|_| EncodeError::InvalidBitstream)?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(nalu);
    Ok(())
}

fn append_unique_parameter_sets(
    output: &mut Vec<u8>,
    parameter_sets: &[Vec<u8>],
    frame_nalus: &[&[u8]],
) -> Result<(), EncodeError> {
    for parameter_set in parameter_sets {
        if !frame_nalus.contains(&parameter_set.as_slice()) {
            append_avcc_nalu(output, parameter_set)?;
        }
    }
    Ok(())
}

fn parse_nal_units(data: &[u8]) -> Result<Vec<&[u8]>, EncodeError> {
    // A four-byte AVCC length of one is byte-identical to an Annex-B start
    // code. Parse a complete AVCC packet first, then fall back to Annex-B.
    parse_avcc(data).or_else(|_| parse_annex_b(data))
}

fn parse_annex_b(data: &[u8]) -> Result<Vec<&[u8]>, EncodeError> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 <= data.len() {
        let prefix = if data[index..].starts_with(&[0, 0, 0, 1]) {
            Some(4)
        } else if data[index..].starts_with(&[0, 0, 1]) {
            Some(3)
        } else {
            None
        };
        if let Some(length) = prefix {
            starts.push((index, index + length));
            index += length;
        } else {
            index += 1;
        }
    }
    let mut nalus = Vec::new();
    for (position, (_, payload_start)) in starts.iter().enumerate() {
        let payload_end = starts
            .get(position + 1)
            .map_or(data.len(), |(next_start, _)| *next_start);
        let mut trimmed_end = payload_end;
        while trimmed_end > *payload_start && data[trimmed_end - 1] == 0 {
            trimmed_end -= 1;
        }
        if trimmed_end > *payload_start {
            nalus.push(&data[*payload_start..trimmed_end]);
        }
    }
    (!nalus.is_empty())
        .then_some(nalus)
        .ok_or(EncodeError::InvalidBitstream)
}

fn parse_avcc(data: &[u8]) -> Result<Vec<&[u8]>, EncodeError> {
    let mut nalus = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        if offset + 4 > data.len() {
            return Err(EncodeError::InvalidBitstream);
        }
        let length =
            u32::from_be_bytes(data[offset..offset + 4].try_into().expect("four bytes")) as usize;
        offset += 4;
        if length == 0 || offset + length > data.len() {
            return Err(EncodeError::InvalidBitstream);
        }
        nalus.push(&data[offset..offset + length]);
        offset += length;
    }
    (!nalus.is_empty())
        .then_some(nalus)
        .ok_or(EncodeError::InvalidBitstream)
}

fn extract_parameter_sets(codec: VideoCodec, nalus: &[&[u8]]) -> Vec<Vec<u8>> {
    nalus
        .iter()
        .filter(|nalu| match codec {
            VideoCodec::H264 => matches!(nalu.first().map(|byte| byte & 0x1f), Some(7 | 8)),
            VideoCodec::Hevc => {
                nalu.len() >= 2
                    && matches!(
                        nalu.first().map(|byte| (byte >> 1) & 0x3f),
                        Some(32..=34)
                    )
            }
        })
        .map(|nalu| nalu.to_vec())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_frames_are_reused_after_warmup() {
        let config = EncoderConfig {
            width: 32,
            height: 32,
            bitrate: 100_000,
            fps: 30,
            keyframe_interval: 300,
            preferred_codec: VideoCodec::H264,
        };
        let mut encoder = LinuxVideoEncoder {
            config,
            open: open_x264(config).unwrap(),
            next_pts: 0,
            force_keyframe: true,
            parameter_sets: Vec::new(),
        };
        let mut pixels = vec![128; 32 * 32 * 4];
        encoder.encode_bgra(&pixels, 128).unwrap();
        SOFTWARE_FRAME_ALLOCATIONS.with(|count| count.set(0));
        for index in 1..=16 {
            pixels.fill(index * 8);
            let output = encoder.encode_bgra(&pixels, 128).unwrap();
            assert_eq!(output.len(), 1);
            assert_eq!(output[0].pts, i64::from(index));
        }
        SOFTWARE_FRAME_ALLOCATIONS.with(|count| {
            assert_eq!(
                count.get(),
                0,
                "conversion buffers must survive encode calls"
            )
        });
    }

    #[test]
    fn reused_frame_detaches_from_retained_native_reference() {
        ffmpeg::init().unwrap();
        let mut frame = allocate_software_frame(Pixel::NV12, 32, 32);
        frame.data_mut(0).fill(29);
        frame.data_mut(1).fill(71);
        let mut retained = frame::Video::empty();
        // SAFETY: Both wrappers own live AVFrames. av_frame_ref creates shared
        // native ownership; each wrapper independently unreferences it on drop.
        let status = unsafe { ffmpeg::ffi::av_frame_ref(retained.as_mut_ptr(), frame.as_ptr()) };
        assert_eq!(status, 0);
        let original = frame.data(0).as_ptr();
        assert_eq!(retained.data(0).as_ptr(), original);
        make_frame_writable(&mut frame).unwrap();
        assert_ne!(frame.data(0).as_ptr(), original);
        frame.data_mut(0).fill(200);
        frame.data_mut(1).fill(100);
        assert!(retained.data(0).iter().all(|byte| *byte == 29));
        assert!(retained.data(1).iter().all(|byte| *byte == 71));
        let unique = frame.data(0).as_ptr();
        make_frame_writable(&mut frame).unwrap();
        assert_eq!(frame.data(0).as_ptr(), unique);
    }

    #[test]
    fn packet_timestamp_requires_identity_and_preserves_present_pts() {
        let mut packet = Packet::empty();
        assert!(
            matches!(
                packet_timestamp(&packet),
                Err(EncodeError::MissingOutputTimestamp)
            ),
            "missing output PTS must not invent input zero"
        );
        for pts in [0, 1, -7, i64::MAX] {
            packet.set_pts(Some(pts));
            assert_eq!(packet_timestamp(&packet).unwrap(), pts);
        }
    }

    #[test]
    fn forced_key_frame_is_an_idr() {
        ffmpeg::init().unwrap();
        let config = EncoderConfig {
            width: 32,
            height: 32,
            bitrate: 100_000,
            fps: 30,
            keyframe_interval: 300,
            preferred_codec: VideoCodec::H264,
        };
        let mut encoder = LinuxVideoEncoder {
            config,
            open: open_x264(config).unwrap(),
            next_pts: 0,
            force_keyframe: true,
            parameter_sets: Vec::new(),
        };
        let pixels = vec![128; 32 * 32 * 4];
        encoder.encode_bgra(&pixels, 128).unwrap();
        encoder.encode_bgra(&pixels, 128).unwrap();
        encoder.force_key_frame();
        let output = encoder.encode_bgra(&pixels, 128).unwrap();
        assert!(
            output.iter().any(|frame| parse_nal_units(&frame.data)
                .unwrap()
                .iter()
                .any(|nalu| nalu[0] & 31 == 5)),
            "forced I picture must contain an IDR NAL"
        );
    }

    #[test]
    fn converts_annex_b_to_avcc_without_padding() {
        let annex_b = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3];
        let nalus = parse_nal_units(&annex_b).unwrap();
        let mut output = Vec::new();
        for nalu in nalus {
            append_avcc_nalu(&mut output, nalu).unwrap();
        }
        assert_eq!(output, vec![0, 0, 0, 3, 0x67, 1, 2, 0, 0, 0, 2, 0x68, 3]);
    }

    #[test]
    fn detects_h264_and_hevc_parameter_sets() {
        let h264 = [vec![0x67, 1], vec![0x68, 2], vec![0x65, 3]];
        let h264_refs = h264.iter().map(Vec::as_slice).collect::<Vec<_>>();
        assert_eq!(
            extract_parameter_sets(VideoCodec::H264, &h264_refs).len(),
            2
        );

        let hevc = [
            vec![32 << 1, 1],
            vec![33 << 1, 2],
            vec![34 << 1, 3],
            vec![19 << 1, 4],
        ];
        let hevc_refs = hevc.iter().map(Vec::as_slice).collect::<Vec<_>>();
        assert_eq!(
            extract_parameter_sets(VideoCodec::Hevc, &hevc_refs).len(),
            3
        );
    }

    #[test]
    fn bitrate_reopens_codec_and_preserves_pts() {
        ffmpeg::init().unwrap();
        let config = EncoderConfig {
            width: 32,
            height: 32,
            bitrate: 100_000,
            fps: 30,
            keyframe_interval: 300,
            preferred_codec: VideoCodec::H264,
        };
        let mut encoder = LinuxVideoEncoder {
            config,
            open: open_x264(config).unwrap(),
            next_pts: 0,
            force_keyframe: true,
            parameter_sets: Vec::new(),
        };
        let pixels = vec![128; 32 * 32 * 4];
        encoder.encode_bgra(&pixels, 128).unwrap();
        let old_context = unsafe { encoder.open.encoder.as_ptr() };
        encoder.update_bitrate(200_000).unwrap();
        assert_ne!(
            unsafe { encoder.open.encoder.as_ptr() },
            old_context,
            "post-open setters do not reconfigure every backend"
        );
        assert_eq!(encoder.config.bitrate, 200_000);
        let output = encoder.encode_bgra(&pixels, 128).unwrap();
        assert_eq!(output[0].pts, 1);
        assert!(output[0].is_key_frame);
        assert!(parse_nal_units(&output[0].data)
            .unwrap()
            .iter()
            .any(|n| n[0] & 31 == 5));
        assert!(encoder.update_bitrate(0).is_err());
        assert_eq!(encoder.config.bitrate, 200_000);
    }

    #[test]
    fn accepts_existing_avcc() {
        let avcc = [0, 0, 0, 2, 0x65, 1, 0, 0, 0, 1, 0x41];
        assert_eq!(
            parse_nal_units(&avcc).unwrap(),
            vec![&avcc[4..6], &avcc[10..11]]
        );
    }
}
