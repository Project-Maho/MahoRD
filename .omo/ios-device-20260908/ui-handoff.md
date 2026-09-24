# iPhone UI Handoff

## 1. Owned and Created Files
- `clients/rust/ios-shell/DESIGN.md` (iOS UI design authority, token mapping, safe areas, touch ergonomics)
- `clients/rust/ios-shell/ui/index.html` (Mobile UI document structure, safe-area layout, viewport meta)
- `clients/rust/ios-shell/ui/styles.css` (Charcoal/coral design tokens, 44px+ touch targets, landscape/portrait rules)
- `clients/rust/ios-shell/ui/frame-parser.js` (NV12 binary buffer unpacking, 16-byte LE header, plane offsets)
- `clients/rust/ios-shell/ui/touch-coords.js` (Aspect-fit letterbox/pillarbox math, top-left 0..1 normalization, touch tracking)
- `clients/rust/ios-shell/ui/keyboard-mapper.js` (Mobile accessory key codes, modifier bitmasks, payload validation)
- `clients/rust/ios-shell/ui/connection-state.js` (Connection lifecycle manager, PIN validation, stats polling, generation guards)
- `clients/rust/ios-shell/ui/video-renderer.js` (WebGL2 & WebGL1 NV12 shader pipeline, texture uploads, sequence reporting)
- `clients/rust/ios-shell/ui/lifecycle.js` (Extracted lifecycle event binding seam)
- `clients/rust/ios-shell/ui/input-queue.js` (Serialized FIFO input queue preventing stale dispatch across generation changes)
- `clients/rust/ios-shell/ui/app.js` (Event coordination, single-flight polling loop, input scope segregation, startup auto-connect)
- `clients/rust/ios-shell/ui/test/frame-parser.test.mjs` (Deterministic tests for frame header parsing and buffer validation)
- `clients/rust/ios-shell/ui/test/touch-coords.test.mjs` (Deterministic tests for aspect-fit coordinate normalization and clamping)
- `clients/rust/ios-shell/ui/test/keyboard-mapper.test.mjs` (Deterministic tests for key code and modifier bitflag mapping)
- `clients/rust/ios-shell/ui/test/connection-state.test.mjs` (Deterministic tests for connect validation and state transitions)
- `clients/rust/ios-shell/ui/test/lifecycle.test.mjs` (Lifecycle regression tests covering hidden/pagehide disconnect and blur)
- `clients/rust/ios-shell/ui/test/input-queue.test.mjs` (Deterministic tests for FIFO ordering and stale-drop across generation changes)

## 2. Verification Commands for the Lead
### Remote Verification on Omarchy (Verified)
Executed remote test suite via Tailscale SSH runner on Omarchy Arch Linux (`100.91.254.71`):
```bash
rsync -az clients/rust/ios-shell/ui/ indo@100.91.254.71:/home/indo/projects/erd-ios-device-20260908/ui/ && \
ssh indo@100.91.254.71 "bash -lc 'cd /home/indo/projects/erd-ios-device-20260908/ui && bun test test'"
```

Actual test run output (100% passing, 0 failures):
```text
bun test v1.4.0 (34cbb9a40)

test/connection-state.test.mjs:
(pass) validateConnectRequest validates required host and optional 8-digit PIN
(pass) createConnectionManager transitions state from idle to waiting-video to streaming
(pass) createConnectionManager ignores stale frame tokens from prior generation
(pass) createConnectionManager reports error when native bridge is missing

test/frame-parser.test.mjs:
(pass) parseNv12Frame parses valid 4x2 NV12 frame buffer
(pass) parseNv12Frame rejects buffer shorter than 16-byte header
(pass) parseNv12Frame rejects truncated frame data
(pass) parseNv12Frame rejects zero dimensions
(pass) parseNv12Frame accepts Uint8Array slices

test/keyboard-mapper.test.mjs:
(pass) mapNamedKeyToCode maps known keys to wire key codes
(pass) createModifierTracker toggles and clears modifier bitflags
(pass) validateKeyPayload validates well-formed send_key payloads
(pass) validateKeyPayload rejects invalid key payload types

test/touch-coords.test.mjs:
(pass) calculateAspectFit calculates correct pillarbox in wide container
(pass) calculateAspectFit calculates correct letterbox in tall container
(pass) calculateAspectFit handles zero or invalid dimensions gracefully
(pass) normalizeTouchCoordinates normalizes coordinates accurately
(pass) normalizeTouchCoordinates clamps out-of-bounds coordinates to 0..1
(pass) mapTouchPhase maps browser touch/pointer events to contract phases
(pass) createTouchTracker tracks and cancels touches

test/input-queue.test.mjs:
(pass) preserves execution order for serialized input commands
(pass) drops queued commands when generation changes before invocation
(pass) captures generation at enqueue time rather than invocation time
(pass) handles invoke rejection without breaking subsequent queue processing

test/lifecycle.test.mjs:
(pass) hidden during streaming invokes input release, disconnect, and invalidates generation
(pass) hidden during connecting invokes input release and disconnect
(pass) pagehide during streaming invokes input release and disconnect
(pass) blur while still visible releases held inputs but preserves polling and does not disconnect stream
(pass) returning to foreground does not resurrect disconnected session or accept stale frame completions

 29 pass
 0 fail
Ran 29 tests across 6 files. [26.00ms]
```

### Node Test Runner Alternative
```bash
node --test clients/rust/ios-shell/ui/test/*.test.mjs
```

## 3. UI and Command Contract Conformance
- `connect({ host, pin })`: Host is required; PIN is validated as optional 8 ASCII digits. PIN is strictly ephemeral in memory and never stored in `localStorage`.
- `disconnect()`: Flushes pending motion, cancels active touches, joins workers, stops audio, clears stale frame states.
- `stats()`: Queried every 500ms when connected; updates HUD metrics and gracefully handles backend state changes.
- `poll_frame()`: Binary response parsed with 16-byte LE header (`width: u32`, `height: u32`, `sequence: u64`) followed by contiguous NV12 Y (`width * height`) and interleaved UV (`width * ceil(height / 2)`) planes. Single-flight polling with session generation guards.
- `touch({ event: { id, x, y, phase } })`: Normalized top-left coordinates `[0.0, 1.0]` relative to the aspect-fit video rectangle. Phases: `"began" | "moved" | "ended" | "cancelled"`. The Rust touch handler owns inverted-Y wire normalization.
- `set_touch_mode({ mode })`: Direct Touch vs Trackpad Relative mode toggle.
- `send_key({ keyCode, down, modifiers })`: Physical wire key codes (Escape: 0x35, Tab: 0x30, ArrowUp: 0x7e, ArrowDown: 0x7d, ArrowLeft: 0x7b, ArrowRight: 0x7c) with modifier bitmask (Shift: 1, Ctrl: 2, Opt: 4, Cmd: 8).
- `set_muted({ muted })`: Real audio queue mute command with `aria-pressed` toggle.
- `presented({ sequence })`: Invoked after actual WebGL drawing on the first frame, and periodically every 60 frames to record verifiable client presentation progress.
- `startup()`: Queries `{ host, auto_connect }` on load. Automatically calls `connect({ host, pin: null })` when `auto_connect` is true, exercising identical backend paths without secret exposure.
- `lifecycle`:
  - Backgrounding (`document.visibilitychange (hidden)` or `window.pagehide`) during connecting or streaming immediately releases held touches, halts frame polling (`onStopPolling`), and invokes caller `onDisconnect` and `connection.disconnect()`, invalidating the generation token.
  - Visible `blur` only releases touch inputs and explicitly preserves polling and active stream rendering.
  - Returning to foreground does not resurrect the session or accept stale frame deliveries from prior generations.
- `inputQueue`:
  - Input commands (`touch` and `send_key`) are queued in strict FIFO order through a promise chain.
  - Captures session generation on enqueue and rechecks `isCurrent(capturedGen)` before native `invoke`.
  - Queued commands from prior generations are dropped upon generation change without sleeps.
  - Tolerates invoke failures without halting subsequent queue processing.

## 4. Resource Invariants and Safety
- Strict input scope segregation: Local UI buttons and sheets never leak touches to the remote canvas.
- Interruption safety: `pointercancel` releases active touches; visible `blur` releases touches while keeping the video stream alive; hidden/pagehide completely tears down streaming and cancels inputs.
- WebGL resilience: WebGL2 primary implementation with automatic WebGL1 fallback (`LUMINANCE` / `LUMINANCE_ALPHA`).
- Zero DOM/manifest/Rust edits: Preserved desktop Tauri shell, Rust core crates, and project configurations untouched.
