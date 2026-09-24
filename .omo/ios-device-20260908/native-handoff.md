# Native VideoToolbox iOS Decoder Handoff

## 1. Owned and Modified Files
- `clients/rust/erd-decode/Cargo.toml` (Added standalone `ios-videotoolbox = []` feature without FFmpeg dependencies)
- `clients/rust/erd-decode/src/lib.rs` (Exported `vt::HevcDecoder` when `ios-videotoolbox` is active on Apple targets, added `DecodeError::VideoToolbox*` variants, updated `HardwareAcceleration`, preserved disabled backend contract, added regression tests for H.264 parameter sets and codec auto-detection)
- `clients/rust/erd-decode/src/vt/ffi.rs` (Audited raw FFI module declaring VideoToolbox, CoreMedia, CoreVideo, and CoreFoundation APIs with exact signatures matched to Xcode iPhoneOS SDK headers)
- `clients/rust/erd-decode/src/vt/mod.rs` (Native iOS/Apple VideoToolbox `HevcDecoder` implementation, NV12 stride-aware conversion, thread synchronization, teardown guarantees, and native regression tests)

## 2. Verification Commands for the Lead
The physical iOS-target check was executed and verified clean (zero warnings, exit code 0):
```bash
ulimit -n 8192
RUSTC_WRAPPER= CARGO_BUILD_JOBS=4 CARGO_TERM_COLOR=never \
  IPHONEOS_DEPLOYMENT_TARGET=16.0 \
  CARGO_TARGET_DIR=/Volumes/T9-Mac/project/EclipticRD-Rewrite/clients/rust/target-ios-device \
  cargo check --manifest-path clients/rust/Cargo.toml \
  -p erd-decode --no-default-features --features ios-videotoolbox \
  --target aarch64-apple-ios
```
Execution output:
```
Checking erd-decode v0.1.0 (/Volumes/T9-Mac/project/EclipticRD-Rewrite/clients/rust/erd-decode)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.63s
```

Additional cross-target verification commands:

1. Desktop regression test suite (FFmpeg backend and pure behavior seams):
```bash
cargo test --manifest-path clients/rust/Cargo.toml -p erd-decode
```

2. macOS/Apple target native VideoToolbox tests:
```bash
cargo test --manifest-path clients/rust/Cargo.toml -p erd-decode --no-default-features --features ios-videotoolbox
```

3. iOS target compile check:
```bash
cargo check --manifest-path clients/rust/Cargo.toml -p erd-decode --target aarch64-apple-ios --no-default-features --features ios-videotoolbox
```

4. Disabled backend check (ensuring no compilation breaks when all backends are disabled):
```bash
cargo check --manifest-path clients/rust/Cargo.toml -p erd-decode --no-default-features
```

## 3. Decoder Contract Conformance
- `ios-videotoolbox` feature: Standalone feature with no transitive FFmpeg dependency.
- Public API surface on iOS:
  - `HevcDecoder::new(extradata: &[u8]) -> Result<Self, DecodeError>`
  - `HevcDecoder::new_h264(extradata: &[u8]) -> Result<Self, DecodeError>`
  - `HevcDecoder::from_keyframe(keyframe: &[u8]) -> Result<Self, DecodeError>`
  - `HevcDecoder::from_keyframe_auto(keyframe: &[u8]) -> Result<(CodecKind, Self), DecodeError>`
  - `decoder.decode(&mut self, access_unit: &[u8], timestamp_ms: i64) -> Result<Vec<Nv12Frame>, DecodeError>`
  - `decoder.flush(&mut self) -> Result<Vec<Nv12Frame>, DecodeError>`
  - `decoder.acceleration(&self) -> HardwareAcceleration` (reports `HardwareAcceleration::VideoToolbox`)
- Codec support: Both HEVC (VPS 32, SPS 33, PPS 34) and H.264 (SPS 7, PPS 8) parameter set parsing and format description generation via `CMVideoFormatDescriptionCreateFromHEVCParameterSets` and `CMVideoFormatDescriptionCreateFromH264ParameterSets`.
- Wire framing: Directly processes 4-byte big-endian length-prefixed NALUs (AVCC/HVCC) without Annex-B transcode, setting `NALUnitHeaderLength = 4` in CoreMedia.
- Actual pixel output: Decodes real pixels into `CVPixelBufferRef` requesting `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange`, reads plane 0 (Y) and plane 1 (UV) with stride awareness, and repacks into contiguous NV12 `Nv12Frame` with `y_stride = width` and `uv_stride = width`.
- Error propagation: Typed `DecodeError` variants (`TruncatedNalu`, `EmptyNalu`, `MissingParameterSets`, `UnsupportedFrame`, `VideoToolboxInit`, `VideoToolbox`) with native status codes preserved.

## 4. Resource Invariants, Synchronization, and Safety
- Thread safety and Send proof: `VTDecompressionSession` is thread-safe per Apple SDK documentation. `HevcDecoder` encapsulates handles and an `Arc<SharedState>` with `Mutex` synchronization. The public interface requires `&mut self` on mutating operations.
- Lifetime of context in async callbacks: `VTDecompressionOutputCallbackRecord` refcon is created via `Arc::into_raw`. In `drop()`, `VTDecompressionSessionWaitForAsynchronousFrames` drains pending work, `VTDecompressionSessionInvalidate` tears down callback invocation, system handles are released via `CFRelease`, and `Arc::from_raw` safely reclaims the context.
- Buffer locking: `CVPixelBufferLockBaseAddress` with `kCVPixelBufferLock_ReadOnly` is guarded by an RAII `UnlockGuard` ensuring `CVPixelBufferUnlockBaseAddress` executes on all return paths.
- Asynchronous callback gate: Accounts for async hardware execution via `VTDecompressionSessionWaitForAsynchronousFrames` whenever `kVTDecodeInfo_Asynchronous` is flagged or frames are in flight.
- Non-Apple & desktop preservation: Desktop FFmpeg backend remains default and unmodified. Existing disabled backend stub includes full public method stubs returning `DecodeError::FfmpegDisabled`. Zero modifications to other crates or lockfile.
