# Mobile Review Implementation Handoff (Phase 2 - Scoped Fixes)

## Summary of Touched Files
- `clients/rust/erd-mobile/src/bridge.rs`: C-ABI entry points, pointer validation, handle safety contracts, removal of fake connection state.
- `clients/rust/erd-mobile/src/android.rs`: Explicit `BackendUnavailable` typed error, preserved configuration checks, elimination of fake decode/written byte counters.
- `clients/rust/erd-mobile/src/ios.rs`: Explicit `BackendUnavailable` typed error, updated audio engine `start()` signature, elimination of fake render/played counters.
- `clients/rust/erd-mobile/src/touch.rs`: Single pointer ownership for DirectTouch and TrackpadRelative, coordinate/viewport sanitization, overflow-safe pan/zoom, release-on-mode-switch.
- `clients/rust/erd-mobile/src/power.rs`: Overflow-safe u32 quotient/remainder arithmetic for `ThermalState::Fair`, cap clamping preventing bitrate inflation above default or battery-saver caps.
- `clients/rust/erd-mobile/tests/mobile_regressions.rs`: Granular, independent regression test suite (32 tests) covering all review findings without tautological assertions.

## Public API Changes
1. `erd_mobile::ErdMobileStatus`:
   - Added `BackendUnavailable = 5` while preserving existing discriminants (`Ok = 0`, `InvalidParam = 1`, `SessionNotFound = 2`, `NetworkError = 3`, `InternalError = 4`).
2. `erd_mobile::MobileSessionHandle`:
   - Removed `pub is_connected: bool` to eliminate misleading state. Session creation is strictly local allocation.
3. `erd_mobile::android::AndroidMediaError`:
   - Added `BackendUnavailable` variant returned by `decode_access_unit` and `write_pcm` when non-empty data is supplied.
4. `erd_mobile::ios::IosMediaError`:
   - Added `BackendUnavailable` variant returned by `render_frame` and `start`.
5. `erd_mobile::ios::IosAudioEnginePlayer`:
   - Changed `start(&mut self) -> Result<(), IosMediaError>` (previously returned `()`) to explicitly signal that native AVAudioEngine is unavailable.
6. `erd_mobile::touch::TouchGestureHandler`:
   - Changed `set_mode(&mut self, mode: TouchMode) -> Option<InputEvent>` (previously returned `()`). Switching away from an active DirectTouch drag returns a synthetic `LeftMouseUp` release event; switching to the identical mode preserves the active gesture and returns `None`.

## Invariants Preserved
- **Wire Coordinates**: Established inverted-Y coordinate normalization (`(norm_x, (1.0 - norm_y).clamp(0.0, 1.0))`) is strictly preserved.
- **Pointer Ownership**: Exactly one primary touch owns the mouse lifecycle in `DirectTouch`. Secondary touches and unknown or duplicate `Ended`/`Cancelled` events cannot emit mouse button events or steal the pointer.
- **Relative Baseline**: In `TrackpadRelative`, the first active touch establishes the baseline. Secondary touches cannot hijack or corrupt the baseline.
- **Cancel Recovery**: Cancelling an active `DirectTouch` uses the last successfully emitted host coordinates even if incoming platform coordinates are non-finite or the viewport has been mutated.
- **Viewport Boundary**: Dimensions, zoom factors, and offsets are validated for finiteness and validity before mutating state. Finite arithmetic subtraction overflow is rejected.
- **Thermal Power Policy**: In `ThermalState::Fair`, target bitrate scales to 80% via overflow-safe u32 quotient/remainder arithmetic: `(target_bitrate / 5) * 4 + ((target_bitrate % 5) * 4) / 5`. It is capped at `target_bitrate` so low configured default or battery-saver caps are never raised, and `u32::MAX` computes deterministically to `3435973836` without integer overflow or narrowing casts.
- **FFI Safety Contracts**: Added explicit `# Safety` docstrings to `erd_mobile_create`, `erd_mobile_destroy`, and `erd_mobile_send_touch`. Output handle slots are null-initialized before validating other parameters.

## Remaining Product-Level Blockers
- **F1 (Remaining - Mobile Session Networking)**: `erd_mobile_create` only allocates a local session struct. Full streaming socket connection (TCP 19730) and protocol handshake remain unimplemented.
- **F2 (Remaining - Hardware Media Pipelines)**: Real Android NDK/MediaCodec hardware decoders, AudioTrack JNI bridges, iOS VideoToolbox decompressors, and AVAudioEngine graphs remain unimplemented. Stubs now explicitly report `BackendUnavailable`.
- **F3 (App Packaging & Build Harness)**: Android Gradle workspace, iOS Xcode workspace, `AndroidManifest.xml`, `Info.plist`, native platform viewports, and APK/IPA packaging remain unimplemented.
- **F4 (Desktop Dependency Leakage)**: Workspace and crate dependencies still leak desktop-oriented crates and ffmpeg paths into the mobile build graph.
- **F8 (Secure Storage, Lifecycle, and IME)**: Native OS keychain/keystore backends, mobile lifecycle event wiring, and IME composition bridges remain stubs or mock implementations.

**Notice**: Real mobile networking, hardware media pipelines, and APK/IPA packaging remain unimplemented; these fixes correct contracts, eliminate fake successes, and resolve regressions, but do not constitute a working end-to-end mobile application.
