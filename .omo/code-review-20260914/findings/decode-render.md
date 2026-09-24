# Lane: decode-render

Reviewed by the lead directly: this lane's DAG node failed twice with a provider `401 Authentication Failed` at spawn (runs `maho-code-review-20260914-w1` and `-w1b`), so the review was performed in-session rather than left uncovered.

## Scope reviewed

- `clients/rust/maho-decode/src/lib.rs`: 740 lines (FFmpeg HEVC/H.264 decoder, NALU parsing, NV12 conversion).
- `clients/rust/maho-decode/src/vt/mod.rs`: 600 lines (VideoToolbox decoder, CVPixelBuffer plane copy, async callback).
- `clients/rust/maho-decode/src/vt/ffi.rs`: 250 lines (raw CoreMedia/VideoToolbox bindings).
- `clients/rust/maho-render/src/audio.rs`: 623 lines (bounded PCM queue, cpal output stream, iOS session activation).
- `clients/rust/maho-render/src/*.rs`: remaining 414 lines.

Static review. Baseline accepted as given (`cargo check --workspace --all-targets` exit 0).

## Findings

### [P1] `copy_nv12` indexes chroma rows past the end of a tightly-packed FFmpeg plane on odd heights

- **Location**: `clients/rust/maho-decode/src/lib.rs:437`
- **Evidence**:
```rust
        let mut uv_plane = vec![0_u8; width * height.div_ceil(2)];
        for row in 0..height {
            let source = &frame.data(0)[row * y_stride..row * y_stride + width];
            y_plane[row * width..(row + 1) * width].copy_from_slice(source);
        }
        for row in 0..height.div_ceil(2) {
            let source = &frame.data(1)[row * uv_stride..row * uv_stride + width];
            uv_plane[row * width..(row + 1) * width].copy_from_slice(source);
        }
```
- **Impact**: The Y loop and the UV loop slice `frame.data(n)` with no check that `row * stride + width` stays inside the plane. Unlike the VideoToolbox path (`vt/mod.rs:123`), there is **no** `y_stride < width || uv_stride < width` guard here, and no check that `data(1).len() >= uv_stride * height.div_ceil(2)`. FFmpeg guarantees neither: for a frame whose allocated chroma height is `height / 2` (odd `height`), or a decoder that returns a plane buffer sized exactly to its own alignment, the final iteration slices out of bounds and panics inside the decode loop, killing the client's decode thread on a peer-supplied stream. The host encoders reject odd width (`encode_windows.rs:95`) but the client decodes whatever the peer sends.
- **Fix**: Mirror the VideoToolbox guard: after reading strides, return `DecodeError::UnsupportedFrame` when `y_stride < width`, `uv_stride < width`, `frame.data(0).len() < y_stride * height`, or `frame.data(1).len() < uv_stride * height.div_ceil(2)`. Then keep the copy loops as-is.
- **Confidence**: high

### [P2] `push_samples` stages the whole peer-supplied batch before applying the queue bound

- **Location**: `clients/rust/maho-render/src/audio.rs:67`
- **Evidence**:
```rust
        let mut pending = VecDeque::with_capacity(AUDIO_QUEUE_CAPACITY);
        let mut input = samples.into_iter();
        while let Some(left) = input.next() {
            let right = input.next().ok_or(AudioError::MisalignedSamples)?;
            if !left.is_finite() || !right.is_finite() {
                return Err(AudioError::NonFinitePcm);
            }
            if pending.len() == AUDIO_QUEUE_CAPACITY {
                pending.drain(..AUDIO_CHANNELS as usize);
            }
            pending.extend([left, right]);
        }
```
- **Impact**: The staging deque is correctly capped at `AUDIO_QUEUE_CAPACITY`, so memory is bounded — but the drain is `O(1)` amortized only because `VecDeque::drain(..2)` is cheap; the loop still walks every sample the peer sent. A single large `MediaEvent::Audio` payload therefore costs CPU proportional to its size on the ingest path while holding no lock, and the `drain` per sample-pair after the cap is reached makes the tail of a large batch strictly wasted work: all of it is discarded by `append_validated`'s `skip`. The validation-before-commit intent is right; the cost model is not.
- **Fix**: Compute the retained tail first (the last `AUDIO_QUEUE_CAPACITY` samples) when the iterator reports a known length, validate only those plus enough of the prefix to detect misalignment, and skip the per-pair `drain`.
- **Confidence**: medium

### [P2] `drain_into` reports consumed samples but silently zero-fills the remainder with no underrun signal

- **Location**: `clients/rust/maho-render/src/audio.rs:131`
- **Evidence**:
```rust
        for destination in output {
            if let Some(sample) = state.samples.pop_front() {
                *destination = sample * gain;
                consumed += 1;
            } else {
                *destination = 0.0;
            }
        }
        Ok(consumed)
```
- **Impact**: Underrun is representable only as `consumed < output.len()`, which the caller must compare itself. A partially-filled buffer produces an audible click (abrupt jump to silence mid-buffer) rather than a ramp, and there is no counter for how often it happens, so chronic underrun is invisible in telemetry while the user hears crackling.
- **Fix**: Count underruns in the queue state and expose them in `AudioOutputStatus`; apply a short linear ramp to zero over the first few samples of the underrun region instead of a hard cut.
- **Confidence**: medium

### [P3] `HevcDecoder::drop` ends with a no-op statement whose intent is a comment

- **Location**: `clients/rust/maho-decode/src/lib.rs:409`
- **Evidence**:
```rust
                let context = self.decoder.as_mut_ptr();
                (*context).extradata = ptr::null_mut();
                (*context).extradata_size = 0;
            }
            let _ = self.extradata.len();
```
- **Impact**: `let _ = self.extradata.len();` does nothing at runtime; it appears to be an attempt to document that `extradata` outlives the context detach. A reader cannot tell whether it is load-bearing. The actual invariant (clear the borrowed pointer before the Vec drops) is already satisfied by the two lines above it.
- **Fix**: Delete the statement and keep the existing comment.
- **Confidence**: high

## Non-findings checked

- `vt/mod.rs:80-97` locks the CVPixelBuffer with `CVPixelBufferLockBaseAddress` and releases it through an `UnlockGuard` `Drop`, so the unlock is exception-safe on every early return.
- `vt/mod.rs:123` explicitly rejects `y_base.is_null() || uv_base.is_null() || y_stride < width || uv_stride < width` before any `from_raw_parts` — the guard the FFmpeg path is missing.
- `vt/mod.rs:118-121` reads both plane base addresses and strides while the lock is held, and `width == 0 || height == 0` is rejected at :114.
- `vt/mod.rs:375-392` `Drop` waits for asynchronous frames, invalidates the session, releases the session and format description, and reclaims the `Arc<SharedState>` raw pointer exactly once, nulling each field after release — no double-free, no leak of the callback context.
- `vt/mod.rs:494` the C callback null-checks `decompression_output_refcon` before dereferencing and does no unwinding work itself.
- `vt/mod.rs:220/237/241/290/315/331` every `CFRelease` is paired with the create call above it on both the success and error paths.
- `lib.rs:398-407` `Drop` unrefs the hardware device buffer and detaches `extradata` from the `AVCodecContext` before the owning `Vec` drops, preventing FFmpeg from freeing Rust-owned memory.
- `lib.rs:359-393` the swscale context is rebuilt whenever the source format/width/height changes, so a mid-stream resolution change does not reuse a stale scaler.
- `lib.rs:47-100` NALU parsing borrows payload slices (`Nalu<'_>`) instead of copying, and the tests at :608-622 assert pointer identity to seal that.
- `audio.rs:82-93` `append_validated` computes `skip`/`overflow` with `saturating_sub` and drains before extending, so the queue length can never exceed `AUDIO_QUEUE_CAPACITY`.
- `audio.rs:49` `push_pcm_bytes` validates 8-byte stereo-f32 alignment before conversion, and non-finite samples are rejected at :73 before anything is committed.
- `audio.rs:116-129` volume is range-checked and mute is applied as a gain inside the locked region, so the render callback cannot observe a torn volume update.
