# iOS App/Runtime Executor Handoff

## 1. Owned, Created, and Modified Files
- `clients/rust/Cargo.toml` (Added `"ios-shell"` to workspace `members`)
- `clients/rust/erd-app/Cargo.toml` (Added `ffmpeg = ["erd-decode/ffmpeg"]` feature with `erd-decode` defaulting to `default-features = false`, preventing transitive FFmpeg activation on mobile while preserving desktop defaults)
- `clients/rust/erd-app/src/pairing.rs` (Refactored `PairingStore` with pluggable `StoreBackend`: `File`, `Keychain`, and `Ephemeral`. `open_default()` on iOS is genuinely Keychain-backed so `ClientSession::pair_with_pin` saves directly to Keychain without writing plaintext JSON to Application Support. `new_ephemeral()` enables in-memory testing on all targets without disk I/O. Desktop file-backed persistence semantics and all existing tests are preserved)
- `clients/rust/erd-mobile/Cargo.toml` (Disabled transitive default features on `erd-app` and `erd-decode`, added target-conditioned `core-foundation` dependency)
- `clients/rust/erd-mobile/src/storage.rs` (Added `IosKeychainStorage` with direct Apple Security framework FFI for iOS/macOS, added `find_by_host` lookup to `MobilePairingStore`, added unit tests for host matching)
- `clients/rust/erd-mobile/src/lib.rs` (Re-exported `IosKeychainStorage` under `target_os = "ios"` / `target_os = "macos"`)
- `clients/rust/erd-render/src/audio.rs` (Added `activate_ios_audio_session` to activate `AVAudioSessionCategoryPlayback` on iOS prior to CPAL stream initialization)
- `clients/rust/ios-shell/Cargo.toml` (New Tauri v2 iOS package `erd-ios`, lib `erd_ios_lib`, target dependencies selecting `ios-videotoolbox` without FFmpeg)
- `clients/rust/ios-shell/build.rs` (Tauri build script)
- `clients/rust/ios-shell/tauri.conf.json` (Tauri v2 configuration with `com.eclipticrd.ios` identifier, `./ui` frontend, and iOS bundle Info.plist linking)
- `clients/rust/ios-shell/Info.plist` (Local-network usage description, Bonjour services `_erd._tcp`/`_erd._udp`, and portrait/landscape orientation declarations)
- `clients/rust/ios-shell/capabilities/default.json` (Default Tauri v2 window and core capabilities)
- `clients/rust/ios-shell/src/main.rs` (CLI entry point delegating to `erd_ios_lib::run()`)
- `clients/rust/ios-shell/src/lib.rs` (Tauri mobile entry point, `AppState` registration, command routing)
- `clients/rust/ios-shell/src/frame.rs` (Repacking native-stride NV12 frames to wire contract: 16-byte LE header, contiguous Y and UV planes)
- `clients/rust/ios-shell/src/qa.rs` (Sandbox directory resolution, debug-only `Documents/erd-device-qa.json` provisioning import into Keychain-backed `PairingStore::open_default()` with immediate deletion)
- `clients/rust/ios-shell/src/state.rs` (Session state machine, dedicated audio worker owning `CpalAudioOutput` stream and reporting DAC callback consumption, non-blocking async operations via `lifecycle_lock`, trackpad displacement scaling to video pixels, touch cancellation recovery, generation guarding, stream telemetry state transition to `error`, and strict presented sequence validation)
- `clients/rust/ios-shell/src/commands.rs` (Implementation of all 10 commands conforming to UI contract, with non-blocking async `disconnect` and poisoned mutex error propagation)
- `clients/rust/ios-shell/src/tests.rs` (Deterministic unit tests for frame repacking, touch normalization, trackpad normalized drag pixel scaling, touch cancellation with non-finite coordinates, presentation sequence validation, poisoned mutex error propagation, mode switching, key validation, and stats serialization)

## 2. Target Compilation & Verification Results

Both physical iOS target checks were executed directly and passed with zero errors and zero warnings:

1. Library check for `aarch64-apple-ios`:
```bash
ulimit -n 8192
RUSTC_WRAPPER= CARGO_BUILD_JOBS=4 CARGO_TERM_COLOR=never \
  IPHONEOS_DEPLOYMENT_TARGET=16.0 \
  CARGO_TARGET_DIR=/Volumes/T9-Mac/project/EclipticRD-Rewrite/clients/rust/target-ios-device \
  cargo check --manifest-path clients/rust/Cargo.toml \
  -p erd-ios --lib --target aarch64-apple-ios
```
Result:
```
    Checking erd-ios v0.1.0 (/Volumes/T9-Mac/project/EclipticRD-Rewrite/clients/rust/ios-shell)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.58s
```

2. Test type-check for `aarch64-apple-ios`:
```bash
RUSTC_WRAPPER= CARGO_BUILD_JOBS=4 CARGO_TERM_COLOR=never \
  IPHONEOS_DEPLOYMENT_TARGET=16.0 \
  CARGO_TARGET_DIR=/Volumes/T9-Mac/project/EclipticRD-Rewrite/clients/rust/target-ios-device \
  cargo check --manifest-path clients/rust/Cargo.toml \
  -p erd-ios --tests --target aarch64-apple-ios
```
Result:
```
    Checking erd-ios v0.1.0 (/Volumes/T9-Mac/project/EclipticRD-Rewrite/clients/rust/ios-shell)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.54s
```

3. Unimplemented / Blocked items:
None. All 10 command contract items, dedicated audio worker with CPAL playback tracking, Keychain-backed pairing persistence, trackpad displacement scaling, cancellation validation, generation isolation, non-blocking async lifecycle, and telemetry error state transitions are fully implemented and compile cleanly.

## 3. App Command Contract Conformance
- `connect({host, pin})`: Serialized via `lifecycle_lock` and offloaded via `tokio::task::spawn_blocking`. Validates host (non-empty) and PIN (8 ASCII digits). Connects using Keychain credentials or `pair_with_pin` saving directly to Keychain. Spawns TCP runtime, dedicated audio worker, and UDP media receiver with monotonic generation guards. Returns safe `SessionStats` only after Ready.
- `disconnect()`: Serialized via `lifecycle_lock` and offloaded via `tokio::task::spawn_blocking`. Flushes pending motion, sends `InputEventType::Reset`, stops TCP runtime, cancels worker loops, completely joins all worker threads (propagating join errors), clears audio queue, drops cached video frames, resets state to `idle`, and returns only after teardown completes.
- `stats()`: Returns `{state, host, frames_received, frames_decoded, audio_packets_received, audio_samples_played, width, height, last_error}` with snake_case fields. State transitions to `"error"` upon media receiver or Persistent decode failure.
- `poll_frame()`: Binary `tauri::ipc::Response`. Returns empty byte buffer if no new frame has been decoded since last poll. When a new frame is present, returns 16-byte LE header (`width: u32`, `height: u32`, `sequence: u64`) followed by contiguous NV12 Y (`width*height`) and UV (`width*ceil(height/2)`) planes with native stride repacked.
- `touch({event:{id, x, y, phase}})`: Coordinates in Began/Moved are validated for finiteness; Cancelled/Ended coordinates are forwarded to the handler even if non-finite so that held touches are safely released. Direct touch retains inverted-Y wire normalization; trackpad relative movement (`RelativeMove`) scales normalized displacements by remote video width/height so host injectors receive discrete pixel movements.
- `set_touch_mode({mode})`: `"direct" | "trackpad"`. Dispatches any pending touch release returned by `TouchGestureHandler::set_mode` before accepting new touches in the new mode.
- `send_key({keyCode, down, modifiers})`: Validates non-zero `keyCode` and valid modifier bitflags, sending physical wire `InputEvent`.
- `set_muted({muted})`: Updates gain/mute on the shared `AudioQueue`.
- `presented({sequence})`: Verifies `sequence > 0 && sequence <= latest_decoded_sequence`. Rejects un-decoded future sequence IDs and logs first verified presentation.
- `startup()`: Returns `{host, auto_connect}`. In debug builds, inspects `Documents/erd-device-qa.json` in the sandbox; if present, imports pairing record into iOS Keychain, deletes the file, and returns `{host: Some(...), auto_connect: true}` without exposing pairing keys or PINs.

## 4. Portability, Keychain, Audio, and Resource Invariants
- Keychain security: `PairingStore` on iOS uses native Apple `Security.framework` (`SecItemAdd`, `SecItemCopyMatching`, `SecItemUpdate`, `SecItemDelete`) with `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`. Pairing records persist across app restarts without writing keys to `localStorage` or unencrypted disk files.
- Desktop persistence preservation: Desktop `PairingStore` JSON file storage logic is completely unchanged on macOS, Windows, and Linux.
- Dedicated Audio Worker: `CpalAudioOutput` is owned exclusively on a dedicated thread which activates `AVAudioSessionCategoryPlayback`, handles initialization errors synchronously without silent fallback, keeps the stream alive until cancellation, and drops/joins cleanly.
- Audio consumption: Consumed sample count is reported directly by the `CpalAudioOutput` render callback via `AudioOutputEvent::Callback`, updating `audio_samples_played` only from actual DAC consumption.
- Single active session & generation guarding: Connection lifecycle is protected by `lifecycle_lock`. Background workers check generation on every write. Old workers are completely joined before new sessions commence. Join errors and poisoned mutexes are never silently discarded.
- Bounded buffering: The video frame mailbox holds at most one decoded NV12 frame at any time, avoiding unbounded buffer growth or memory bloat on mobile.

## 5. Physical iOS Linker Integration (`native-static-libs` & Xcode Project)

### Diagnosed Linker Requirements
Executed `cargo rustc` to extract exact required system SDKs from the Rust static library:
```bash
ulimit -n 8192
RUSTC_WRAPPER= CARGO_BUILD_JOBS=4 CARGO_TERM_COLOR=never \
  IPHONEOS_DEPLOYMENT_TARGET=16.0 \
  CARGO_TARGET_DIR=/Volumes/T9-Mac/project/EclipticRD-Rewrite/clients/rust/target-ios-device \
  cargo rustc --manifest-path clients/rust/Cargo.toml \
  -p erd-ios --lib --target aarch64-apple-ios -- --print native-static-libs
```
Reported native static libraries and frameworks:
`-lobjc -framework AVFoundation -framework AudioToolbox -framework CoreAudio -framework Security -framework CoreFoundation -framework CoreVideo -framework CoreMedia -framework VideoToolbox -lclang_rt.ios -framework WebKit -framework UIKit -framework UserNotifications -framework CoreData -framework CoreLocation -framework CoreText -framework CoreImage -framework CloudKit -framework QuartzCore -framework CoreGraphics -framework Foundation -liconv`

### System Frameworks Added to `project.yml` and `tauri.conf.json`:
- **VideoToolbox Pipeline**: `VideoToolbox.framework`, `CoreMedia.framework`, `CoreVideo.framework` (required by `erd-decode` native hardware decoding).
- **CoreAudio / CPAL Audio Pipeline**: `AudioToolbox.framework`, `AVFoundation.framework`, `CoreAudio.framework` (required by `erd-render` / `cpal`).
- **Apple UI / Runtime Services**: `CloudKit.framework`, `CoreData.framework`, `CoreFoundation.framework`, `CoreImage.framework`, `CoreLocation.framework`, `CoreText.framework`, `Foundation.framework`, `UserNotifications.framework`.
- **C Standard Library Extensions**: `libiconv.tbd` (and `OTHER_LDFLAGS: $(inherited) -liconv`).

### Xcode Project Regeneration
Executed `xcodegen generate` in `clients/rust/ios-shell/gen/apple`:
```bash
cd clients/rust/ios-shell/gen/apple && xcodegen generate
```
Result:
```
⚙️  Generating plists...
⚙️  Generating project...
⚙️  Writing project...
Created project at /Volumes/T9-Mac/project/EclipticRD-Rewrite/clients/rust/ios-shell/gen/apple/erd-ios.xcodeproj
```
Verified that `erd-ios.xcodeproj/project.pbxproj` contains all required frameworks and `libiconv.tbd` in the `Frameworks` build phase.
