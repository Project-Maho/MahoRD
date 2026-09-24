# Lane: tests-diff

## Scope reviewed
- `clients/rust/ios-shell/tests/lifecycle.rs`: 658 lines
- `clients/rust/maho-app/src/bin/maho_client/continuity_codec_tests.rs`: 155 lines
- `clients/rust/maho-app/src/bin/maho_client/receiver_telemetry_tests.rs`: 33 lines
- `clients/rust/maho-app/src/mcp_dispatch_contract_tests.rs`: 119 lines
- `clients/rust/maho-app/src/mcp_tests.rs`: 302 lines
- `clients/rust/maho-app/src/mcp_windows_tests.rs`: 3 lines
- `clients/rust/maho-app/src/receiver_trace_tests.rs`: 125 lines
- `clients/rust/maho-app/src/tcp_write_tests.rs`: 293 lines
- `clients/rust/maho-app/tests/agent_control_e2e.rs`: 273 lines
- `clients/rust/maho-app/tests/cli_mcp_contract.rs`: 267 lines
- `clients/rust/maho-app/tests/cli_receiver_telemetry.rs`: 281 lines
- `clients/rust/maho-app/tests/client_copy_cost.rs`: 241 lines
- `clients/rust/maho-app/tests/core_semantics.rs`: 136 lines
- `clients/rust/maho-app/tests/media_reassembly.rs`: 244 lines
- `clients/rust/maho-app/tests/receiver_telemetry.rs`: 566 lines
- `clients/rust/maho-app/tests/session_mock.rs`: 701 lines
- `clients/rust/maho-app/tests/windows-stdio/src/observed.rs`: 121 lines
- `clients/rust/maho-host/src/native_stall_tests.rs`: 72 lines
- `clients/rust/maho-host/src/sender_packetization_tests.rs`: 327 lines
- `clients/rust/maho-host/src/sender_trace_tests.rs`: 156 lines
- `clients/rust/maho-host/tests/linux_audio_selection.rs`: 193 lines
- `clients/rust/maho-host/tests/pairing_isolation.rs`: 301 lines
- `clients/rust/maho-mobile/tests/mobile_regressions.rs`: 1094 lines
- `clients/rust/maho-net/src/nonblocking_write_tests.rs`: 218 lines
- `clients/rust/maho-net/tests/discovery_metadata.rs`: 384 lines
- `clients/rust/maho-net/tests/discovery_scoped_ipv6.rs`: 276 lines
- `clients/rust/maho-proto/tests/framing_burst.rs`: 106 lines
- `clients/rust/maho-proto/tests/protocol_v3.rs`: 866 lines
- `clients/rust/maho-proto/tests/timestamp_stats.rs`: 104 lines
- `clients/rust/maho-render/tests/audio_api.rs`: 89 lines
- `clients/rust/maho-render/tests/audio_queue.rs`: 159 lines
- `clients/rust/tauri-shell/src-tauri/src/desktop_integration_tests.rs`: 544 lines
- `clients/rust/tauri-shell/src-tauri/src/discovery_tests.rs`: 329 lines
- `clients/rust/tauri-shell/src-tauri/src/mailbox_tests.rs`: 260 lines
- `clients/rust/tauri-shell/src-tauri/src/pairing_tests.rs`: 775 lines
- `clients/rust/tauri-shell/src-tauri/src/recovery_tests.rs`: 256 lines
- `clients/rust/tauri-shell/src/features/library/HostGrid.test.tsx`: 341 lines
- `clients/rust/tauri-shell/src/features/session/SessionCanvas.test.tsx`: 402 lines
- `clients/rust/tauri-shell/src/features/session/SessionSettingsPanel.test.tsx`: 243 lines
- `clients/rust/tauri-shell/src/features/session/SessionView.test.tsx`: 655 lines
- `clients/rust/tauri-shell/src/features/session/useRemoteInput.test.ts`: 618 lines
- `clients/rust/tauri-shell/src/lib/connection.test.ts`: 415 lines
- `clients/rust/tauri-shell/src/lib/ipc.test.ts`: 404 lines
- `clients/rust/tauri-shell/src/lib/library.test.ts`: 284 lines
- `clients/rust/tauri-shell/src/lib/overlay.test.ts`: 235 lines
- `clients/rust/tauri-shell/src/lib/renderer.test.ts`: 311 lines
- Working tree uncommitted diff (13 files, +196/-62):
  - `clients/rust/maho-host/Cargo.toml`: 89 lines
  - `clients/rust/maho-host/src/capture_macos.rs`: 749 lines
  - `clients/rust/maho-host/src/capture_windows.rs`: 449 lines
  - `clients/rust/maho-host/src/lib.rs`: 56 lines
  - `clients/rust/maho-host/src/main.rs`: 301 lines
  - `clients/rust/maho-host/src/session.rs`: 5280 lines
  - `clients/rust/maho-host/src/windows_logic.rs`: 779 lines
  - `clients/rust/maho-host/tests/pairing_isolation.rs`: 301 lines
  - `clients/rust/tauri-shell/index.html`: 12 lines
  - `clients/rust/tauri-shell/src/app/App.tsx`: 83 lines
  - `clients/rust/tauri-shell/src/features/session/SessionCanvas.tsx`: 226 lines
  - `clients/rust/tauri-shell/src/features/session/SessionView.tsx`: 443 lines
  - `clients/rust/tauri-shell/src/styles/globals.css`: 93 lines

## Findings

### [P1] Windows input injection and cursor query mix logical and physical metrics, breaking scaled displays
- **Location**: `clients/rust/maho-host/src/session.rs:2277` (and secondary site `clients/rust/maho-host/src/session.rs:1337`)
- **Evidence**:
```rust
        #[cfg(target_os = "windows")]
        let mut input = {
            let target = TargetDisplay {
                x: self.config.display.desktop_x,
                y: self.config.display.desktop_y,
                width: self.config.display.pixel_width,
                height: self.config.display.pixel_height,
            };
            WindowsInputInjector::new(Some(target)).map_err(SessionError::Io)?
        };
```
- **Impact**:
In `resolve_output_metadata`, `desktop_x` and `desktop_y` originate from `DXGI_OUTPUT_DESC.DesktopCoordinates`, which are expressed in Windows desktop coordinates (spanning `logical_width` by `logical_height`, e.g., 3072x1280 on a 3840x1600 125% DPI display). Similarly, `VirtualDesktop` queried via `GetSystemMetrics(SM_CXVIRTUALSCREEN)` is expressed in desktop space.
However, `TargetDisplay` is constructed with `pixel_width` and `pixel_height` (physical raster dimensions, e.g. 3840x1600). In `normalize_absolute_pointer`, `desktop_x = target.x + (normalized_x * target.width) - desktop.x`. Because `target.width` is physical (3840) while `target.x` and `desktop` are logical (3072), absolute mouse clicks overshoot by the DPI scaling factor: clicks beyond 80% screen width spill into adjacent monitors, and inputs across the right 20% of the display are entirely unreachable.
Additionally, in `query_cursor` (lines 1337-1347), `ci.ptScreenPos` from `GetCursorInfo` is in desktop coordinates. Subtracting `origin_x` yields a value in `[0, logical_width]`. Dividing this by `mon_w` (`pixel_width`) clamps the reported normalized cursor coordinates to `1.0 / DPI_scale` (e.g. max 0.8 at 125% DPI). When the mouse moves, DXGI provides physical frame offsets causing the cursor to jump to 1.0; when motion pauses, `query_cursor` teleports the cursor backwards to 0.8, causing erratic cursor jumping and truncation under any DPI scaling.
- **Fix**:
Pass `logical_width` and `logical_height` to `TargetDisplay` in `session.rs` line 2280:
```rust
let target = TargetDisplay {
    x: self.config.display.desktop_x,
    y: self.config.display.desktop_y,
    width: self.config.display.logical_width,
    height: self.config.display.logical_height,
};
```
In `WindowsMediaSource`, bind `mon_w` and `mon_h` to `logical_width` and `logical_height` for the `query_cursor` calculation:
```rust
let mon_w = self.config.display.logical_width;
let mon_h = self.config.display.logical_height;
```
- **Confidence**: high

### [P1] Working tree diff declares untracked modules in lib.rs
- **Location**: `clients/rust/maho-host/src/lib.rs:27`
- **Evidence**:
```rust
#[cfg(target_os = "windows")]
pub mod service_windows;
pub mod session;
pub mod windows_logic;
pub mod windows_session;
```
- **Impact**:
The uncommitted diff modifies `clients/rust/maho-host/src/lib.rs` to expose `pub mod service_windows;` and `pub mod windows_session;`. Both backing source files (`service_windows.rs` and `windows_session.rs`) are currently untracked (`??` in `git status`). If the working tree diff (the 13 modified files) is committed as-is without these untracked files, any clean clone or CI build will immediately fail to compile with `error[E0583]: file not found for module service_windows` and `windows_session`.
The working-tree diff is incomplete and incoherent to commit on its own.
- **Fix**:
Do not declare `pub mod service_windows;` or `pub mod windows_session;` in `lib.rs` until the underlying implementations are complete, tested, and staged together in git.
- **Confidence**: high

### [P2] Tautological assertion in `audio_api.rs` asserts an enum variant matches itself
- **Location**: `clients/rust/maho-render/tests/audio_api.rs:85`
- **Evidence**:
```rust
    assert!(matches!(
        AudioError::UnsupportedSampleFormat(cpal::SampleFormat::I16),
        AudioError::UnsupportedSampleFormat(cpal::SampleFormat::I16)
    ));
```
- **Impact**:
The test `native_public_api_retains_cpal_types_and_entry_points` claims to verify error handling for unsupported sample formats. However, lines 85-88 instantiate `AudioError::UnsupportedSampleFormat(cpal::SampleFormat::I16)` inline and assert that it matches the exact same pattern. It exercises no library function or conversion path and cannot fail under any regression.
- **Fix**:
Invoke the actual entry point or conversion that emits `AudioError::UnsupportedSampleFormat` (such as testing an unsupported sample format passed to `CpalAudioOutput::start_on_device` or validating the `Display` format of the error).
- **Confidence**: high

### [P2] `SessionSettingsPanel.test.tsx` tests extracted helpers directly, bypassing the volume slider UI
- **Location**: `clients/rust/tauri-shell/src/features/session/SessionSettingsPanel.test.tsx:117` (and secondary sites lines 144, 173, 208)
- **Evidence**:
```tsx
  it("changing the volume slider calls the volume wrapper with a 0..1 value, not 0..100", async () => {
    // 1. Driving volume change with value 65 on 0..100 scale
    await applyVolumeLevel(65, true);

    expect(setAudioVolumeCalls.length).toBe(1);
    const calledVolume = setAudioVolumeCalls[0];

    // Assert that the called value is in 0..1, NOT 0..100
    expect(calledVolume).toBe(0.65);
```
- **Impact**:
The test suite names a specific UI interaction regression: "changing the volume slider calls the volume wrapper with a 0..1 value, not 0..100". However, the test never renders `SessionSettingsPanel` nor interacts with its `<Slider />` element. Instead, it directly calls the standalone helper `applyVolumeLevel(65, true)`.
If a regression in `SessionSettingsPanel.tsx` causes the slider's `onValueChange` handler to send raw 0..100 values, bind to the wrong state variable, or omit the event handler entirely, this test will still pass. The same pattern applies to `toggleAudioMuted`, `applyBitrateCeiling`, and `applyAudioDeviceSelection`. The UI controls remain completely unverified.
- **Fix**:
Render `<SessionSettingsPanel isConnected={true} />` into a DOM container and simulate user interactions (e.g. slider change events, button clicks) on the rendered DOM elements, verifying that `setAudioVolume` receives the normalized float through the real UI component.
- **Confidence**: high

### [P2] Latency percentiles in `receiver_telemetry_tests.rs` are pinned to identical constants (20), masking rank inversion bugs
- **Location**: `clients/rust/maho-app/src/bin/maho_client/receiver_telemetry_tests.rs:20`
- **Evidence**:
```rust
    for (field, value) in [
        ("frames", 3),
        ("p50_us", 20),
        ("p95_us", 20),
        ("p99_us", 20),
        ("max_us", 20),
    ] {
        assert_eq!(json[field], value, "{field}");
    }
```
- **Impact**:
The test pushes `[999, 10, 20]` into a `LatencyRecorder` with capacity 2. Sample `999` is evicted upon inserting `20`, leaving `[10, 20]`. For a 2-element array, `(len - 1) * percentile` rounds to index 1 for all percentiles (0.50, 0.95, and 0.99), making p50, p95, p99, and max all evaluate to `20`.
If the JSON serialization or calculation in `stats_json` inverts percentiles, confuses `p50` with `max`, or maps fields incorrectly, the assertion `assert_eq!(json[field], 20)` still passes. The test cannot fail for field mapping errors across percentiles.
- **Fix**:
Use a sample dataset with distinct values at each percentile rank (e.g., 100 sequential samples or distinct steps) so that `p50_us`, `p95_us`, `p99_us`, and `max_us` produce strictly different expected values.
- **Confidence**: high

### [P2] `mobile_regressions.rs` silently exits on constructor failure and guards assertions with `if let Ok`
- **Location**: `clients/rust/maho-mobile/tests/mobile_regressions.rs:95` (and secondary sites lines 78, 105, 118)
- **Evidence**:
```rust
    let mut player = match AndroidAudioTrackPlayer::new(48_000, 2) {
        Ok(p) => p,
        Err(_) => return,
    };

    // When: Nonempty PCM buffer is written.
    let pcm = [0.1f32, -0.1f32, 0.2f32, -0.2f32];
    let res = player.write_pcm(&pcm);

    // Then: Must not report written samples and counter must remain 0.
    if let Ok(written) = res {
        assert_eq!(written, 0);
    }
    assert_eq!(player.samples_written, 0);
```
- **Impact**:
If `AndroidAudioTrackPlayer::new` fails, the test hits `Err(_) => return` and exits with an unasserted pass. The same silent-exit pattern exists in `android_mediacodec_missing_native_backend_must_not_report_decoded` (line 80) and `ios_videotoolbox_missing_native_backend_must_not_report_rendered` (line 120).
Furthermore, `if let Ok(written) = res` wraps the `assert_eq!(written, 0)` assertion. When `write_pcm` returns `Err(AndroidMediaError::BackendUnavailable)` as expected when the native backend is absent, the assertion block never runs.
- **Fix**:
Replace `match ... { Ok(p) => p, Err(_) => return }` with `.expect("initialization must succeed")`, and assert the expected error explicitly: `assert_eq!(res, Err(AndroidMediaError::BackendUnavailable))`.
- **Confidence**: high

### [P3] Global `user-select: none` in `index.html` and `globals.css` disables text selection across entire desktop shell
- **Location**: `clients/rust/tauri-shell/src/styles/globals.css:79` (and secondary sites `clients/rust/tauri-shell/index.html:8`, `clients/rust/tauri-shell/src/app/App.tsx:60`, `clients/rust/tauri-shell/src/features/session/SessionView.tsx:212`)
- **Evidence**:
```css
  html,
  body,
  #root {
    width: 100%;
    height: 100%;
    margin: 0;
    padding: 0;
    overflow: hidden;
    user-select: none;
    -webkit-user-select: none;
  }
```
- **Impact**:
Applying `user-select: none;` and `overflow: hidden;` globally to `html`, `body`, and `#root` prevents users from selecting, copying, or highlighting text throughout the desktop shell (e.g. host IDs, Tailscale IPs, error descriptions, diagnostics, and settings text).
Additionally, full-viewport reset rules (`fixed inset-0`, `overflow: hidden`, `width: 100%`, `user-select: none`) are repeated five times across `index.html` inline styles, `globals.css` base layer, `App.tsx` wrapper `div`, `SessionView.tsx` container `className`, and `SessionView.tsx` inline `style`.
- **Fix**:
Confine `select-none` to the active streaming viewport container (`#viewport` / `SessionView`), removing the global `user-select: none` from `index.html` and `globals.css`. Remove duplicated inline styles in `SessionView.tsx` that duplicate Tailwind classes.
- **Confidence**: high

## Non-findings checked
- Verified `maho-proto/tests/protocol_v3.rs` tests packet headers, codec error variants, and handshake v3 round-trips without tautologies or mock-only assertions.
- Verified `maho-proto/tests/framing_burst.rs` validates 1024-packet coalesced bursts and invalid tail resynchronization deterministically.
- Verified `maho-proto/tests/timestamp_stats.rs` verifies wire layout, endianness, truncated payloads, and single-byte corruption against fixed byte vectors.
- Verified `maho-render/tests/audio_queue.rs` confirms atomic rejection of invalid PCM floats, non-finite values, misaligned stereo frames, and volume bounds without timing-dependent sleeps.
- Verified `maho-net/src/nonblocking_write_tests.rs` tests partial ciphertext retry and WouldBlock handling using deterministic gated I/O rather than fixed delays.
- Verified `maho-net/tests/discovery_metadata.rs` tests v3 TXT record parsing, IPv4 preference, protocol validation, and port clamping.
- Verified `maho-net/tests/discovery_scoped_ipv6.rs` verifies scoped link-local publishing, unscoped retraction, and interface index resolution across Linux, macOS, and iOS platforms.
- Verified `maho-app/tests/session_mock.rs` verifies PSK handshake, input event round-trip, permission modes, and ABR bitrate adjustment over loopback TCP/UDP.
- Verified `maho-app/tests/core_semantics.rs` validates pointer coordinate inversion/clamping, virtual key mappings, out-of-order audio fragment reassembly, and clipboard duplicate suppression.
- Verified `maho-app/tests/client_copy_cost.rs` tracks exact allocation counts and SHA256 frame hashes through counting allocator without wall-clock sleeps.
- Verified `maho-app/tests/cli_mcp_contract.rs` tests EOF shutdown, HTTP port conflict handling, and tool call dispatch using real process streams and oneshot synchronization.
- Verified `maho-host/tests/pairing_isolation.rs` verifies client outbound credentials cannot be loaded or used by the host authorization store.
- Verified `maho-host/src/sender_packetization_tests.rs` verifies MTU headroom (1200-byte budget) across single-chunk and multi-chunk video/audio frame packetization.
- Verified `maho-host/src/sender_trace_tests.rs` validates sequence tracking, error recording, and fixed-capacity ring buffer overflow without timing-luck.
- Verified `tauri-shell/src-tauri/src/recovery_tests.rs` exercises HEVC keyframe recovery and out-of-order packet suppression with real NALU bitstreams and channel barriers.
- Verified `tauri-shell/src-tauri/src/mailbox_tests.rs` validates latest-clipboard retention under 128 backpressured TCP frames without race conditions.
- Verified `tauri-shell/src-tauri/src/discovery_tests.rs` tests Tailscale JSON parsing, error propagation on missing binary, and LAN deduplication.
- Verified `tauri-shell/src-tauri/src/desktop_integration_tests.rs` tests gain, muting, device switching, and input payload mapping with simulated backend queues.
- Verified `tauri-shell/src/lib/overlay.test.ts` confirms overlay input filtering, held input tracking, and idempotent release events.
- Verified `tauri-shell/src/lib/renderer.test.ts` confirms BT.601 full-range NV12-to-RGB conversion round-trips within tight 1/255 tolerances across standard test colors.
- Verified `tauri-shell/src/lib/connection.test.ts` verifies PIN format validation, custom port forwarding, state machine transitions, and retryable cleanup without fixed sleeps.
- Verified `tauri-shell/src/features/session/useRemoteInput.test.ts` validates repeat-key suppression, modifier bitmasks (shift=1, ctrl=2, alt=4, meta=8), and held-button teardown releases.
