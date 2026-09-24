# Lane: host-encode

## Scope reviewed
- `clients/rust/maho-host/src/encode_windows.rs` (1,304 lines)
- `clients/rust/maho-host/src/encode_linux.rs` (844 lines)
- `clients/rust/maho-host/src/encode_vt.rs` (722 lines)
- `clients/rust/maho-host/src/sender_packetization_tests.rs` (327 lines)
- `clients/rust/maho-host/src/sender_trace_tests.rs` (156 lines)
- Supporting files read to assess correctness:
  - `clients/rust/maho-host/src/session.rs` (packetization, chunk math, and sender thread integration)
  - `clients/rust/maho-host/src/native_pipeline.rs` (pipeline worker loop, control handoff, FrameTimes)
  - `clients/rust/maho-proto/src/media.rs` (wire protocol codecs and chunk validation)
  - `clients/rust/maho-app/src/media.rs` (receiver reassembly invariants and limits)

## Findings

### [P0] Unhandled EAGAIN from send_frame on Linux Crashes Video Pipeline and VAAPI Surface Pool Size 4 Causes Frequent ENOMEM Exhaustion
- **Location**: `clients/rust/maho-host/src/encode_linux.rs:323` (and `encode_linux.rs:473`)
- **Evidence**:
```rust
            hardware.set_pts(Some(pts));
            hardware.set_kind(software.kind());
            self.open.encoder.send_frame(&hardware)?;
        } else {
            self.open.encoder.send_frame(software)?;
        }
```
Secondary site (`encode_linux.rs:473`):
```rust
        let frames_context = (*frames).data.cast::<ffmpeg::ffi::AVHWFramesContext>();
        (*frames_context).format = Pixel::VAAPI.into();
        (*frames_context).sw_format = Pixel::NV12.into();
        (*frames_context).width = config.width as i32;
        (*frames_context).height = config.height as i32;
        (*frames_context).initial_pool_size = 4;
        let status = ffmpeg::ffi::av_hwframe_ctx_init(frames);
```
- **Impact**: In modern FFmpeg encoder architectures (`avcodec_send_frame`), returning `EAGAIN` is normal and expected behavior whenever internal hardware/software input queues are full; the caller must drain available packets via `receive_packet` and then resubmit the frame (as done in `encode_vt.rs:331-337` and `encode_windows.rs:1003-1011`). `encode_linux.rs` blindly propagates `send_frame(&hardware)?` with `?`. When `EAGAIN` occurs, `encode_bgra` returns an error, causing `native_pipeline::run_encoder` to abort and permanently terminate the encoder thread (`maho-linux-encode`). Compounding this, `initial_pool_size` is hard-coded to only 4 surfaces. Because the VAAPI encoder retains reference frames for inter-prediction alongside in-flight GPU encoding and frame uploads, this pool of 4 surfaces is exhausted under routine render jitter, causing `av_hwframe_get_buffer` to return `AVERROR(ENOMEM)` and aborting the stream.
- **Fix**: Handle `EAGAIN` in `encode_bgra`: if `send_frame` returns `EAGAIN`, call `self.receive_packets()`, retry `send_frame`, and combine the packet batches. Increase `initial_pool_size` in `open_vaapi` to at least 16 (or dynamically configure it based on GOP length and reference count) to provide sufficient headroom for GPU hardware surfaces.
- **Confidence**: high

### [P1] Out-of-Bounds Panic on Odd Resolution Width in Linux NV12 Color Conversion
- **Location**: `clients/rust/maho-host/src/encode_linux.rs:275`
- **Evidence**:
```rust
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
```
- **Impact**: `EncoderConfig::validate` in `encode_linux.rs:71-76` validates that `width != 0`, but does not enforce that `width` is even (unlike `encode_windows.rs:95-103`). When an odd width is configured (e.g. 1919), the final loop iteration has `x = width - 1 = 1918`. Then `p1 = (x + 1) * 4 = width * 4`. `s_row[p1 + 2]` indexes 2 bytes beyond `s_row` (which has length `width * 4`), and `duv_row[x + 1]` indexes `duv_row[width]` (out of bounds for length `width`). This produces an immediate Rust index out-of-bounds panic, crashing the host process.
- **Fix**: Require `width % 2 == 0 && height % 2 == 0` in `EncoderConfig::validate` and in `LinuxVideoEncoder::encode_bgra`.
- **Confidence**: high

### [P1] VideoToolbox Parameter Sets Accumulate Unboundedly and Emit Conflicting VPS/SPS/PPS on Every Keyframe
- **Location**: `clients/rust/maho-host/src/encode_vt.rs:421`
- **Evidence**:
```rust
    let nalus = split_nalus(packet)?;
    for nalu in &nalus {
        let kind = hevc_nalu_type(nalu).ok_or(EncodeError::MalformedHevc)?;
        if matches!(kind, 32..=34) && !parameter_sets.iter().any(|known| known.as_slice() == *nalu)
        {
            parameter_sets.push(nalu.to_vec());
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
```
- **Impact**: In `encode_vt.rs`, `parameter_sets` is an append-only `Vec<Vec<u8>>`. When the encoder generates updated SPS or PPS parameter sets (e.g. across dynamic bitrate updates or slice renegotiation), the new parameter sets are appended without evicting previous parameter sets of the same NAL unit type. On every subsequent keyframe, all historical parameter sets—both stale and current—are prepended into the output AVCC bitstream. Having duplicate, contradictory parameter sets with identical IDs in a single HEVC access unit causes hardware decoders (Apple VideoToolbox, Android MediaCodec, and libavcodec) to experience decoder reinitialization loops, artifacts, or decode failures. In addition, `parameter_sets` grows without bound for the lifetime of the session.
- **Fix**: Store parameter sets keyed by their NAL type or parsed parameter set ID (or replace the cached set when new ones are detected, as in `encode_windows.rs:572-575`), rather than appending unconditionally.
- **Confidence**: high

### [P1] Media Foundation MFT Dynamic Stream Change Violates Property Ordering and Drops Low-Latency Constraints
- **Location**: `clients/rust/maho-host/src/encode_windows.rs:546`
- **Evidence**:
```rust
                let new_type = unsafe { self.transform.GetOutputAvailableType(0, 0)? };
                unsafe { self.transform.SetOutputType(0, &new_type, 0)? };
                self.output_type = new_type;
                self.parameter_sets = media_type_parameter_sets(&self.output_type, self.codec);
                return self.take_output();
```
- **Impact**: Microsoft Media Foundation documentation explicitly dictates that encoder properties (`ICodecAPI`) must be configured before setting media types on the transform. When `ProcessOutput` returns `MF_E_TRANSFORM_STREAM_CHANGE`, `take_output` calls `SetOutputType(0, &new_type, 0)` directly on the candidate type returned by `GetOutputAvailableType(0, 0)`. It does not re-apply `CODECAPI_AVEncCommonLowLatency`, `CODECAPI_AVEncCommonRateControlMode`, bitrate, GOP size, or `CODECAPI_AVEncMPVDefaultBPictureCount = 0`, nor does it enforce `H264_PROFILE_BASELINE`. Consequently, the MFT reverts to default encoder settings, re-enabling B-frames and multi-frame latency buffering, which breaks real-time remote desktop performance. Furthermore, changing output type on an MFT clears its input type; because `SetInputType` is not called, subsequent calls to `ProcessInput` can fail with `MF_E_TRANSFORM_TYPE_NOT_SET`.
- **Fix**: When handling `MF_E_TRANSFORM_STREAM_CHANGE`, re-apply all `ICodecAPI` properties, ensure baseline profile attributes are set on `new_type` before `SetOutputType`, and re-commit the input NV12 type via `transform.SetInputType(0, &input_type, 0)`.
- **Confidence**: high

### [P1] Zero-Length Video Frame Packetization Sends Invalid FrameHeader, Causing Client Reassembly Rejection
- **Location**: `clients/rust/maho-host/src/session.rs:2969`
- **Evidence**:
```rust
        let chunk_count = frame.data.len().div_ceil(SENDER_MAX_VIDEO_CHUNK_BYTES);
        let frame_id = self.frame_id.wrapping_add(1);
        let header = FrameHeader {
            frame_id,
            width: width.min(u16::MAX as u32) as u16,
            height: height.min(u16::MAX as u32) as u16,
            is_key_frame: frame.is_key_frame,
            total_chunks: u16::try_from(chunk_count)
                .map_err(|_| SessionError::Store("encoded frame has too many chunks".into()))?,
            total_size: u32::try_from(frame.data.len())
                .map_err(|_| SessionError::Store("encoded frame is too large".into()))?,
        };
        let encoded_header = header.encode()?;
```
- **Impact**: If `frame.data.is_empty()` (e.g. from an empty flush packet or encoder boundary condition), `chunk_count` calculates to `0`. `FrameHeader::validate` only checks upper bounds (`MAX_CHUNKS_PER_FRAME` and `MAX_FRAME_BYTES`), so `header.encode()` succeeds. `send_frame` sends a `FrameHeader` datagram with `total_chunks: 0, total_size: 0`. On the receiving client, `maho-app/src/media.rs:68-75` checks `if header.total_chunks == 0 || header.total_size == 0 { return Err(MediaAssemblyError::InvalidHeader); }`. The client immediately rejects the frame with an assembly error and drops frame synchronization. While `send_audio` has an explicit guard (`if bytes.is_empty() { return Ok(()); }`), `send_frame` lacks this check.
- **Fix**: Guard `send_frame` at the beginning: `if frame.data.is_empty() { return Ok(()); }` (or return an explicit error), and update `FrameHeader::validate` to require `self.total_chunks > 0 && self.total_size > 0`.
- **Confidence**: high

### [P1] Unbounded Leak of IMFActivate COM Objects and Task Memory in Media Foundation MFT Enumeration
- **Location**: `clients/rust/maho-host/src/encode_windows.rs:742`
- **Evidence**:
```rust
        let entries = slice::from_raw_parts_mut(activations, count as usize);
        for entry in entries {
            let Some(activation) = entry.take() else {
                continue;
            };
            if selected.is_none() {
                let clsid_id = activation
                    .GetGUID(&MFT_TRANSFORM_CLSID_Attribute)
                    .map(|guid| {
                        let bytes = guid.to_u128().to_le_bytes();
                        u64::from_le_bytes(bytes[0..8].try_into().unwrap())
                    })
                    .unwrap_or(0);
                selected = Some((activation.ActivateObject::<IMFTransform>()?, clsid_id));
            }
        }
        CoTaskMemFree(Some(activations.cast()));
```
- **Impact**: `MFTEnumEx` returns a buffer of `IMFActivate` COM interface pointers allocated via `CoTaskMemAlloc`, with a reference count already added for each activation object. In `enumerate_transform`, if `activation.ActivateObject::<IMFTransform>()?` fails on line 757, the function returns early via `?`. `CoTaskMemFree` is bypassed, permanently leaking the activation array buffer. Furthermore, any remaining activation entries (`entries[1..count]`) are never taken and dropped, leaking their COM references and holding the registered transform DLLs loaded in the process memory.
- **Fix**: Wrap `activations` in a scope guard or drain all entries and call `CoTaskMemFree` before propagating an error from `ActivateObject`.
- **Confidence**: high

### [P2] Fatal Encoder Worker Exit on Desktop Resolution Change due to Missing Dynamic Reconfiguration
- **Location**: `clients/rust/maho-host/src/encode_windows.rs:349` (and `session.rs:1462`, `encode_linux.rs:236`, `encode_vt.rs:306`)
- **Evidence**:
In `encode_windows.rs:349`:
```rust
        let expected = nv12_len(self.config.width, self.config.height)?;
        if nv12.len() != expected {
            return Err(EncodeError::InvalidFrameLength {
                expected,
                actual: nv12.len(),
            });
        }
```
Secondary site (`session.rs:1462`):
```rust
                        Err(CaptureError::AccessLost) => {
                            capture =
                                match WindowsCapture::new(display_index, Duration::from_millis(33))
```
- **Impact**: When the host's display resolution changes (e.g. monitor re-plug, desktop resize, or fullscreen mode switch), Windows DXGI capture returns `AccessLost`. The capture worker in `session.rs:1462` recovers by calling `WindowsCapture::new()`, which queries the new physical resolution and begins producing raw frames matching the new dimensions. However, the encoder worker (`MediaFoundationEncoder`) is never notified or reconfigured; it remains initialized with the old resolution. When the next frame arrives at `encode_nv12`, `nv12.len() != expected` fails with `InvalidFrameLength`. This error bubbles out of `WindowsSessionEncoder::encode` and causes `native_pipeline::run_encoder` to exit with an error, killing the video encoder thread and freezing the session. The same vulnerability exists on Linux (`encode_bgra` returns `InvalidFrame`) and macOS (`encode` returns `FrameShape`).
- **Fix**: Propagate resolution change events through the pipeline handoff controls (or detect frame dimension changes in `Encoder::encode`), re-creating or reconfiguring the encoder transform when frame dimensions change rather than failing fatally.
- **Confidence**: high

### [P2] FrameTimes Desynchronization and Unbounded Map Leak in LinuxSessionEncoder on Encoding Failure
- **Location**: `clients/rust/maho-host/src/session.rs:1655` (and `encode_linux.rs:236`)
- **Evidence**:
In `session.rs:1655`:
```rust
    fn encode(&mut self, frame: LinuxRawFrame) -> Result<Vec<VideoFrame>, String> {
        self.times.submitted(frame.captured_at, Instant::now());
        let frames = self
            .encoder
            .encode_bgra(&frame.bgra, frame.stride)
            .map_err(|error| format!("encode: {error}"))?;
        self.output(frames)
    }
```
In `encode_linux.rs:236`:
```rust
        let row_bytes = self.config.width as usize * 4;
        let required = stride
            .checked_mul(self.config.height as usize)
            .ok_or(EncodeError::InvalidFrame)?;
        if stride < row_bytes || bgra.len() < required {
            return Err(EncodeError::InvalidFrame);
        }

        let pts = self.next_pts;
        self.next_pts += 1;
```
- **Impact**: `LinuxSessionEncoder::encode` records frame timestamps via `self.times.submitted`, which unconditionally increments `self.times.next_pts` and inserts into `self.times.pending: BTreeMap`. If `self.encoder.encode_bgra` fails validation on line 239 (`InvalidFrame`) prior to line 242 (`self.next_pts += 1`), `self.times.next_pts` has advanced while `self.encoder.next_pts` has not. Every subsequent frame will have a 1-off PTS mismatch between the encoder output and `self.times`. If `self.times.take(encoded.pts)` fails to match, it returns `format!("encoder output has unknown input PTS {}", encoded.pts)`, killing the pipeline. Furthermore, whenever `encode_bgra` fails at any point, the un-consumed entry in `self.times.pending` is never removed, causing an unbounded memory leak.
- **Fix**: Call `self.times.submitted` only after `encode_bgra` has accepted the frame, or roll back `self.times` on error.
- **Confidence**: high

### [P2] Media Foundation Keyframe State Machine Corrupted when Draining Buffered Frames on Backpressure
- **Location**: `clients/rust/maho-host/src/encode_windows.rs:574`
- **Evidence**:
```rust
        let sample_clean_point = unsafe {
            sample
                .GetUINT32(&MFSampleExtension_CleanPoint)
                .unwrap_or_default()
                != 0
        };
        let is_key_frame = sample_clean_point || !self.first_keyframe_emitted;
        let bytes = sample_bytes(&sample)?;
        let mut nalus = parse_access_unit(&bytes)?;
        if is_key_frame {
            self.force_keyframe = false;
            self.first_keyframe_emitted = true;
```
- **Impact**: `!self.first_keyframe_emitted` is used to force keyframe treatment on the first frame if the underlying MFT does not emit `MFSampleExtension_CleanPoint`. If the encoder returns `MF_E_NOTACCEPTING` on an input sample, `submit_and_drain` calls `process_output()` to drain buffered frames before retrying input. The first frame drained from the transform satisfies `!self.first_keyframe_emitted`, so it is marked `is_key_frame = true` and `first_keyframe_emitted` is set to `true`. When the retried input (the intended keyframe) is subsequently processed and emitted, `first_keyframe_emitted` is already `true`. If the MFT fails to set `MFSampleExtension_CleanPoint` (common among Intel and older Microsoft MFTs), the actual keyframe is emitted with `is_key_frame: false` and without parameter sets. The client cannot decode subsequent frames because it never receives parameter sets for the new GOP.
- **Fix**: Store `force_keyframe` in `InputMetadata` alongside `timestamp_hns` so that each output frame's keyframe status is determined by the specific input that generated it, rather than relying on a global boolean flag in the encoder.
- **Confidence**: high

### [P2] Annex B Start Code Parsing in VideoToolbox Retains Trailing Zero Padding Bytes in AVCC NALUs
- **Location**: `clients/rust/maho-host/src/encode_vt.rs:493`
- **Evidence**:
```rust
    let mut nalus = Vec::new();
    for (position, (start, start_len)) in starts.iter().copied().enumerate() {
        let end = starts
            .get(position + 1)
            .map_or(packet.len(), |(next, _)| *next);
        let nalu_data = &packet[start + start_len..end];
        if !nalu_data.is_empty() {
            nalus.push(nalu_data);
        }
    }
```
- **Impact**: In Annex B bitstreams, 4-byte start codes (`0x00000001`) are frequently preceded by zero bytes (`trailing_zero_8bits` or alignment padding). In `encode_vt.rs`, `split_annex_b` indexes `end` directly at the next start code without trimming preceding zero bytes (unlike `encode_linux.rs:591-594`, which trims trailing zeroes). As a consequence, trailing zero bytes are incorporated into `nalu_data` and encoded into the AVCC length-prefixed payload. When Apple VideoToolbox or Android MediaCodec decodes parameter sets (SPS/PPS) containing trailing zero bytes, parsing fails with `kVTVideoDecoderBadDataErr` (-12909) or format errors, preventing client video decoding.
- **Fix**: Trim trailing zero bytes from `nalu_data` in `split_annex_b` before pushing into `nalus`.
- **Confidence**: high

## Non-findings checked
- Media Foundation buffer lock-unlock pairing in `sample_from_bytes`: buffer is properly unlocked via `buffer.Unlock()` before setting length and adding to sample.
- Media Foundation COM threading affinity: `MediaFoundationRuntime` initializes multithreaded COM via `CoInitializeEx(None, COINIT_MULTITHREADED)` and enforces single-thread affinity via `_thread_affinity: PhantomData<Rc<()>>`.
- Media Foundation output event collection cleanup: `output[0].pEvents` is extracted via `ManuallyDrop::take` across all `take_output` branches, ensuring `IMFCollection` references are freed.
- VAAPI resource management: `VaapiResources::drop` safely unreferences `frames` and `device` contexts using `ffmpeg::ffi::av_buffer_unref`.
- VAAPI frame writability before color conversion: `make_frame_writable` calls `av_frame_make_writable` before modifying software frame buffers, avoiding in-place mutation of shared native frames.
- VideoToolbox thread lifecycle: `VideoToolboxEncoder::drop` signals cancellation, dispatches `Command::Stop`, and joins the worker thread without hanging or leaking threads.
- VideoToolbox bounded queues: command channel (`ENCODE_QUEUE_DEPTH = 3`) and output channel (`OUTPUT_QUEUE_DEPTH = 3`) enforce backpressure bounds to avoid memory bloat.
- AVCC big-endian length prefix encoding: `write_avcc` correctly encodes 4-byte big-endian lengths for NAL units conforming to ISO/IEC 14496-15.
- MTU budget math against MAX_CHUNKS_PER_FRAME: `SENDER_MAX_VIDEO_CHUNK_BYTES = 1154` ensures total UDP datagrams are $\le 1200$ bytes, safely within the 1280-byte IPv6 MTU.
- Sender frame sequence continuity: `UdpSender::send_frame` increments `frame_id` only after successful header encoding, maintaining strict sequence monotonicity.
- Large keyframe packet pacing: frames requiring more than 16 chunks insert 1 ms pauses every 8 datagrams to mitigate network burst drop without impacting 60 fps P-frame delivery.
- Baseline profile and low-latency property ordering on initial setup: `MediaFoundationEncoder::configure` sets `CODECAPI_AVEncCommonLowLatency`, CBR mode, GOP size, and worker threads prior to `SetOutputType`.
