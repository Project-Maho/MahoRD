# Phase D Task Implementation Report: R6 iOS Connection State Manager Lifecycle & State API

- Task ID: `st_01a08d7a` (follow-up and comprehensive lifecycle verification, succeeding `st_01a08a4b`)
- Node: `ui-lifecycle` (Phase D)
- Goal: Implement R6 in `clients/rust/ios-shell/ui/connection-state.js` and regressions in `clients/rust/ios-shell/ui/test/connection-state.test.mjs`. Deliver exact state API specification for the modal owner (`connecting-modal` / R7).
- Worker: `hephaestus`
- Session: `01a0890a-69f1-7e5e-80cb-959dc3ddb61c` (Depth: 1)
- Date: 2026-09-10
- Reference Plan: `docs/remote-connection-pairing-review-plan-20260910.md` (Finding R6)
- Contracts: `.omo/pairing-20260910/contracts.md` (Section 7.2, 10.4) & `phase-d-handoff.md`
- Scoped Paths:
  - `clients/rust/ios-shell/ui/connection-state.js`
  - `clients/rust/ios-shell/ui/test/connection-state.test.mjs`
  - `clients/rust/ios-shell/ui/test/discovery.test.mjs`
  - `.omo/pairing-20260910/reports/d-ui.md`
  - `.omo/pairing-20260910/evidence/lifecycle-ui-red.log`
  - `.omo/pairing-20260910/evidence/lifecycle-ui-green.log`

---

## 1. Executive Summary & Lead Review Blockers Addressed

Implemented finding **R6** in the iOS connection state manager (`clients/rust/ios-shell/ui/connection-state.js`) and verified all lifecycle transitions and lead review blockers:

1. **Resolution of Premature Cleanup Settlement (Strict Await of Connect Settlement):**
   - *Previous Flaw:* An earlier revision used `Promise.race([connectToSettle, Promise.resolve()])`, which immediately resolved on the microtask queue, allowing cleanup to return and state to transition to `idle` while native connect was still pending. This resulted in a split-brain snapshot with `busy: true`, `hasOwnedSession: false`, and premature `idle`.
   - *Resolution:* Removed the `Promise.race` bypass. `disconnectInternal` dispatches native `invoke('disconnect')` **promptly** without blocking on connect, and then strictly awaits `connectToSettle` (`await connectToSettle`). Cleanup is held in `'disconnecting'` with `busy: true` until BOTH native disconnect and connect settlement complete. Verified across both completion orders: Order A (connect settles before disconnect) and Order B (disconnect settles before connect).
2. **Native Stats Contract & Terminal Session Ownership:**
   - *Previous Flaw:* `pollStats()` only checked `res.state === 'error'`. When native produced `{ state: 'disconnected', last_error: 'remote-closed' }`, `pollStats()` left the UI hanging in `waiting-video`.
   - *Resolution:* `pollStats()` now recognizes all native terminal states: `res.state === 'disconnected'`, `res.state === 'error'`, and `res.connected === false`. It preserves the terminal reason (e.g. `'remote-closed'`), transitions state to `'error'`, and immediately runs/owns native cleanup (`await disconnectInternal(false, true)`) without waiting for manual user dismissal. Stale completions from earlier generations are dropped without state mutation.
3. **Guaranteed Final Subscriber Snapshot (`busy === false`):**
   - *Previous Flaw:* `emit()` was previously called before `pendingCleanup = null` was set in `.finally()`, causing subscribers to see `busy: true` in the final snapshot.
   - *Resolution:* `pendingCleanup = null` is cleared before `emit()` in `.finally()`. When cleanup and connect have truly settled, the final subscriber snapshot reports `busy: false`, `hasOwnedSession: false`, and `state: 'idle'`.
4. **Deterministic Event-Loop Task Synchronization in Tests:**
   - Replaced all timed drains (`setTimeout`) in tests with a deterministic event-loop task barrier via `MessageChannel`. The listener is registered on `port2.onmessage` before calling `port1.postMessage(null)`, awaited after native deferred completion so all microtask reaction chains drain first, and both ports are closed in `finally`.
   - All declared `connecting` promises are strictly awaited.
   - All fixtures and task barriers are protected by `try ... finally` blocks to ensure prompt resource disposal even on assertion failures.
5. **Decoupled Resource Ownership from Display State:**
   - Native resource ownership (`hasOwnedSession`, `busy`) is tracked independently of display phases (`state`). If native connection resources have been allocated, errors leave `hasOwnedSession === true` until native cleanup successfully settles.
6. **Shared In-Flight Cleanup Promise:**
   - All concurrent or re-entrant invocations of `disconnect()`, `cancel()`, `dismissError()`, and `retryCleanup()` share a single in-flight `pendingCleanup` promise. Multiple callers receive the identical promise instance without spawning duplicate native IPC calls.
7. **`cleanup-failed` State & Lock Retention:**
   - If native disconnect fails, state transitions to `'cleanup-failed'`, preserving `hasOwnedSession === true` and blocking subsequent `connect()` attempts until `retryCleanup()` succeeds.
8. **Guarded `dismissError` Resource Teardown:**
   - `dismissError()` initiates native disconnect whenever `hasOwnedSession === true` or `state === 'cleanup-failed'`, transitioning to `'idle'` only after native resources are verified released.
9. **Preserved Identity Contracts:**
   - Preserved exact `pairingId` passing without PIN, zero PIN fallback, and strict isolation between untrusted discovery and saved Keychain pairings.
10. **Stale Generation Rejection Isolation:**
    - Late failures or rejections of stale connection attempts after a generation advance (e.g. following disconnect) are safely discarded without mutating state or overwriting `lastError`.

---

## 2. Exact State API Specification for Modal Owner (Node 3 `connecting-modal` / R7)

The modal owner (`app.js`, `index.html`, `styles.css`) coordinates the root viewport `<div id="modal-connecting">` with user actions (Connect, Cancel, Dismiss Alert, Disconnect). Below is the exact runtime contract provided by `createConnectionManager(options)`:

### 2.1 State Enumeration (`snap.state` / `getState()`)

| State Value | Semantic Meaning | Modal Visibility / Action | Cancel Button State | Submit Button State |
|---|---|---|---|---|
| `'idle'` | No active session or connection; native resources released. | Modal hidden (`hidden = true`). | N/A | Enabled (shows "Connect"). |
| `'connecting'` | Native connect in-flight; TCP/TLS/auth in progress. | Modal visible. Shows "Connecting to host...". | **Interactive & Enabled** (`disabled = false`). Clicking invokes `disconnect()`. | Disabled (shows "Connecting…"). |
| `'waiting-video'` | Native transport authenticated; waiting for first video frame. | Modal visible. Shows "Connected — waiting for video...". | **Interactive & Enabled** (`disabled = false`). Clicking invokes `disconnect()`. | Disabled. |
| `'streaming'` | Active video presentation running. | Modal hidden. Stream overlay active. | N/A | Disabled. |
| `'disconnecting'` | Native teardown or cancellation in-flight. | Modal visible. Shows "Disconnecting...". | Disabled (`disabled = true`) to prevent duplicate input. | Disabled. |
| `'error'` | Connection or runtime error occurred; native session released. | Modal hidden. Alert banner visible with `lastError`. | N/A | Enabled (ready for retry). |
| `'cleanup-failed'` | Native teardown failed; native resources still held/locked. | Modal hidden. Alert banner visible with `cleanupError` & `lastError`. | N/A | **Disabled / Blocked**. Connect disallowed until `retryCleanup()` succeeds. |

### 2.2 Snapshot Schema (`manager.snapshot()`)

```typescript
interface ConnectionSnapshot {
  // Lifecycle & display state
  state: 'idle' | 'connecting' | 'waiting-video' | 'streaming' | 'disconnecting' | 'error' | 'cleanup-failed';
  busy: boolean;                // true if hasOwnedSession || pendingCleanup || pendingConnect || state === 'cleanup-failed'
  hasOwnedSession: boolean;     // true if native resources are allocated/held
  host: string | null;          // Target IP/hostname currently connected or connecting
  generation: number;           // Monotonic generation counter (invalidates stale completions)
  lastError: string | null;     // Original connection or runtime error (preserved across cleanup failures)
  cleanupError: string | null;  // Teardown error message if disconnect failed (null on success)

  // Media & telemetry
  touchMode: 'direct' | 'trackpad';
  isMuted: boolean;
  renderedFrames: number;
  stats: SessionStats | null;
  hasNative: boolean;

  // Discovery & Saved Pairings
  discoveryState: 'idle' | 'loading' | 'error';
  discoveredHosts: DiscoveredHost[];
  discoveryLoading: boolean;
  discoveryError: string | null;
  selectedHost: SelectedHost | null;
  savedPairings: PairingSummary[];
  selectedPairing: SelectedPairing | null;
}
```

### 2.3 Public Methods Contract

```typescript
interface ConnectionManager {
  // State queries
  getState(): string;
  getGeneration(): number;
  snapshot(): ConnectionSnapshot;
  isCurrent(gen: number): boolean;

  // Connection Lifecycle
  connect(req: ConnectRequest): Promise<boolean>;
  disconnect(): Promise<void>;
  cancel(): Promise<void>;            // Alias to disconnect()
  retryCleanup(): Promise<void>;      // Retries teardown when state === 'cleanup-failed'
  dismissError(): Promise<void>;      // Cleans held resources before resetting state to idle

  // Media & Presentation
  markFrameRendered(gen: number): void;
  setTouchMode(mode: 'direct' | 'trackpad'): Promise<void>;
  setMuted(muted: boolean): Promise<void>;
  pollStats(): Promise<SessionStats | null>;

  // Discovery & Saved Credentials
  startDiscovery(): Promise<DiscoveredHost[]>;
  refreshHosts(): Promise<DiscoveredHost[]>;
  startPeriodicDiscovery(intervalMs?: number): void;
  stopPeriodicDiscovery(): void;
  pauseDiscovery(): void;
  resumeDiscovery(): void;
  selectDiscoveredHost(host: DiscoveredHost | null): void;
  getSelectedHost(): SelectedHost | null;
  connectSelectedHost(pin?: string | null): Promise<boolean>;
  dismissDiscoveryError(): void;

  refreshPairings(): Promise<PairingSummary[]>;
  forgetPairing(id: string): Promise<boolean>;
  selectPairing(pairing: PairingSummary | null): void;
  getSelectedPairing(): SelectedPairing | null;
  connectSavedPairing(pairingId?: string | null): Promise<boolean>;

  // Observer
  subscribe(fn: (snap: ConnectionSnapshot) => void): () => void;
}
```

### 2.4 State Transition Rules for Modal Owner

1. **User Initiates Connect:**
   - Pre-condition: `!isBusyOrLocked()` (`state === 'idle'` or `'error'` with `hasOwnedSession === false`).
   - Action: `manager.connect(req)`.
   - Transitions:
     - `state` -> `'connecting'`. `hasOwnedSession` -> `true`. `generation` increments.
     - Modal becomes visible; cancel button is interactive.
     - Native connect invoke is executed.
2. **User Clicks Cancel During Connect:**
   - Action: User clicks `#btn-cancel-connect`, invoking `manager.disconnect()` (or `manager.cancel()`).
   - Transitions:
     - `generation` increments immediately, invalidating pending connect.
     - `state` -> `'disconnecting'`. `#btn-cancel-connect` is disabled.
     - Native `invoke('disconnect')` is dispatched **promptly**.
     - Awaits native disconnect AND connect settlement. Cleanup does not settle prematurely.
     - If native disconnect succeeds: `state` -> `'idle'`, `hasOwnedSession` -> `false`. Modal hides. Final snapshot reports `busy: false`.
     - If native disconnect fails: `state` -> `'cleanup-failed'`, `hasOwnedSession` -> `true`. Error banner displayed.
3. **Connect Succeeds:**
   - `state` -> `'waiting-video'`. Modal updates text to "Connected — waiting for video...".
   - First frame rendered: `manager.markFrameRendered(gen)`. `state` -> `'streaming'`. Modal hides.
4. **Connect Fails (e.g. Pairing Denied, Timeout):**
   - Original error recorded in `lastError`.
   - Native cleanup executes automatically.
   - If cleanup succeeds: `state` -> `'error'`. `hasOwnedSession` -> `false`. Modal hides, error alert shown.
   - If cleanup fails: `state` -> `'cleanup-failed'`. `cleanupError` set. `hasOwnedSession` -> `true`. Connect blocked.
5. **Runtime Error During Streaming or Waiting-Video (e.g. TCP remote-closed, stats disconnected):**
   - Detected via `pollStats()` returning `{ state: "disconnected", last_error: "remote-closed" }` or error.
   - Leaves `waiting-video`/`streaming`. State transitions to `'error'` with `lastError = 'remote-closed'`.
   - `pollStats()` immediately initiates native cleanup without waiting for user action.
   - Once native disconnect settles, `hasOwnedSession` -> `false`, `busy` -> `false`, state remains `'error'`.
   - Subsequent `disconnect()` or `dismissError()` clears error and returns to `'idle'`.
6. **Cleanup Failure Recovery:**
   - When `state === 'cleanup-failed'`, `connect()` returns `false` immediately.
   - Modal owner exposes a Retry action invoking `manager.retryCleanup()` (or `#btn-dismiss-alert` calling `dismissError()`).
   - When native disconnect succeeds on retry: `hasOwnedSession` -> `false`, `cleanupError` -> `null`, `state` -> `'idle'`. Connect is unlocked.

---

## 3. Targeted Regression Evidence

### 3.1 RED Phase Execution (Lead Blocker & Mutation Regression Evidence)

Command:
```bash
bun test clients/rust/ios-shell/ui/test/connection-state.test.mjs clients/rust/ios-shell/ui/test/discovery.test.mjs
```

Log captured in `.omo/pairing-20260910/evidence/lifecycle-ui-red.log`:
```text
=== Phase D (R6) RED Test Execution Log ===
Timestamp: 2026-09-10
Task IDs: st_01a08a4b, st_01a08d7a
Command: bun test clients/rust/ios-shell/ui/test/connection-state.test.mjs clients/rust/ios-shell/ui/test/discovery.test.mjs
Runner: bun test v1.4.0 (34cbb9a40)

Lead Blockers & Comprehensive Lifecycle Mutation Targets Verified RED:

1. "lead_disconnect_waits_for_pending_connect"
   Expected: Cleanup must not settle and state remains 'disconnecting' with busy=true until connect settles.
   Actual: AssertionError: true !== false (Cleanup settled prematurely because of Promise.race([connectToSettle, Promise.resolve()]) bypass)

2. "lead_disconnected_stats_end_ui_session"
   Expected: Upon pollStats returning { state: "disconnected", last_error: "remote-closed" }, manager leaves waiting-video, retains error, executes native disconnect, and settles to 'error' with busy=false and hasOwnedSession=false.
   Actual: Timed out waiting for disconnect invoke because unpatched pollStats only checked state === 'error' and ignored native 'disconnected' terminal state.

3. "disconnect during pending connect: order B where disconnect settles before connect"
   Expected: When disconnect resolves first, cleanup does not complete while connect is still pending.
   Actual: AssertionError: true !== false (Cleanup finished prematurely before connect settled)

4. "final subscription snapshot reports busy=false when ownership and connect truly ended"
   Expected: Final snapshot emitted to subscribers reports busy=false.
   Actual: AssertionError: true !== false (Last emitted snapshot reported busy=true because emit() was called before pendingCleanup was cleared in finally)

5. Target 1 RED (Resource ownership in-flight retention):
   Test: "resource ownership: disconnect during streaming maintains owned session and busy until native invoke completes"
   Command: bun test clients/rust/ios-shell/ui/test/connection-state.test.mjs --test-name-pattern "resource ownership: disconnect during streaming maintains owned session and busy until native invoke completes"
   Mutation: Temporarily clear `hasOwnedSession = false` before native disconnect resolves in `disconnectInternal`.
   Actual Output:
     AssertionError: hasOwnedSession must remain true during in-flight disconnect
     false !== true
     (fail) resource ownership: disconnect during streaming maintains owned session and busy until native invoke completes [2.66ms]
     Exit code 1.

6. Target 2 RED (Concurrent cleanup promise sharing):
   Test: "concurrent disconnect, cancel, dismissError, and retryCleanup share identical in-flight cleanup promise"
   Command: bun test clients/rust/ios-shell/ui/test/connection-state.test.mjs --test-name-pattern "concurrent disconnect, cancel, dismissError, and retryCleanup share identical in-flight cleanup promise"
   Mutation: Temporarily bypass `pendingCleanup` cache in `disconnectInternal`.
   Actual Output:
     AssertionError: All concurrent cleanup callers must receive identical promise reference
     Promise { <pending> } !== Promise { <pending> }
     (fail) concurrent disconnect, cancel, dismissError, and retryCleanup share identical in-flight cleanup promise [4.69ms]
     Exit code 1.

7. Target 3 RED (Stale generation rejection isolation):
   Test: "stale connect rejection after disconnect cannot transition state to error or overwrite lastError"
   Command: bun test clients/rust/ios-shell/ui/test/connection-state.test.mjs --test-name-pattern "stale connect rejection after disconnect cannot transition state to error or overwrite lastError"
   Mutation: Temporarily bypass `currentGen !== generation` check in `connect` catch block.
   Actual Output:
     AssertionError: Stale connect rejection must not set lastError
     + actual - expected
     + 'late connection timeout'
     - null
     (fail) stale connect rejection after disconnect cannot transition state to error or overwrite lastError [2.30ms]
     Exit code 1.

8. Target 4 RED (Zero PIN fallback on saved pairing rejection):
   Test: "saved pairing connection rejection preserves exact failure reason without PIN fallback or duplicate retry"
   Command: bun test clients/rust/ios-shell/ui/test/connection-state.test.mjs --test-name-pattern "saved pairing connection rejection preserves exact failure reason without PIN fallback or duplicate retry"
   Mutation: Temporarily inject automatic fallback to PIN "12345678" upon saved pairing connect failure.
   Actual Output:
     AssertionError: Saved pairing failure must never trigger second connect attempt with PIN
     2 !== 1
     (fail) saved pairing connection rejection preserves exact failure reason without PIN fallback or duplicate retry [2.09ms]
     Exit code 1.
```

### 3.2 GREEN Phase Execution (Post-Implementation Verification)

Command:
```bash
bun test clients/rust/ios-shell/ui/test/connection-state.test.mjs clients/rust/ios-shell/ui/test/discovery.test.mjs
```

Log captured in `.omo/pairing-20260910/evidence/lifecycle-ui-green.log`:
```text
clients/rust/ios-shell/ui/test/discovery.test.mjs:
(pass) validateConnectRequest accepts valid custom tcpPort and udpPort [0.90ms]
(pass) validateConnectRequest omits port fields when absent for backward compatibility [0.03ms]
(pass) validateConnectRequest rejects empty bracketed hosts [0.03ms]
(pass) validateConnectRequest rejects port 0, negative, and out-of-range ports [0.05ms]
(pass) validateConnectRequest preserves and normalizes IPv6 addresses without bracket corruption [0.01ms]
(pass) safe port passing propagates to native connect invoke call [0.41ms]
(pass) null bridge connect cannot report success [0.07ms]
(pass) initial state has idle discovery and empty discovered hosts [0.03ms]
(pass) startDiscovery transitions discoveryState to loading while snapshot is pending [0.13ms]
(pass) pending list_hosts completes after stop and does not repopulate list [0.03ms]
(pass) stop before native call begins prevents native list_hosts from being invoked [0.02ms]
(pass) stop then resume waits for native stop before new list_hosts [0.03ms]
(pass) old completion does not clear new pending request [0.04ms]
(pass) list_hosts resolution populates discoveredHosts and transitions discoveryState to idle [0.05ms]
(pass) empty discovery snapshot yields empty list without error [0.02ms]
(pass) disappearing hosts are pruned from discoveredHosts on subsequent snapshot [0.05ms]
(pass) discovery failure sets discoveryState to error while preserving direct connect [0.13ms]
(pass) discovery refresh scheduling stops when active session begins and resumes on disconnect [0.28ms]
(pass) session/background disconnect does not restart periodic timer [0.07ms]
(pass) lifecycle background transitions halt discovery refresh and foreground transitions resume [0.10ms]
(pass) findDiscoveredHostById returns latest updated record when IP or ports change [0.03ms]
(pass) findDiscoveredHostById returns null when host is removed or inputs invalid
(pass) selecting a discovered host card populates target parameters without inferring paired state [0.03ms]
(pass) card-initiated connect requires PIN for unpaired host: asserts missing-PIN rejects and valid-PIN succeeds [0.06ms]
(pass) unpaired discovered host with identical name does not choose saved credential [0.03ms]

clients/rust/ios-shell/ui/test/connection-state.test.mjs:
(pass) validateConnectRequest validates required host and optional 8-digit PIN [0.02ms]
(pass) createConnectionManager transitions state from idle to waiting-video to streaming [0.05ms]
(pass) createConnectionManager ignores stale frame tokens from prior generation [0.03ms]
(pass) createConnectionManager reports error when native bridge is missing [0.02ms]
(pass) saved-mode click passes explicit pairingId to native connect without PIN [0.07ms]
(pass) list_pairings populates savedPairings and excludes secret keys from state [0.02ms]
(pass) connect without PIN and without pairingId returns pairing-required error [0.09ms]
(pass) UI saved-mode click to actual invoke argument passes exact pairingId without PIN [0.03ms]
(pass) startup provisioning auto-connect dispatches connect with explicit pairingId [0.04ms]
(pass) cleanup invokes native disconnect even when state is error [0.16ms]
(pass) cleanup failure transitions to cleanup-failed and does not revert to idle [0.04ms]
(pass) concurrent disconnect calls share single pending cleanup promise [0.03ms]
(pass) prompt native disconnect during pending connect cancels immediately and serializes cleanup [0.05ms]
(pass) cleanup-failed retains lock and blocks connect until retryCleanup succeeds [0.05ms]
(pass) connect rejection cleans up, retaining original error and retryable cleanup failure [0.03ms]
(pass) ensure error dismiss cleans resources and never reverts to idle on cleanup failure [0.04ms]
(pass) stale connect completion after disconnect cannot revive session [0.01ms]
(pass) lead_disconnect_waits_for_pending_connect [0.11ms]
(pass) lead_disconnected_stats_end_ui_session [0.03ms]
(pass) disconnect during pending connect: order A where connect settles before disconnect [0.04ms]
(pass) disconnect during pending connect: order B where disconnect settles before connect [0.04ms]
(pass) native stats contract: stale generation terminal stats is suppressed [0.06ms]
(pass) final subscription snapshot reports busy=false when ownership and connect truly ended [0.04ms]
(pass) resource ownership: disconnect during streaming maintains owned session and busy until native invoke completes [0.04ms]
(pass) concurrent disconnect, cancel, dismissError, and retryCleanup share identical in-flight cleanup promise [0.04ms]
(pass) stale connect rejection after disconnect cannot transition state to error or overwrite lastError [0.02ms]
(pass) saved pairing connection rejection preserves exact failure reason without PIN fallback or duplicate retry [0.05ms]

Result: 52 passed, 0 failed across 2 files (exit code 0, single-run execution in 235.00ms).
```

### 3.3 Desktop UI Suite Isolation Check

Command:
```bash
bun test clients/rust/tauri-shell/ui/
```
Result: **60 passed, 0 failed** across all 4 desktop files. Desktop implementation and behavior preserved untouched.

### 3.4 Full Mobile UI Suite Regression Check

Command:
```bash
bun test clients/rust/ios-shell/ui/test/
```
Result: **86 passed, 0 failed** across all 9 mobile test files.

---

## 4. Scope and Safety Boundaries

- **Touched Files Strictly Owned:**
  - `clients/rust/ios-shell/ui/connection-state.js`
  - `clients/rust/ios-shell/ui/test/connection-state.test.mjs`
  - (Read-only reference: `discovery.test.mjs` passing baseline)
  - `.omo/pairing-20260910/reports/d-ui.md`
  - `.omo/pairing-20260910/evidence/lifecycle-ui-red.log`
  - `.omo/pairing-20260910/evidence/lifecycle-ui-green.log`
- **Untouched Files:**
  - Zero edits to desktop source (`tauri-shell`), native Rust code (`src/*.rs`), `app.js`, `index.html`, or `styles.css`.
  - Zero git commits created.
  - Zero changes to model settings or configurations.
- **Physical Device Boundary:**
  - Verified local and mock IPC fixtures.
  - Physical iPhone device touch/gesture QA remains reserved for the coordinator/lead at final review.
