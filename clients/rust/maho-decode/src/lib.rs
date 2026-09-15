//! FFmpeg HEVC decoding for the MahoRD AVCC media stream.

use thiserror::Error;

pub const NAL_LENGTH_BYTES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareAcceleration {
    Software,
    #[cfg(any(feature = "videotoolbox", feature = "ios-videotoolbox"))]
    VideoToolbox,
    #[cfg(feature = "d3d11va")]
    D3d11va,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nalu<'a> {
    pub nal_type: u8,
    pub data: &'a [u8],
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("HEVC access unit is truncated at byte {offset}")]
    TruncatedNalu { offset: usize },
    #[error("HEVC NALU at byte {offset} has zero length")]
    EmptyNalu { offset: usize },
    #[error("HEVC parameter sets (VPS/SPS/PPS) are incomplete")]
    MissingParameterSets,
    #[error("FFmpeg support is disabled")]
    FfmpegDisabled,
    #[error("FFmpeg initialization failed: {0}")]
    FfmpegInit(String),
    #[error("FFmpeg decoder failed: {0}")]
    Ffmpeg(String),
    #[error("decoded frame has unsupported dimensions or format")]
    UnsupportedFrame,
    #[error("VideoToolbox support is disabled")]
    VideoToolboxDisabled,
    #[error("VideoToolbox initialization failed: {0}")]
    VideoToolboxInit(String),
    #[error("VideoToolbox decoder failed: {0}")]
    VideoToolbox(String),
}

/// Parses one access unit containing 4-byte big-endian length-prefixed HEVC NALUs.
pub fn parse_length_prefixed_nalus(access_unit: &[u8]) -> Result<Vec<Nalu<'_>>, DecodeError> {
    let mut offset = 0;
    let mut nalus = Vec::new();
    while offset < access_unit.len() {
        if access_unit.len() - offset < NAL_LENGTH_BYTES {
            return Err(DecodeError::TruncatedNalu { offset });
        }
        let length = u32::from_be_bytes(
            access_unit[offset..offset + NAL_LENGTH_BYTES]
                .try_into()
                .expect("four-byte NAL length"),
        ) as usize;
        let length_offset = offset;
        offset += NAL_LENGTH_BYTES;
        if length < 2 {
            return Err(DecodeError::EmptyNalu {
                offset: length_offset,
            });
        }
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= access_unit.len())
            .ok_or(DecodeError::TruncatedNalu {
                offset: length_offset,
            })?;
        let data = &access_unit[offset..end];
        nalus.push(Nalu {
            nal_type: (data[0] >> 1) & 0x3f,
            data,
        });
        offset = end;
    }
    if nalus.is_empty() {
        return Err(DecodeError::TruncatedNalu { offset: 0 });
    }
    Ok(nalus)
}

/// Converts length-prefixed NALUs to Annex-B (0x00, 0x00, 0x00, 0x01 prefixed).
pub fn to_annex_b(data: &[u8]) -> Vec<u8> {
    prepare_annex_b(data).unwrap_or_else(|_| data.to_vec())
}

fn prepare_annex_b(data: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let nalus = parse_length_prefixed_nalus(data)?;
    // Four-byte start codes replace four-byte lengths without changing size.
    let mut out = Vec::with_capacity(data.len());
    for nalu in nalus {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nalu.data);
    }
    Ok(out)
}

/// Extracts the direct AVCC-style VPS/SPS/PPS blob expected by the decoder context.
pub fn hevc_parameter_set_blob(access_unit: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let mut parameter_sets: [Option<&[u8]>; 3] = [None, None, None];
    for nalu in parse_length_prefixed_nalus(access_unit)? {
        match nalu.nal_type {
            32 => parameter_sets[0] = Some(nalu.data),
            33 => parameter_sets[1] = Some(nalu.data),
            34 => parameter_sets[2] = Some(nalu.data),
            _ => {}
        }
    }
    if parameter_sets.iter().any(Option::is_none) {
        return Err(DecodeError::MissingParameterSets);
    }
    let mut output = Vec::new();
    for parameter_set in parameter_sets.into_iter().flatten() {
        output.extend_from_slice(&(parameter_set.len() as u32).to_be_bytes());
        output.extend_from_slice(parameter_set);
    }
    Ok(output)
}

/// H.264 analog of [`hevc_parameter_set_blob`]: SPS (NAL type 7) then PPS (8).
pub fn h264_parameter_set_blob(access_unit: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let mut parameter_sets: [Option<&[u8]>; 2] = [None, None];
    for nalu in parse_length_prefixed_nalus(access_unit)? {
        // The shared parser reports HEVC-style nal_type; H.264 lives in the
        // low 5 bits of the header byte instead.
        let h264_type = nalu.data.first().map(|byte| byte & 0x1f).unwrap_or(0);
        match h264_type {
            7 => parameter_sets[0] = Some(nalu.data),
            8 => parameter_sets[1] = Some(nalu.data),
            _ => {}
        }
    }
    if parameter_sets.iter().any(Option::is_none) {
        return Err(DecodeError::MissingParameterSets);
    }
    let mut output = Vec::new();
    for parameter_set in parameter_sets.into_iter().flatten() {
        output.extend_from_slice(&(parameter_set.len() as u32).to_be_bytes());
        output.extend_from_slice(parameter_set);
    }
    Ok(output)
}

/// Which video codec an access unit carries, inferred from its parameter-set
/// NAL units. The wire protocol does not carry a codec field, so the decoder
/// sniffs the first keyframe instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecKind {
    Hevc,
    H264,
}

/// Classifies the first parameter-set NAL of an access unit. H.264 SPS has
/// nal_type 7 (mask 0x1F) while HEVC VPS/SPS/PPS are 32/33/34 after the
/// 1-bit-zero + 6-bit-type shift.
pub fn detect_codec(access_unit: &[u8]) -> Result<CodecKind, DecodeError> {
    for nalu in parse_length_prefixed_nalus(access_unit)? {
        if nalu.data.is_empty() {
            continue;
        }
        let first = nalu.data[0];
        let h264_type = first & 0x1F;
        let hevc_type = (first >> 1) & 0x3F;
        if h264_type == 7 {
            return Ok(CodecKind::H264);
        }
        if hevc_type == 32 || hevc_type == 33 || hevc_type == 34 {
            return Ok(CodecKind::Hevc);
        }
    }
    Err(DecodeError::MissingParameterSets)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nv12Frame {
    pub width: u32,
    pub height: u32,
    pub y_stride: usize,
    pub uv_stride: usize,
    pub y_plane: Vec<u8>,
    pub uv_plane: Vec<u8>,
    pub timestamp_ms: i64,
}

#[cfg(feature = "ffmpeg")]
mod ffmpeg_impl {
    use std::ptr;

    use ffmpeg::{codec, format::Pixel, frame, software::scaling};
    use ffmpeg_next as ffmpeg;

    use super::{
        hevc_parameter_set_blob, parse_length_prefixed_nalus, to_annex_b, DecodeError,
        HardwareAcceleration, Nv12Frame,
    };

    pub struct HevcDecoder {
        decoder: codec::decoder::Video,
        scaler: Option<scaling::Context>,
        acceleration: HardwareAcceleration,
        hw_device: *mut ffmpeg::ffi::AVBufferRef,
    }

    unsafe impl Send for HevcDecoder {}

    impl HevcDecoder {
        pub fn new(extradata: &[u8]) -> Result<Self, DecodeError> {
            Self::with_codec(extradata, codec::Id::HEVC, "HEVC decoder is unavailable")
        }

        /// H.264 variant for hosts whose Media Foundation pipeline emits H.264
        /// (e.g. the Windows software encoder MFT). Parameter sets are SPS/PPS.
        pub fn new_h264(extradata: &[u8]) -> Result<Self, DecodeError> {
            Self::with_codec(extradata, codec::Id::H264, "H264 decoder is unavailable")
        }

        fn with_codec(
            extradata: &[u8],
            codec_id: codec::Id,
            missing_message: &str,
        ) -> Result<Self, DecodeError> {
            ffmpeg::init().map_err(|error| DecodeError::FfmpegInit(error.to_string()))?;
            if extradata.is_empty() {
                return Err(DecodeError::MissingParameterSets);
            }
            let codec = codec::decoder::find(codec_id)
                .ok_or_else(|| DecodeError::Ffmpeg(missing_message.to_owned()))?;
            let mut context = codec::Context::new_with_codec(codec);
            unsafe {
                (*context.as_mut_ptr()).err_recognition =
                    ffmpeg::ffi::AV_EF_CRCCHECK | ffmpeg::ffi::AV_EF_BUFFER;
                (*context.as_mut_ptr()).flags |= ffmpeg::ffi::AV_CODEC_FLAG_LOW_DELAY as i32;
                (*context.as_mut_ptr()).flags2 |= ffmpeg::ffi::AV_CODEC_FLAG2_FAST;
                // Disable multi-frame threading (FF_THREAD_FRAME introduces 1-frame latency per thread).
                // Use slice-level threading (FF_THREAD_SLICE = 2) for zero latency.
                (*context.as_mut_ptr()).thread_type = ffmpeg::ffi::FF_THREAD_SLICE;
                (*context.as_mut_ptr()).thread_count = 4;
            }
            let annex_b = to_annex_b(extradata);
            let raw_len = annex_b.len();
            let padded_len = raw_len + ffmpeg::ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize;
            let buffer = unsafe { ffmpeg::ffi::av_mallocz(padded_len) as *mut u8 };
            if buffer.is_null() {
                return Err(DecodeError::Ffmpeg(
                    "av_mallocz failed for extradata".to_owned(),
                ));
            }
            unsafe {
                ptr::copy_nonoverlapping(annex_b.as_ptr(), buffer, raw_len);
                (*context.as_mut_ptr()).extradata = buffer;
                (*context.as_mut_ptr()).extradata_size = raw_len as i32;
            }
            let (acceleration, hw_device) = configure_hardware(&mut context);
            let decoder = context
                .decoder()
                .video()
                .map_err(|error| DecodeError::Ffmpeg(error.to_string()))?;
            Ok(Self {
                decoder,
                scaler: None,
                acceleration,
                hw_device,
            })
        }

        pub fn from_keyframe(keyframe: &[u8]) -> Result<Self, DecodeError> {
            Self::new(&hevc_parameter_set_blob(keyframe)?)
        }

        /// Builds a decoder from the first keyframe, sniffing H.264 vs HEVC
        /// from its parameter-set NAL units. Windows hosts emit H.264; macOS
        /// hosts emit HEVC.
        pub fn from_keyframe_auto(
            keyframe: &[u8],
        ) -> Result<(super::CodecKind, Self), DecodeError> {
            let kind = super::detect_codec(keyframe)?;
            let decoder = match kind {
                super::CodecKind::Hevc => Self::new(&super::hevc_parameter_set_blob(keyframe)?),
                super::CodecKind::H264 => {
                    Self::new_h264(&super::h264_parameter_set_blob(keyframe)?)
                }
            }?;
            Ok((kind, decoder))
        }

        pub fn acceleration(&self) -> HardwareAcceleration {
            self.acceleration
        }

        /// Feeds one complete 4-byte length-prefixed access unit to FFmpeg.
        pub fn decode(
            &mut self,
            access_unit: &[u8],
            timestamp_ms: i64,
        ) -> Result<Vec<Nv12Frame>, DecodeError> {
            let nalus = parse_length_prefixed_nalus(access_unit)?;
            let mut packet = ffmpeg::Packet::new(access_unit.len());
            let output = packet.data_mut().expect("nonempty packet has storage");
            let mut offset = 0;
            for nalu in nalus {
                output[offset..offset + 4].copy_from_slice(&[0, 0, 0, 1]);
                offset += 4;
                output[offset..offset + nalu.data.len()].copy_from_slice(nalu.data);
                offset += nalu.data.len();
            }
            packet.set_pts(Some(timestamp_ms));
            packet.set_dts(Some(timestamp_ms));
            self.decoder
                .send_packet(&packet)
                .map_err(|error| DecodeError::Ffmpeg(error.to_string()))?;
            self.receive_available(timestamp_ms)
        }

        pub fn flush(&mut self) -> Result<Vec<Nv12Frame>, DecodeError> {
            self.decoder
                .send_eof()
                .map_err(|error| DecodeError::Ffmpeg(error.to_string()))?;
            self.receive_available(0)
        }

        fn receive_available(
            &mut self,
            fallback_timestamp_ms: i64,
        ) -> Result<Vec<Nv12Frame>, DecodeError> {
            let mut frames = Vec::new();
            loop {
                let mut decoded = frame::Video::empty();
                match self.decoder.receive_frame(&mut decoded) {
                    Ok(()) => {
                        let timestamp_ms = decoded
                            .timestamp()
                            .or_else(|| decoded.pts())
                            .unwrap_or(fallback_timestamp_ms);
                        if matches!(
                            decoded.format(),
                            Pixel::VIDEOTOOLBOX | Pixel::D3D11 | Pixel::D3D11VA_VLD
                        ) {
                            let decoded = transfer_hardware_frame(&decoded)?;
                            frames.push(self.convert_to_nv12(&decoded, timestamp_ms)?);
                        } else {
                            frames.push(self.convert_to_nv12(&decoded, timestamp_ms)?);
                        }
                    }
                    Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
                    Err(ffmpeg::Error::Eof) => break,
                    Err(error) => return Err(DecodeError::Ffmpeg(error.to_string())),
                }
            }
            Ok(frames)
        }

        fn convert_to_nv12(
            &mut self,
            decoded: &frame::Video,
            timestamp_ms: i64,
        ) -> Result<Nv12Frame, DecodeError> {
            let converted;
            let source = if decoded.format() == Pixel::NV12 {
                decoded
            } else {
                let needs_scaler = self.scaler.as_ref().map_or(true, |scaler| {
                    scaler.input().format != decoded.format()
                        || scaler.input().width != decoded.width()
                        || scaler.input().height != decoded.height()
                });
                if needs_scaler {
                    self.scaler = Some(
                        scaling::Context::get(
                            decoded.format(),
                            decoded.width(),
                            decoded.height(),
                            Pixel::NV12,
                            decoded.width(),
                            decoded.height(),
                            scaling::Flags::FAST_BILINEAR,
                        )
                        .map_err(|error| DecodeError::Ffmpeg(error.to_string()))?,
                    );
                }
                converted = {
                    let mut converted = frame::Video::empty();
                    self.scaler
                        .as_mut()
                        .expect("scaler initialized above")
                        .run(decoded, &mut converted)
                        .map_err(|error| DecodeError::Ffmpeg(error.to_string()))?;
                    converted
                };
                &converted
            };
            copy_nv12(source, timestamp_ms)
        }
    }

    impl Drop for HevcDecoder {
        fn drop(&mut self) {
            unsafe {
                if !self.hw_device.is_null() {
                    ffmpeg::ffi::av_buffer_unref(&mut self.hw_device);
                }
            }
        }
    }

    fn transfer_hardware_frame(decoded: &frame::Video) -> Result<frame::Video, DecodeError> {
        let mut software = frame::Video::empty();
        let result = unsafe {
            ffmpeg::ffi::av_hwframe_transfer_data(software.as_mut_ptr(), decoded.as_ptr(), 0)
        };
        if result < 0 {
            return Err(DecodeError::Ffmpeg(ffmpeg::Error::from(result).to_string()));
        }
        Ok(software)
    }

    pub(super) fn copy_nv12(
        frame: &frame::Video,
        timestamp_ms: i64,
    ) -> Result<Nv12Frame, DecodeError> {
        if frame.format() != Pixel::NV12 || frame.planes() < 2 {
            return Err(DecodeError::UnsupportedFrame);
        }
        let width = frame.width() as usize;
        let height = frame.height() as usize;
        let y_stride = frame.stride(0);
        let uv_stride = frame.stride(1);
        if y_stride < width
            || uv_stride < width
            || frame.data(0).len() < y_stride * height
            || frame.data(1).len() < uv_stride * height.div_ceil(2)
        {
            return Err(DecodeError::UnsupportedFrame);
        }
        let mut y_plane = vec![0_u8; width * height];
        let mut uv_plane = vec![0_u8; width * height.div_ceil(2)];
        for row in 0..height {
            let source = &frame.data(0)[row * y_stride..row * y_stride + width];
            y_plane[row * width..(row + 1) * width].copy_from_slice(source);
        }
        for row in 0..height.div_ceil(2) {
            let source = &frame.data(1)[row * uv_stride..row * uv_stride + width];
            uv_plane[row * width..(row + 1) * width].copy_from_slice(source);
        }
        Ok(Nv12Frame {
            width: width as u32,
            height: height as u32,
            y_stride: width,
            uv_stride: width,
            y_plane,
            uv_plane,
            timestamp_ms,
        })
    }

    fn configure_hardware(
        _context: &mut codec::Context,
    ) -> (HardwareAcceleration, *mut ffmpeg::ffi::AVBufferRef) {
        #[cfg(all(feature = "videotoolbox", target_os = "macos"))]
        if let Some(device) = create_hw_device(
            _context,
            ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
        ) {
            return (HardwareAcceleration::VideoToolbox, device);
        }
        #[cfg(all(feature = "d3d11va", target_os = "windows"))]
        if let Some(device) = create_hw_device(
            _context,
            ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
        ) {
            return (HardwareAcceleration::D3d11va, device);
        }
        (HardwareAcceleration::Software, ptr::null_mut())
    }

    #[cfg(any(
        all(feature = "videotoolbox", target_os = "macos"),
        all(feature = "d3d11va", target_os = "windows")
    ))]
    fn create_hw_device(
        context: &mut codec::Context,
        device_type: ffmpeg::ffi::AVHWDeviceType,
    ) -> Option<*mut ffmpeg::ffi::AVBufferRef> {
        let mut device = ptr::null_mut();
        let result = unsafe {
            ffmpeg::ffi::av_hwdevice_ctx_create(
                &mut device,
                device_type,
                ptr::null(),
                ptr::null_mut(),
                0,
            )
        };
        if result < 0 || device.is_null() {
            return None;
        }
        unsafe {
            (*context.as_mut_ptr()).hw_device_ctx = ffmpeg::ffi::av_buffer_ref(device);
        }
        Some(device)
    }

    pub use HevcDecoder as ExportedHevcDecoder;
}

#[cfg(feature = "ffmpeg")]
pub use ffmpeg_impl::ExportedHevcDecoder as HevcDecoder;

#[cfg(all(
    not(feature = "ffmpeg"),
    feature = "ios-videotoolbox",
    any(target_os = "ios", target_os = "macos")
))]
mod vt;

#[cfg(all(
    not(feature = "ffmpeg"),
    feature = "ios-videotoolbox",
    any(target_os = "ios", target_os = "macos")
))]
pub use vt::HevcDecoder;

#[cfg(all(
    not(feature = "ffmpeg"),
    not(all(
        feature = "ios-videotoolbox",
        any(target_os = "ios", target_os = "macos")
    ))
))]
pub struct HevcDecoder;

#[cfg(all(
    not(feature = "ffmpeg"),
    not(all(
        feature = "ios-videotoolbox",
        any(target_os = "ios", target_os = "macos")
    ))
))]
impl HevcDecoder {
    pub fn new(_extradata: &[u8]) -> Result<Self, DecodeError> {
        Err(DecodeError::FfmpegDisabled)
    }

    pub fn new_h264(_extradata: &[u8]) -> Result<Self, DecodeError> {
        Err(DecodeError::FfmpegDisabled)
    }

    pub fn from_keyframe(_keyframe: &[u8]) -> Result<Self, DecodeError> {
        Err(DecodeError::FfmpegDisabled)
    }

    pub fn from_keyframe_auto(_keyframe: &[u8]) -> Result<(CodecKind, Self), DecodeError> {
        Err(DecodeError::FfmpegDisabled)
    }

    pub fn decode(
        &mut self,
        _access_unit: &[u8],
        _timestamp_ms: i64,
    ) -> Result<Vec<Nv12Frame>, DecodeError> {
        Err(DecodeError::FfmpegDisabled)
    }

    pub fn flush(&mut self) -> Result<Vec<Nv12Frame>, DecodeError> {
        Err(DecodeError::FfmpegDisabled)
    }

    pub fn acceleration(&self) -> HardwareAcceleration {
        HardwareAcceleration::Software
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "ffmpeg")]
    #[test]
    fn decoder_packet_avoids_intermediate_payload_copy() {
        let access_unit: Vec<_> = include_str!("../tests/fixtures/black_16x16.hevc.hex")
            .split_whitespace()
            .map(|byte| u8::from_str_radix(byte, 16).unwrap())
            .collect();
        let mut decoder = HevcDecoder::new(&access_unit).unwrap();
        assert_eq!(decoder.decode(&access_unit, 1).unwrap().len(), 1);
        let (frames, count) =
            crate::test_alloc::allocations(|| decoder.decode(&access_unit, 2).unwrap());
        assert_eq!(frames.len(), 1);
        let frame = &frames[0];
        assert_eq!((frame.width, frame.height, frame.timestamp_ms), (16, 16, 2));
        for row in frame.y_plane.chunks(frame.y_stride).take(16) {
            assert_eq!(&row[..16], &[16; 16]);
        }
        for row in frame.uv_plane.chunks(frame.uv_stride).take(8) {
            assert_eq!(&row[..16], &[128; 16]);
        }
        eprintln!("warm decoded 16x16 HEVC Rust allocations: {count}");
        assert!(
            count <= 4,
            "decoded planes, output list and NAL metadata must not include a copied input Vec"
        );
    }

    #[cfg(feature = "ffmpeg")]
    #[test]
    fn copy_nv12_rejects_undersized_chroma_stride() {
        use ffmpeg_next::{format::Pixel, frame};
        // Given an NV12 frame whose chroma rows are narrower than the frame width.
        let mut frame = frame::Video::new(Pixel::NV12, 16, 16);
        assert!(crate::ffmpeg_impl::copy_nv12(&frame, 0).is_ok());
        unsafe {
            (*frame.as_mut_ptr()).linesize[1] = 8;
        }
        // When the FFmpeg path copies it.
        let result = crate::ffmpeg_impl::copy_nv12(&frame, 0);
        // Then the frame is rejected instead of panicking the decode thread.
        assert!(matches!(result, Err(DecodeError::UnsupportedFrame)));
    }

    #[cfg(feature = "ffmpeg")]
    #[test]
    fn decoder_initialization_with_corrupt_extradata_cleans_up_safely() {
        // Given random non-parameter bytes passed as extradata
        let bogus = [0xde, 0xad, 0xbe, 0xef, 0x00, 0x11, 0x22, 0x33];
        // When HevcDecoder is initialized
        let decoder = HevcDecoder::new(&bogus);
        // It cleans up safely without allocator mismatch or double free
        drop(decoder);
    }

    #[test]
    fn annex_b_preserves_payloads_when_replacing_length_prefixes() {
        // Given multiple NALs, including an emulation-prevention sequence.
        let data = [0, 0, 0, 5, 64, 1, 0, 0, 3, 0, 0, 0, 2, 103, 2];
        let expected = [0, 0, 0, 1, 64, 1, 0, 0, 3, 0, 0, 0, 1, 103, 2];
        // When strict NAL normalization converts the access unit.
        let prepared = prepare_annex_b(&data).unwrap();
        // Then only the four-byte prefixes change; public normalization agrees.
        assert_eq!(prepared, expected);
        assert_eq!(prepared.capacity(), data.len());
        assert_eq!(to_annex_b(&data), expected);
    }

    #[test]
    fn parser_borrows_payloads_when_access_unit_has_multiple_nals() {
        // Given two distinct HEVC NAL headers.
        let data = [0, 0, 0, 2, 64, 1, 0, 0, 0, 2, 66, 2];
        // When the public parser returns its NAL metadata.
        let nalus = parse_length_prefixed_nalus(&data).unwrap();
        // Then NAL types and payload slices retain the public borrowed contract.
        assert_eq!(nalus.len(), 2);
        assert_eq!((nalus[0].nal_type, nalus[1].nal_type), (32, 33));
        assert_eq!(nalus[0].data, &data[4..6]);
        assert_eq!(nalus[1].data, &data[10..12]);
        assert_eq!(nalus[0].data.as_ptr(), data[4..].as_ptr());
        assert_eq!(nalus[1].data.as_ptr(), data[10..].as_ptr());
    }

    #[test]
    fn strict_errors_preserve_offsets_when_normalization_falls_back() {
        // Given empty, short-prefix, short-NAL, oversized and trailing garbage inputs.
        let cases: &[(&[u8], bool, usize)] = &[
            (&[], false, 0),
            (&[0], false, 0),
            (&[0, 0, 0], false, 0),
            (&[0, 0, 0, 0], true, 0),
            (&[0, 0, 0, 1, 64], true, 0),
            (&[0, 0, 0, 8, 64, 1], false, 0),
            (&[255, 255, 255, 255], false, 0),
            (&[0, 0, 0, 2, 64, 1, 0], false, 6),
        ];
        // When strict preparation receives malformed lengths.
        for &(data, empty, offset) in cases {
            // Then errors retain their type/offset; public normalization falls back.
            for error in [
                prepare_annex_b(data).unwrap_err(),
                parse_length_prefixed_nalus(data).unwrap_err(),
            ] {
                match error {
                    DecodeError::EmptyNalu { offset: actual } => {
                        assert!(empty);
                        assert_eq!(actual, offset);
                    }
                    DecodeError::TruncatedNalu { offset: actual } => {
                        assert!(!empty);
                        assert_eq!(actual, offset);
                    }
                    other => panic!("unexpected error: {other:?}"),
                }
            }
            assert_eq!(to_annex_b(data), data);
        }
    }

    #[test]
    fn corrupt_length_prefix_is_a_graceful_error() {
        assert!(matches!(
            parse_length_prefixed_nalus(&[0, 0, 0, 8, 1, 2]),
            Err(DecodeError::TruncatedNalu { offset: 0 })
        ));
    }

    #[test]
    fn parameter_set_blob_keeps_four_byte_lengths() {
        let mut keyframe = Vec::new();
        for data in [[0x40, 1], [0x42, 2], [0x44, 3]] {
            keyframe.extend_from_slice(&(data.len() as u32).to_be_bytes());
            keyframe.extend_from_slice(&data);
        }
        assert_eq!(hevc_parameter_set_blob(&keyframe).unwrap(), keyframe);
    }

    #[test]
    fn h264_parameter_set_blob_extracts_sps_and_pps() {
        let mut keyframe = Vec::new();
        let sps = [0x67, 0x42, 0xc0, 0x0a];
        keyframe.extend_from_slice(&(sps.len() as u32).to_be_bytes());
        keyframe.extend_from_slice(&sps);
        let pps = [0x68, 0xce, 0x3c, 0x80];
        keyframe.extend_from_slice(&(pps.len() as u32).to_be_bytes());
        keyframe.extend_from_slice(&pps);
        let idr = [0x65, 0x88, 0x84, 0x00];
        keyframe.extend_from_slice(&(idr.len() as u32).to_be_bytes());
        keyframe.extend_from_slice(&idr);

        let blob = h264_parameter_set_blob(&keyframe).unwrap();
        let expected = {
            let mut out = Vec::new();
            out.extend_from_slice(&(sps.len() as u32).to_be_bytes());
            out.extend_from_slice(&sps);
            out.extend_from_slice(&(pps.len() as u32).to_be_bytes());
            out.extend_from_slice(&pps);
            out
        };
        assert_eq!(blob, expected);
    }

    #[test]
    fn h264_parameter_set_blob_missing_parameter_fails() {
        let mut keyframe = Vec::new();
        let sps = [0x67, 0x42, 0xc0, 0x0a];
        keyframe.extend_from_slice(&(sps.len() as u32).to_be_bytes());
        keyframe.extend_from_slice(&sps);
        assert!(matches!(
            h264_parameter_set_blob(&keyframe),
            Err(DecodeError::MissingParameterSets)
        ));
    }

    #[test]
    fn detect_codec_identifies_hevc_and_h264() {
        let mut hevc_au = Vec::new();
        let vps = [0x40, 0x01, 0x0c];
        hevc_au.extend_from_slice(&(vps.len() as u32).to_be_bytes());
        hevc_au.extend_from_slice(&vps);
        assert_eq!(detect_codec(&hevc_au).unwrap(), CodecKind::Hevc);

        let mut h264_au = Vec::new();
        let sps = [0x67, 0x42, 0xc0];
        h264_au.extend_from_slice(&(sps.len() as u32).to_be_bytes());
        h264_au.extend_from_slice(&sps);
        assert_eq!(detect_codec(&h264_au).unwrap(), CodecKind::H264);

        let mut idr_au = Vec::new();
        let idr = [0x65, 0x88];
        idr_au.extend_from_slice(&(idr.len() as u32).to_be_bytes());
        idr_au.extend_from_slice(&idr);
        assert!(matches!(
            detect_codec(&idr_au),
            Err(DecodeError::MissingParameterSets)
        ));
    }
}

#[cfg(all(test, feature = "ffmpeg"))]
#[path = "../../test-support/allocations.rs"]
mod test_alloc;
