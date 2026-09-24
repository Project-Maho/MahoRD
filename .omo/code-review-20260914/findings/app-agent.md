# Lane: app-agent
## Scope reviewed
- `clients/rust/maho-app/src/agent_server.rs`: 2,237 lines, read in full.
- `clients/rust/maho-app/src/agent_input.rs`: 1,430 lines, read in full.
- `clients/rust/maho-app/src/mcp_server.rs`: 227 lines, read in full.
- `clients/rust/maho-app/src/mcp_dispatch.rs`: 163 lines, read in full.
- `clients/rust/maho-app/src/mcp_tests.rs`: 302 lines, read in full.
- Direct dependencies: `clients/rust/maho-app/src/input.rs` (142 lines), `clients/rust/maho-app/src/mcp_stdio.rs` (116 lines), and `clients/rust/maho-app/src/mcp_dispatch_contract_tests.rs` (119 lines), read in full.
- Method: source-level control-flow and failure-path review, rather than modifying the repository to add a harness. The supplied successful compilation/test baseline was not rerun. Deployment callers are outside the permitted scope; the actual port-19735 bind address and token configuration are not established by this review.

## Findings

### [P0] MCP stdin can grow a request buffer without limit
- **Location**: `clients/rust/maho-app/src/mcp_server.rs:179` (unbounded read at line 188)
- **Evidence**:
```rust
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    let mut tracker = InputStateTracker::default();
    let mut current_pos = (0.5, 0.5);

    let work = async {
        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line).await?;
```
- **Impact**: An MCP stdio peer can stream arbitrarily many non-newline bytes. `read_line` continuously appends them before JSON parsing or the action-event budget runs, so this long-lived process has unbounded memory growth and can terminate on allocation failure. A completed huge line also leaves its allocation retained by `String::clear`. This is a stdio-peer issue, not a demonstrated unauthenticated HTTP attack; P0 follows the supplied unbounded-resource-growth rule.
- **Fix**: Bound each input record before appending more bytes, using a capped incremental line reader. On exceeding the limit, terminate with an explicit error or discard through the next newline without retaining the oversized record. Do not implement the limit only after `read_line` returns.
- **Confidence**: high

### [P1] Partial HTTP input dispatch can leave an untracked key or button held
- **Location**: `clients/rust/maho-app/src/agent_server.rs:653`; `clients/rust/maho-app/src/agent_input.rs:810`
- **Evidence**:
```rust
                        for event in events {
                            if let Err(err) = backend.send_input_event(event) {
                                return (
                                    500,
                                    "Internal Error",
                                    serde_json::json!({"ok": false, "error": err}),
                                );
                            }
                            total_events += 1;
                        }
```
```rust
        AgentAction::KeyPress { key, .. } => {
            let (k, mods) = parse_key_name(key)?;
            let macos_code = crate::input::InputKeyMap::to_macos(k).unwrap_or(0);
            events.push(InputEvent {
                event_type: InputEventType::KeyDown,
                x: current_pos.0,
                y: current_pos.1,
                key_code: macos_code,
                modifiers: mods,
                scroll_dx: 0.0,
                scroll_dy: 0.0,
            });
            events.push(InputEvent {
                event_type: InputEventType::KeyUp,
                x: current_pos.0,
                y: current_pos.1,
                key_code: macos_code,
                modifiers: mods,
                scroll_dx: 0.0,
                scroll_dy: 0.0,
            });
        }
```
- **Impact**: For `{"action":"key_press","key":"a"}`, let the down send succeed and the up send fail. HTTP immediately returns 500 without attempting Reset. Conversion never records this key in the tracker, so the idle watchdog sees an empty tracker and never releases it. Clicks, hotkeys and text have the same partial-send problem; drag conversion removes its button before transmission. The remote session can continue with a held key/button after a transient backend failure.
- **Fix**: Track successfully dispatched downs until their corresponding ups succeed. On any send failure, attempt all releases and a trailing Reset, preserving conservative held state if cleanup fails. Reuse the explicit reset path's retry behavior rather than merely returning the first send error.
- **Confidence**: high

### [P1] The auto-release watchdog forgets held input when release transmission fails
- **Location**: `clients/rust/maho-app/src/agent_server.rs:184`; `clients/rust/maho-app/src/agent_input.rs:578` (destructive release implementation at lines 536-576)
- **Evidence**:
```rust
                    if let Some(events) = t.check_timeout(pos.0, pos.1) {
                        if !events.is_empty() {
                            for event in events {
                                let _ = watchdog_backend.send_input_event(event);
                            }
                        }
                    }
```
```rust
    pub fn check_timeout(&mut self, current_x: f32, current_y: f32) -> Option<Vec<InputEvent>> {
        if !self.is_empty() && self.last_action_at.elapsed() > self.hold_timeout {
            Some(self.release_all(current_x, current_y))
        } else {
            None
        }
    }
```
- **Impact**: `release_all` drains active keys/buttons and clears modifiers before returning its events. If the backend rejects the timeout-generated key-up/button-up and Reset during a transient outage, the watchdog discards every error and leaves the tracker empty. When transmission recovers, later ticks cannot retry; remote input can remain held indefinitely. This also defeats the retry state restored by the explicit HTTP reset helper if its eventual watchdog retry fails.
- **Fix**: Separate timeout detection from destructive release, then use the error-aware release transaction that retains held state when Reset fails. Log or propagate release failures and retry on a bounded cadence. Keep backend sends off the async executor as the normal HTTP input path does.
- **Confidence**: high

### [P1] External HTTP-server shutdown aborts the only held-input watchdog without releasing input
- **Location**: `clients/rust/maho-app/src/agent_server.rs:233` (shutdown branch at line 201; watchdog owned by the aborted JoinSet at line 173)
- **Evidence**:
```rust
        work.input_closed.store(true, Ordering::SeqCst);
        drop(listener);
        connections.abort_all();
        while let Some(joined) = connections.join_next().await {
            log_connection_result(Some(joined));
        }
        // Blocking operations cannot be forcibly cancelled. Drain their permits
        // before backend stop, or return an error rather than claiming a join.
        let drained =
            tokio::time::timeout(WORK_TIMEOUT, work.jobs.clone().acquire_many_owned(MAX_WORK))
                .await
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?
                .map_err(std::io::Error::other)?;
        drop(drained);
        if session_disconnect_requested {
            work.blocking(move || backend.try_disconnect_session())
                .await?
                .map_err(std::io::Error::other)?;
        }
        Ok(())
```
- **Impact**: Send a successful `mouse_down` or `key_down`, then stop `AgentServer::run` using its shutdown watch channel (or drop the last shutdown sender). Unlike `/session/disconnect`, this path never sends releases or Reset. It aborts the watchdog and returns success after draining jobs. When the caller retains the backend/session, including the supported generic non-disconnecting backend case, the remote input remains held with no server watchdog left to release it.
- **Fix**: After gating input, stopping connections and draining in-flight dispatches, release the tracker through the error-aware release helper on every shutdown path. Only then optionally stop a backend-owned session. Report cleanup failure rather than returning `Ok(())` with unreleased state.
- **Confidence**: high

### [P1] Key-up never removes its modifier from subsequent input events
- **Location**: `clients/rust/maho-app/src/agent_input.rs:511`; secondary key-up emission at line 796
- **Evidence**:
```rust
    pub fn record_key_down(&mut self, key_code: u16, modifiers: Modifiers) {
        self.active_keys.insert(key_code);
        self.active_modifiers |= modifiers;
        self.last_action_at = Instant::now();
    }

    pub fn record_key_up(&mut self, key_code: u16) {
        self.active_keys.remove(&key_code);
        self.last_action_at = Instant::now();
    }
```
```rust
        AgentAction::KeyUp { key } => {
            let (k, _) = parse_key_name(key)?;
            let macos_code = crate::input::InputKeyMap::to_macos(k).unwrap_or(0);
            tracker.record_key_up(macos_code);
            events.push(InputEvent {
                event_type: InputEventType::KeyUp,
                x: current_pos.0,
                y: current_pos.1,
                key_code: macos_code,
                modifiers: tracker.active_modifiers,
                scroll_dx: 0.0,
                scroll_dy: 0.0,
            });
        }
```
- **Impact**: A normal sequence `key_down(ctrl)`, `key_up(ctrl)`, `key_down(c)` still emits CONTROL on `c`, turning a plain key into Ctrl+C. Mouse actions likewise inherit released modifiers. The bits are only cleared by reset/release-all; further key/button actions refresh the timer and can prolong the wrong modifier state indefinitely.
- **Fix**: Associate modifier contributions with held keys and recompute the active modifier mask when a key is released. Emit key-up and subsequent events using the post-release mask, retaining modifiers still owned by other held keys.
- **Confidence**: high

### [P1] Advertised function keys silently turn into the A key
- **Location**: `clients/rust/maho-app/src/agent_input.rs:313` and line 810; `clients/rust/maho-app/src/input.rs:76` (A mapping at line 16)
- **Evidence**:
```rust
        "f1" => 0x70,
        "f2" => 0x71,
        "f3" => 0x72,
        "f4" => 0x73,
        "f5" => 0x74,
        "f6" => 0x75,
        "f7" => 0x76,
        "f8" => 0x77,
        "f9" => 0x78,
        "f10" => 0x79,
        "f11" => 0x7A,
        "f12" => 0x7B,
```
```rust
        AgentAction::KeyPress { key, .. } => {
            let (k, mods) = parse_key_name(key)?;
            let macos_code = crate::input::InputKeyMap::to_macos(k).unwrap_or(0);
```
```rust
            0x70 => 0x7a,
            0x25 => 0x7b,
            0x27 => 0x7c,
            0x28 => 0x7d,
            0x26 => 0x7e,
            0x24 => 0x73,
            0x23 => 0x77,
            0x21 => 0x74,
            0x22 => 0x79,
            0x2e => 0x75,
            _ => return None,
```
- **Impact**: Parsing accepts F2-F12, but the shared mapper only implements F1. `remote_key_press` with `key: "F5"`, for example, sends macOS key code 0 (A) and reports success rather than refreshing the remote application. HTTP key-down/up and both interfaces' hotkeys use the same fallback, so a requested modifier+function-key shortcut can instead execute a modifier+A shortcut.
- **Fix**: Add the accepted function-key mappings to `InputKeyMap::to_macos`, and replace `unwrap_or(0)` with an explicit unmappable-key error in all agent action variants. A valid A key code must never serve as an error sentinel.
- **Confidence**: high

### [P1] MCP screen-change waits block cancellation on the async executor
- **Location**: `clients/rust/maho-app/src/mcp_dispatch.rs:91`; `clients/rust/maho-app/src/mcp_server.rs:198` and line 209
- **Evidence**:
```rust
                    let start = std::time::Instant::now();
                    let poll_interval = std::time::Duration::from_millis(25);
                    loop {
                        if let Some(meta) = backend.get_latest_frame_metadata() {
                            if last_frame_id.is_none() || meta.frame_id != last_frame_id.unwrap() {
                                break Ok(json!([{"type":"text","text":format!("Screen changed: frame_id={}, timestamp_ms={}, age_ms={}", meta.frame_id, meta.timestamp_ms, meta.age_ms)}]));
                            }
                        }
                        if start.elapsed() >= std::time::Duration::from_millis(timeout_ms) {
                            break Ok(json!([{"type":"text","text":format!("Timeout: no change detected (last_frame_id={})", last_frame_id.unwrap_or(0))}]));
                        }
                        std::thread::sleep(poll_interval);
                    }
```
```rust
            if let Some(resp) =
                handle_mcp_message(trimmed, backend.clone(), &mut tracker, &mut current_pos)
```
```rust
    let result = tokio::select! {
        biased;
        _ = stop.wait_for(|stopped| *stopped) => Ok(()),
        result = work => result,
    };
```
- **Impact**: With no new frame, a normal `remote_wait_for_screen_change` call sleeps synchronously for five seconds by default, or up to 30 seconds from its argument. `handle_mcp_message` runs inline inside the async `work` future and never yields during this loop, so the surrounding select cannot observe shutdown or run final input cleanup until the wait finishes. On a current-thread runtime it also stalls every other task on that runtime, including any task that would deliver the awaited metadata update.
- **Fix**: Make the screen-change operation asynchronous and cancellation-aware, awaiting a frame notification or an async timer together with the stop signal. Do not call this synchronous sleeping loop from the stdio async task; merely adding an outer Tokio timeout cannot preempt it.
- **Confidence**: high

### [P2] NV12 length validation admits dimensions that panic during chroma lookup
- **Location**: `clients/rust/maho-app/src/agent_input.rs:69`
- **Evidence**:
```rust
pub fn nv12_to_rgb(width: u32, height: u32, nv12_buf: &[u8]) -> Option<Vec<u8>> {
    let w = width as usize;
    let h = height as usize;
    let expected_len = w * h * 3 / 2;
    if nv12_buf.len() < expected_len || w == 0 || h == 0 {
        return None;
    }

    let mut rgb = Vec::with_capacity(w * h * 3);
    let uv_plane_start = w * h;

    for y in 0..h {
        let y_row_start = y * w;
        let uv_row_start = uv_plane_start + (y / 2) * w;
        for x in 0..w {
            let y_val = nv12_buf[y_row_start + x] as f32;
            let uv_offset = uv_row_start + (x / 2) * 2;
            let u_val = nv12_buf[uv_offset] as f32 - 128.0;
            let v_val = nv12_buf[uv_offset + 1] as f32 - 128.0;
```
- **Impact**: For `(width, height, buffer length) = (2, 1, 3)`, the advertised size check passes, but the first pixel reads V at index 3 and panics. Odd widths also invalidate the assumed interleaved chroma stride. `encode_nv12_screenshot` promises an error for invalid input but instead unwinds: HTTP loses the screenshot job/response, and MCP dispatch has no panic boundary. Whether production decoders can supply these dimensions was not established within scope, so this is an edge-condition validation finding, not a claimed remotely exploitable crash. The unchecked products also lack overflow handling for extreme dimensions.
- **Fix**: For this tightly packed NV12 contract, reject odd width/height and use checked multiplication/addition for plane lengths and RGB allocation before indexing. If odd dimensions must be supported, carry explicit plane strides and validate the complete rounded-up chroma extent instead.
- **Confidence**: high

## Non-findings checked
- With a configured token, every input route (action, batch, reset, disconnect) checks authentication before backend input dispatch; screen info, screenshot and wait-change routes also check it.
- `X-Maho-Token` header names are matched case-insensitively and values must equal the configured token; `Authorization: Bearer` is an alternative. No configured-token bypass was found.
- `/health` and `/api/v1/health` deliberately omit auth, but return only a fixed status and never inject input. Unknown routes likewise do not dispatch input.
- Authentication is optional in the public server constructors (`None` permits requests), and binding uses the caller's `SocketAddr` without enforcing loopback. This is not proof that the deployed port-19735 listener is public or tokenless: no permitted file contains that production caller. Deployment authentication/binding remains unverified, rather than an asserted P0 bypass.
- The reviewed listener implements HTTP with one request per connection and `Connection: close`; it has no WebSocket upgrade/dispatch path. A separate WS listener, if any, is outside this scope.
- HTTP framing caps headers plus body at 65,536 bytes, uses a five-second absolute read deadline, rejects duplicate Content-Length and transfer encoding, and handles parse failures without JSON unwraps.
- UTF-8 validation of HTTP headers occurs before the lossy whole-request view is used, so invalid body bytes do not shift the earlier header/body delimiter offset into an out-of-bounds slice.
- HTTP connection tasks and blocking work are bounded, screenshot work has a one-job semaphore, and running blocking jobs retain permits after the waiter is cancelled.
- Explicit successful session disconnect gates new input, serializes with existing input transactions, attempts release before the response, closes the listener after the bounded response attempt, drains blocking work, and then invokes backend stop.
- Explicit HTTP reset attempts every release including Reset, reports send errors, and restores conservative tracker state when Reset fails; the watchdog does not share that protection, as reported above.
- MCP JSON parsing and envelope/argument deserialization return protocol errors rather than panicking; notifications do not invoke tools. Stdio itself is the local trust boundary, not an HTTP route missing an X-Maho-Token check.
- MCP EOF, read/write error and stop completion paths attempt cleanup and surface cleanup failure; the synchronous screen-change operation delays reaching that cleanup, as reported above.
- Hotkey parsing rejects empty/unknown input without an unchecked final-key unwrap; `parse_key_name` only unwraps a character after proving byte length is one. No arbitrary-string parse panic was found.
- Coordinates are checked for finiteness and clamped; click/drag/text expansion is rejected above the 4,096-event per-action budget before tracker mutation.
- The MCP partial-dispatch contract test uses a backend that accepts mouse-down and rejects mouse-up, then asserts a later accepted Reset; it can detect absence of cleanup. Test-only unwraps were not treated as production defects.
