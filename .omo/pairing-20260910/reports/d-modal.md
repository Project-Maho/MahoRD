# Phase D Task Implementation Report: R7 iOS Connecting & Disconnecting Modal UX, Accessibility & Video Pipeline Verification

- Task ID: `st_01a08d7c` (succeeding `st_01a08a54` and verified against finished UI lifecycle contract `reports/d-ui.md` from `st_01a08d7a`)
- Node: `connecting-modal` (Phase D)
- Goal: Implement finding **R7** against the finished UI lifecycle state contract (`reports/d-ui.md`) and resolve in-scope video presentation pipeline mismatches blocking the `waiting-video` -> `streaming` transition.
- Worker: `hephaestus`
- Session: `01a0890a-69f1-7e5e-80cb-959dc3ddb61c` (Depth: 1)
- Date: 2026-09-10
- Reference Plan: `docs/remote-connection-pairing-review-plan-20260910.md` (Finding R7)
- Contracts: `.omo/pairing-20260910/contracts.md` (Section 7.3, 10.4), `phase-d-handoff.md`, and `.omo/pairing-20260910/reports/d-ui.md`
- Status & Dependency Notice:
  - Phase D overall is **OPEN / IN-PROGRESS**.
  - UI lifecycle contract R6 in `clients/rust/ios-shell/ui/connection-state.js` is finished and verified (`reports/d-ui.md`).
  - This worker strictly kept modal, presentation, and real-page test scope (`app.js`, `index.html`, `styles.css`, and UI test files); `connection-state.js`, native code, and desktop code were **NOT modified**.
- Scoped Paths:
  - `clients/rust/ios-shell/ui/index.html`
  - `clients/rust/ios-shell/ui/styles.css`
  - `clients/rust/ios-shell/ui/app.js`
  - `clients/rust/ios-shell/ui/test/page-lifecycle-order-a.test.mjs`
  - `clients/rust/ios-shell/ui/test/dom-modal.test.mjs`
  - `clients/rust/ios-shell/ui/test/page-harness.mjs`
  - `clients/rust/ios-shell/ui/test/page-modal.test.mjs`
  - `clients/rust/ios-shell/ui/test/run-action-logs.mjs`
  - `clients/rust/ios-shell/ui/test/lifecycle.test.mjs`
  - `.omo/pairing-20260910/reports/d-modal.md`
  - `.omo/pairing-20260910/evidence/lifecycle-modal-red.log`
  - `.omo/pairing-20260910/evidence/lifecycle-modal-green.log`
  - `.omo/pairing-20260910/evidence/lifecycle-streaming-pipeline-red.log`
  - `.omo/pairing-20260910/evidence/lifecycle-streaming-pipeline-green.log`
  - `.omo/pairing-20260910/evidence/lifecycle-modal-connecting-430x932.png`
  - `.omo/pairing-20260910/evidence/lifecycle-modal-disconnecting-430x932.png`
  - `.omo/pairing-20260910/evidence/lifecycle-modal-settled-430x932.png`
  - `.omo/pairing-20260910/evidence/lifecycle-modal-connecting-1280x800.png`
  - `.omo/pairing-20260910/evidence/lifecycle-modal-disconnecting-1280x800.png`
  - `.omo/pairing-20260910/evidence/lifecycle-modal-settled-1280x800.png`
  - `.omo/pairing-20260910/evidence/lifecycle-modal-streaming-presented.png`
  - `.omo/pairing-20260910/evidence/lifecycle-modal-actions.json`

---

## 1. Executive Summary & Lead Review Resolutions

Implemented finding **R7** in the iOS shell UI (`ios-shell/ui/`), strictly observing all coordinator directives, coordinating with the finished UI lifecycle contract (`reports/d-ui.md`), and fixing the pre-existing video presentation pipeline defect:

1. **Independent Top-Level Modal Positioning:**
   - Moved `<div id="modal-connecting">` out of `<section id="view-session" hidden>` into the top-level `<body>` container as a direct child, positioned as a fixed sibling of `<main id="view-connect">` and `<section id="view-session">`. Modal visibility and geometry are now completely decoupled from whether `view-connect` or `view-session` is hidden.
2. **Prompt Cancellation Dispatch & Reachable Cancel Button:**
   - During `connecting` and `waiting-video` phases, `#btn-cancel-connect` has positive dimensions (`width: 83.19px, height: 44px`), zero hidden ancestors, sits within the viewport, hit-tests to itself, and dispatches native `disconnect` promptly when clicked.
3. **Cleanup Progress Visibility & Retention Across Both Settle Orders:**
   - During `disconnecting`, the modal remains visible with a spinning progress indicator, updates the status message to `"Disconnecting..."`, and disables `#btn-cancel-connect` (`disabled = true`) to prevent racing or duplicate teardown calls.
   - Real-page tests explicitly verify both completion orders:
     - **Order A (`page-lifecycle-order-a.test.mjs`):** `connect` settles (rejects/resolves) FIRST while `disconnect` remains in-flight; modal remains visible in disconnecting state until `disconnect` settles.
     - **Order B (`page-modal.test.mjs`):** `disconnect` settles FIRST while native `connect` remains deferred; modal remains visible in disconnecting state until `connect` settles.
4. **Strict Background Inertness:**
   - When the modal is open, both `#view-connect` and `#view-session` are set to `inert = true` and `aria-hidden = "true"`, with CSS `[inert]` rules enforcing `pointer-events: none !important; user-select: none !important;`. Background elements cannot receive focus or clicks while connection operations are pending.
5. **Real Focus Trapping & Restoration:**
   - Tests explicitly exercise `Tab`, `Shift+Tab`, and background focus interception via `focusin`. Focus is placed on `#btn-cancel-connect`, stays within modal focusables on keyboard navigation, redirects background focus attempts back to the modal, and restores to `#btn-connect-submit` upon modal close.
6. **Immediate Form PIN Clearance & Redaction:**
   - In `connectForm` submit handler, `pinInput.value = ''` immediately clears the PIN field upon submission. PIN is never retained in memory snapshots or logs. `window.__TAURI__.core.invoke` redacts `args.pin` to `"[REDACTED]"` in stored call history.
7. **Resolution of Pre-Existing Video Presentation Pipeline Mismatch (`app.js:pollLoop`):**
   - Aligned `pollLoop` to invoke `poll_frame`, check byte length (≥ 16 bytes), parse via `FrameParser.parseNv12Frame(packet)` (verifying `parsed.ok`), invoke `renderer.render(parsed)`, check render return status, trigger `onPresented(sequence)` -> `invoke('presented', { sequence })`, update FPS and resolution, call `connection.markFrameRendered(generation)`, and route unrecoverable frame/renderer errors to generation-aware cleanup.
8. **Repository Test Portability (Zero External Path Coupling):**
   - Eliminated all hardcoded user-home package imports (`/Users/indo/...`) and hardcoded Chrome application binary paths.
   - All browser interaction and DOM tests now run directly via `Bun.WebView` through `openPage` from `./page-harness.mjs` and native browser APIs (`page.view.click`, `page.view.screenshot`), running deterministically on macOS arm64 without external browser dependencies.
9. **Deterministic Task Synchronization (Zero Sleeping / Polling):**
   - Completely eliminated forbidden `waitForTimeout(200)` and fixed delays.
   - Replaced with direct event settlement observation (`fixture.until`) and a deterministic single-MessageChannel event task barrier (`fixture.taskBarrier()`) ensuring macro/microtasks flush with ports cleanly closed in `finally`.
10. **Confirmed Pre-Click Command Registration:**
    - Both test suites and action logs call `fixture.arm(cmd)` and verify synchronous confirmation before issuing separate browser clicks (`page.view.click`), eliminating race conditions where an IPC call could arrive before the listener was active.
11. **Strict Native Command Name Validation:**
    - Updated `page-harness.mjs` with `KNOWN_COMMANDS` matching `lib.rs`, rejecting unknown command names with `Unknown native command: <cmd>`.

---

## 2. Pre-Existing Video Presentation Pipeline Blocker Resolution

### 2.1 Mismatch Identification

| Layer / File | Expected API / Command | Buggy Implementation in `app.js` | Impact |
|---|---|---|---|
| Native IPC (`src/lib.rs:commands`) | `commands::poll_frame` | `invoke('next_frame')` | Command not registered in native backend; in strict harness, throws `Unknown native command: next_frame`. |
| Packet Parser (`frame-parser.js`) | `FrameParser.parseNv12Frame(buf)` | `FrameParser.parseFramePacket(packet)` | `parseFramePacket` does not exist; threw runtime `TypeError`. |
| WebGL Renderer (`video-renderer.js`) | `renderer.render(parsedFrame)` | `renderer.drawFrame(parsed)` | `drawFrame` does not exist; threw runtime `TypeError`. |
| Session State (`connection-state.js`) | `markFrameRendered(generation)` | Never reached | State remained stuck on `waiting-video`; modal stayed visible forever; `presented` IPC never sent. |

### 2.2 Authorized Resolution in `app.js`

```javascript
async function pollLoop(generation) {
  if (!renderLoopActive || !connection.isCurrent(generation)) {
    return;
  }

  if (hasNative) {
    try {
      const packet = await invoke('poll_frame');
      if (!connection.isCurrent(generation)) {
        return;
      }
      if (packet) {
        const byteLength = packet.byteLength !== undefined
          ? packet.byteLength
          : packet.length !== undefined
          ? packet.length
          : 0;

        if (byteLength >= 16) {
          const parsed = FrameParser.parseNv12Frame(packet);
          if (parsed && parsed.ok && renderer) {
            const success = renderer.render(parsed);
            if (success) {
              connection.markFrameRendered(generation);
              updateFpsCounter();
              if (statResolution) {
                statResolution.textContent = `${parsed.width}x${parsed.height}`;
              }
            } else {
              console.error('Renderer failed to draw frame; initiating session cleanup');
              if (connection.isCurrent(generation)) {
                stopPresentation();
                connection.disconnect().catch(() => {});
                return;
              }
            }
          } else if (parsed && !parsed.ok) {
            console.warn('Frame parse error:', parsed.error);
          }
        }
      }
    } catch (err) {
      console.error('poll_frame unrecoverable error:', err);
      if (connection.isCurrent(generation)) {
        stopPresentation();
        connection.disconnect().catch(() => {});
        return;
      }
    }
  }

  if (renderLoopActive && connection.isCurrent(generation)) {
    requestAnimationFrame(() => pollLoop(generation));
  }
}
```

### 2.3 Dedicated Video Pipeline Failing-First Proof

**Command:**
```bash
bun test clients/rust/ios-shell/ui/test/page-modal.test.mjs --test-name-pattern "actual-page video streaming pipeline"
```

**RED Result on Unpatched `app.js` (`lifecycle-streaming-pipeline-red.log`):**
```text
clients/rust/ios-shell/ui/test/page-modal.test.mjs:
(fail) actual-page video streaming pipeline: poll_frame NV12 frame transitions waiting-video to streaming, renders WebGL, dispatches presented, and hides modal [20000.24ms]
  ^ this test timed out after 20000ms.
Proved that unpatched pollLoop never invoked poll_frame and failed to transition to streaming.
```

**GREEN Result on Patched `app.js` (`lifecycle-streaming-pipeline-green.log`):**
```text
clients/rust/ios-shell/ui/test/page-modal.test.mjs:
(pass) actual-page video streaming pipeline: poll_frame NV12 frame transitions waiting-video to streaming, renders WebGL, dispatches presented, and hides modal [323.80ms]
1 pass, 0 fail (8 expect() calls, finished in 532.00ms, exit code 0)
```

**Visual Verification (`lifecycle-modal-streaming-presented.png`):**
- Real 2x2 NV12 frame (16-byte header + 4 Y bytes + 2 UV bytes per `src/frame.rs`) rendered in WebGL on `#screen-canvas`.
- Connecting modal completely hidden (`modal.hidden === true`).
- `#session-view` active.
- Top overlay menu pill visible with connection status dot.
- Virtual accessory keyboard bar active at viewport bottom.
- `presented` IPC call verified with `{ sequence: 1 }`.

---

## 3. Pre-Pinned RED Phase Verification & Evidence

Before executing GREEN runs, regressions were pinned in `/var/folders/zh/7cc25lt91b1_dj577306nwdh0000gn/T/ulw-20260910-111421.XXXXXX.md.pNqwGP19PH` and verified RED.

### 3.1 RED Command Execution

1. **Unpatched DOM & Page Modal Baseline (Original Defect Proof):**
   ```bash
   bun test clients/rust/ios-shell/ui/test/dom-modal.test.mjs clients/rust/ios-shell/ui/test/page-modal.test.mjs
   ```
   *Result:* 1 pass, 7 fail across 2 files (exit code 1).
   *Defects Proven:*
   - `modal-connecting` parent was `view-session` rather than `body`.
   - `btn-cancel-connect` had hidden ancestor `view-session` and 0 client rects.
   - `connectView.inert` was false.
   - Focus was not placed on `btn-cancel-connect`.
   - PIN input was not cleared upon submission (retained `"12345678"`).
   - Real browser interaction at 430x932 and 1280x800 failed on hidden ancestor and uncleared PIN.
   *Log:* `.omo/pairing-20260910/evidence/lifecycle-modal-red.log`.

2. **Order A Cancellation Regression (Connect Settles Before Disconnect):**
   ```bash
   bun test clients/rust/ios-shell/ui/test/page-lifecycle-order-a.test.mjs
   ```
   *Mutation:* Temporarily mutated `app.js` line 346: `isConnectingModalVisible = phase === 'connecting' || phase === 'waiting-video'` (omitting `disconnecting` phase).
   *Result:* 0 pass, 2 fail across 1 file (exit code 1).
   *Failure:* `expect(disconnectingState.modalHidden).toBe(false)` received `true` at both 430x932 and 1280x800. Proved that without explicit disconnecting modal state handling, the modal hides prematurely while native cleanup is in flight.

---

## 4. Post-Implementation GREEN Phase Verification & Evidence

### 4.1 GREEN Command Execution

```bash
bun test clients/rust/ios-shell/ui/test/page-lifecycle-order-a.test.mjs clients/rust/ios-shell/ui/test/page-modal.test.mjs clients/rust/ios-shell/ui/test/dom-modal.test.mjs
```

### 4.2 Raw GREEN Results (Saved at `.omo/pairing-20260910/evidence/lifecycle-modal-green.log`)

```text
bun test v1.4.0 (34cbb9a40)

clients/rust/ios-shell/ui/test/page-lifecycle-order-a.test.mjs:
(pass) real browser interaction Order A at 430x932: fill, submit, modal appearance, hit-test, focus trap, cancel click, connect settles before disconnect, cleanup progress visible until disconnect settles, focus restore, and no stale reconnect [507.91ms]
(pass) real browser interaction Order A at 1280x800: fill, submit, modal appearance, hit-test, focus trap, cancel click, connect settles before disconnect, cleanup progress visible until disconnect settles, focus restore, and no stale reconnect [264.13ms]

clients/rust/ios-shell/ui/test/page-modal.test.mjs:
(pass) real browser interaction at 430x932: fill, submit, modal appearance, hit-test, focus trap, cancel click, cleanup progress, focus restore, and no stale reconnect [261.86ms]
(pass) real browser interaction at 1280x800: fill, submit, modal appearance, hit-test, focus trap, cancel click, cleanup progress, focus restore, and no stale reconnect [227.88ms]
(pass) actual-page video streaming pipeline: poll_frame NV12 frame transitions waiting-video to streaming, renders WebGL, dispatches presented, and hides modal [177.15ms]

clients/rust/ios-shell/ui/test/dom-modal.test.mjs:
(pass) connecting modal is child of body, not hidden session view [84.68ms]
(pass) cancel button is clickable and dispatches disconnect while connecting [84.47ms]
(pass) background views are marked inert and aria-hidden when modal is open [88.79ms]
(pass) focus is trapped to modal and restored on close [85.43ms]
(pass) pin input is cleared upon submission [91.80ms]
(pass) cleanup progress visible and cancel button disabled during disconnecting [118.80ms]

 11 pass
 0 fail
 155 expect() calls
Ran 11 tests across 3 files. [2.98s]
```

---

## 5. Real-Page Browser Interaction Evidence (430x932 & 1280x800)

Using native `Bun.WebView` driven via `openPage` from `./page-harness.mjs`, the complete user interaction lifecycle was verified on live served HTML/CSS/JS without external browser dependencies.

### 5.1 Verification Summary Table

| Viewport | Connecting Modal Centered & Visible | Zero Hidden Ancestors | Hit-Test Target Matches Button | Cancel Button Interactive | Background Views Inert | PIN Cleared on Submit | Order A: Connect Settle While Disconnect Pending: Modal Retained | Order B: Disconnect Settle While Connect Deferred: Modal Retained | Both Settled: Modal Hides | Focus Restored on Settle | No Stale Reconnect (Task Barrier) |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **430x932** (Mobile) | YES (`430x932`) | YES (`null`) | YES (`hitTargetId: btn-cancel-connect`) | YES (`disabled: false`) | YES (`inert: true`) | YES (`""`) | YES (`"Disconnecting...", disabled: true`) | YES (`"Disconnecting...", disabled: true`) | YES (`modalHidden: true, inert: false`) | YES (`activeId: btn-connect-submit`) | YES (1 connect call only) |
| **1280x800** (Desktop) | YES (`1280x800`) | YES (`null`) | YES (`hitTargetId: btn-cancel-connect`) | YES (`disabled: false`) | YES (`inert: true`) | YES (`""`) | YES (`"Disconnecting...", disabled: true`) | YES (`"Disconnecting...", disabled: true`) | YES (`modalHidden: true, inert: false`) | YES (`activeId: btn-connect-submit`) | YES (1 connect call only) |

### 5.2 Visual Inspection of Captured Screenshots

All screenshots were visually inspected and verified via `read`:
1. **Mobile Connecting (`lifecycle-modal-connecting-430x932.png`):**
   - Translucent dark frosted backdrop (`rgba(20, 21, 23, 0.85)`) over the entire 430x932 screen.
   - Centered card with spinning accent indicator (`#f47660`).
   - "Connecting to host...", "192.0.2.10".
   - Cancel button displayed with clear red outline, centered and active with focus outline.
2. **Mobile Disconnecting (`lifecycle-modal-disconnecting-430x932.png`):**
   - Spinner actively spinning.
   - Status updated to "Disconnecting...".
   - Cancel button rendered in disabled state (`opacity` reduced, cursor not-allowed), preventing duplicate clicks while native cleanup runs.
3. **Mobile Settled (`lifecycle-modal-settled-430x932.png`):**
   - Modal cleanly hidden.
   - Host Address retains "192.0.2.10".
   - Pairing PIN field is completely blank.
   - Connect button enabled and ready.
4. **Desktop Connecting (`lifecycle-modal-connecting-1280x800.png`):**
   - Modal card centered horizontally and vertically across 1280x800 viewport.
   - Translucent frosted glass effect obscures background connect view.
5. **Desktop Disconnecting (`lifecycle-modal-disconnecting-1280x800.png`):**
   - Shows "Disconnecting..." status and disabled Cancel button.
6. **Desktop Settled (`lifecycle-modal-settled-1280x800.png`):**
   - Modal dismissed; focus returned to "Connect" button.
7. **Streaming Active (`lifecycle-modal-streaming-presented.png`):**
   - Modal hidden. 2x2 NV12 frame rendered on `#screen-canvas` in WebGL. Virtual accessory bar and session trigger menu active.

### 5.3 Structured Action Log Extract (`lifecycle-modal-actions.json`)

```json
{
  "timestamp": "2026-09-10T22:44:29.440Z",
  "testRun": "R7 Modal UX & Lifecycle Verification",
  "viewports": [
    {
      "viewport": "430x932",
      "dimensions": { "width": 430, "height": 932 },
      "steps": [
        { "step": "page_ready", "origin": "http://127.0.0.1:62857" },
        { "step": "form_filled", "host": "192.0.2.10", "hasPin": true },
        { "step": "command_armed", "command": "connect", "armed": true },
        {
          "step": "connect_issued",
          "command": "connect",
          "args": { "host": "192.0.2.10", "pin": "[REDACTED]" }
        },
        { "step": "pin_cleared_verification", "pinValue": "", "isCleared": true },
        {
          "step": "connecting_modal_inspected",
          "modalParent": "body",
          "hiddenAncestor": null,
          "modalHidden": false,
          "modalRect": { "x": 0, "y": 0, "width": 430, "height": 932 },
          "btnRect": { "x": 173.4, "y": 506.5, "width": 83.19, "height": 44 },
          "btnDisabled": false,
          "inViewport": true,
          "hitTargetId": "btn-cancel-connect",
          "hitMatches": true,
          "activeElementId": "btn-cancel-connect",
          "connectViewInert": true,
          "sessionViewInert": true
        },
        {
          "step": "focus_trap_exercised",
          "afterTab": true,
          "afterShiftTab": true,
          "afterBgAttempt": true
        },
        { "step": "command_armed", "command": "disconnect", "armed": true },
        { "step": "disconnect_issued", "command": "disconnect" },
        {
          "step": "disconnecting_inspected",
          "modalHidden": false,
          "btnDisabled": true,
          "statusText": "Disconnecting..."
        },
        {
          "step": "cleanup_disconnect_settled_connect_pending",
          "modalHidden": false,
          "btnDisabled": true,
          "statusText": "Disconnecting...",
          "modalRemainsDuringCleanup": true
        },
        {
          "step": "settled_inspected",
          "modalHidden": true,
          "connectViewInert": false,
          "activeElementId": "btn-connect-submit",
          "submitDisabled": false
        },
        { "step": "stale_connect_check", "totalConnectCalls": 1, "noStaleCalls": true }
      ],
      "screenshots": {
        "connecting": ".omo/pairing-20260910/evidence/lifecycle-modal-connecting-430x932.png",
        "disconnecting": ".omo/pairing-20260910/evidence/lifecycle-modal-disconnecting-430x932.png",
        "settled": ".omo/pairing-20260910/evidence/lifecycle-modal-settled-430x932.png"
      },
      "cleanup": { "webViewClosed": true, "serverStopped": true }
    }
  ]
}
```

---

## 6. Broad Test Suite Verification

### 6.1 Full iOS Shell UI Suite Pass

Command:
```bash
bun test clients/rust/ios-shell/ui/test/
```

Result:
```text
clients/rust/ios-shell/ui/test/frame-parser.test.mjs: 5 pass
clients/rust/ios-shell/ui/test/discovery.test.mjs: 25 pass
clients/rust/ios-shell/ui/test/page-lifecycle-order-a.test.mjs: 2 pass
clients/rust/ios-shell/ui/test/connection-state.test.mjs: 27 pass
clients/rust/ios-shell/ui/test/input-queue.test.mjs: 4 pass
clients/rust/ios-shell/ui/test/lifecycle.test.mjs: 5 pass
clients/rust/ios-shell/ui/test/page-modal.test.mjs: 3 pass
clients/rust/ios-shell/ui/test/keyboard-mapper.test.mjs: 4 pass
clients/rust/ios-shell/ui/test/touch-coords.test.mjs: 7 pass
clients/rust/ios-shell/ui/test/dom-modal.test.mjs: 6 pass

Result: 88 passed, 0 failed across 10 files (finished in 2.31s, exit code 0)
```

### 6.2 Full Desktop Tauri Shell UI Suite Safety Check

Command:
```bash
bun test clients/rust/tauri-shell/ui/
```

Result:
```text
clients/rust/tauri-shell/ui/performance.test.mjs: 16 pass
clients/rust/tauri-shell/ui/library.test.mjs: 9 pass
clients/rust/tauri-shell/ui/connection-state.test.mjs: 16 pass
clients/rust/tauri-shell/ui/session-overlay.test.mjs: 19 pass

Result: 60 passed, 0 failed across 4 files (finished in 267ms, exit code 0)
```

### 6.3 Pure Line of Code (LOC) Compliance

Per the programming skill discipline, every touched file is kept strictly below the 250 pure LOC ceiling:
- `clients/rust/ios-shell/ui/test/page-lifecycle-order-a.test.mjs`: **136 pure LOC** (Healthy)
- `clients/rust/ios-shell/ui/test/page-modal.test.mjs`: **204 pure LOC** (Healthy)
- `clients/rust/ios-shell/ui/test/dom-modal.test.mjs`: **204 pure LOC** (Healthy)
- `clients/rust/ios-shell/ui/test/page-harness.mjs`: **237 pure LOC** (Healthy)
- `clients/rust/ios-shell/ui/test/run-action-logs.mjs`: **169 pure LOC** (Healthy)

---

## 7. Scope, Boundaries & Physical iPhone Device Limits

- **Scoped File Boundary:**
  - Touched: `ios-shell/ui/index.html`, `ios-shell/ui/styles.css`, `ios-shell/ui/app.js`, `ios-shell/ui/test/page-lifecycle-order-a.test.mjs`, `ios-shell/ui/test/dom-modal.test.mjs`, `ios-shell/ui/test/page-harness.mjs`, `ios-shell/ui/test/page-modal.test.mjs`, `ios-shell/ui/test/run-action-logs.mjs`, and minor mock settlement fix in `lifecycle.test.mjs`.
  - Untouched: `connection-state.js` was strictly NOT modified in this task.
  - Zero modifications to desktop native code, desktop UI, or Rust session code.
  - Zero git commits created.
- **Physical Device Boundary (Stated Honestly):**
  - All browser interaction and visual QA checks were executed on real `Bun.WebView` instances with exact viewport emulation (430x932 and 1280x800).
  - These tests verify DOM tree structure, computed CSS styling, hit-testing, focus trapping, and IPC event serialization.
  - Physical iOS hardware touch gesture interpretation, mobile Safari WKWebView native viewport inset behavior, and iOS developer certificate signing are not simulated and remain strictly reserved for the coordinator lead at final physical-device review.
- **Workflow State:**
  - `d-native.md` is NOT accepted; native worker follow-up is ongoing.
  - Phase D overall acceptance remains OPEN pending coordinator integration and verification of all revived nodes.
