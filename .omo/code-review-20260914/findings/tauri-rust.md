# Lane: tauri-rust

## Scope reviewed

Read-only review of all 22 `#[tauri::command]` functions and their state, worker, serialization, and teardown paths. Line counts are whole-file counts; imported files were read only in the indicated relevant portions. Sibling tests were read for context, not changed or rerun. The lead's successful Cargo check and Bun test baseline was accepted without rerunning it.

| File | Lines | Coverage |
| --- | ---: | --- |
| `clients/rust/tauri-shell/src-tauri/src/lib.rs` | 2542 | Entire file, including inline test context |
| `clients/rust/tauri-shell/src-tauri/src/main.rs` | 5 | Entire file |
| `clients/rust/tauri-shell/src-tauri/src/desktop_integration_tests.rs` | 544 | Entire file, context only |
| `clients/rust/tauri-shell/src-tauri/src/discovery_tests.rs` | 329 | Entire file, context only |
| `clients/rust/tauri-shell/src-tauri/src/mailbox_tests.rs` | 260 | Entire file, context only |
| `clients/rust/tauri-shell/src-tauri/src/pairing_tests.rs` | 775 | Entire file, context only |
| `clients/rust/tauri-shell/src-tauri/src/recovery_tests.rs` | 256 | Entire file, context only |
| `clients/rust/maho-app/src/agent_input.rs` | 1430 | Lines 1-918: action validation/conversion, tracker, screenshot encoder |
| `clients/rust/maho-app/src/input.rs` | 142 | Entire file: pointer and key conversion |
| `clients/rust/maho-app/src/pairing.rs` | 1227 | Lines 258-497: default path, load/save/delete, endpoint persistence |
| `clients/rust/maho-app/src/session.rs` | 1734 | Lines 202-386, 603-761, 961-1101, 1220-1379: session ownership, transport, cursor ingress, runtime mailbox/stop |
| `clients/rust/maho-app/src/media.rs` | 584 | Lines 1-120, 325-364: assembly admission and cursor state |
| `clients/rust/maho-app/src/frame_queue.rs` | 339 | Lines 1-240: bounded queue, stop notification, recovery |
| `clients/rust/maho-decode/src/lib.rs` | 740 | Lines 170-459, 490-561: decoder output layout and backend selection |
| `clients/rust/maho-decode/src/vt/mod.rs` | 600 | Lines 76-152: native NV12 output layout |
| `clients/rust/maho-render/src/audio.rs` | 623 | Lines 1-270: PCM bounds, gain validation, device API, callback state |
| `clients/rust/maho-host/src/lib.rs` | 56 | Entire file: directly used host exports |
| `clients/rust/maho-host/src/session.rs` | 5194 | Lines 2083-2524, 2580-2770: serve/stop contract, consent, authenticated connection loop |

## Findings

### [P0] Stopping an active host waits indefinitely without stopping its connection
- **Location**: `clients/rust/tauri-shell/src-tauri/src/lib.rs:784` (secondary: `clients/rust/tauri-shell/src-tauri/src/lib.rs:1965`; `clients/rust/maho-host/src/session.rs:2094`, `clients/rust/maho-host/src/session.rs:2261`, `clients/rust/maho-host/src/session.rs:2702`)
- **Evidence**:
```rust
    pub fn stop_host(&self) -> Result<HostStatus, String> {
        self.host_runtime.stop_flag.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.host_runtime.thread_handle.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
        self.host_runtime.running.store(false, Ordering::SeqCst);
        self.get_host_status()
    }
```
```rust
    #[tauri::command]
    pub fn stop_host(state: State<'_, AppState>) -> Result<HostStatus, String> {
        state.stop_host()
    }
```
```rust
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            match self.tcp_listener.accept() {
                Ok((tcp, peer)) => {
                    let _ = tcp.set_nonblocking(false);
                    let admission_deadline = std::time::Instant::now() + self.preauth_timeout;
                    tcp.set_nodelay(true)?;
                    let tls_server = maho_net::tls_psk::TlsPskServer::new(self.current_psks()?)?;
                    match tls_server.accept_stream_until(tcp, admission_deadline) {
                        Ok(stream) => {
                            if let Err(error) =
                                self.handle_connection(stream, peer, admission_deadline)
```
- **Impact**: With a previously authorized remote client connected, invoking `stop_host` joins the server thread, but the stop token is checked only in the outer accept loop. `handle_connection` does not receive that token; its authenticated loop remains live as long as the peer answers heartbeats. Consequently the stop IPC never completes, the synchronous command blocks its invocation thread, and screen streaming/input access continue despite the stop request. The same join exists in `HostRuntime::drop`, so shutdown can also hang. A cooperative or malicious authorized peer can keep this condition alive indefinitely; this is not merely the bounded preauthentication timeout.
- **Fix**: Pass host cancellation into the active connection loop and make cancellation interrupt its socket reads/writes and stop its media workers. Make the Tauri stop command asynchronous and move joining onto a blocking worker, while retaining serialized host lifecycle ownership. Moving only the join off-thread does not fix the connection that never stops.
- **Confidence**: high

### [P1] The shell exposes a pairing PIN but cannot approve any new client
- **Location**: `clients/rust/tauri-shell/src-tauri/src/lib.rs:194` (secondary: `clients/rust/tauri-shell/src-tauri/src/lib.rs:739`, `clients/rust/tauri-shell/src-tauri/src/lib.rs:2212`; `clients/rust/maho-host/src/session.rs:2502`)
- **Evidence**:
```rust
            auto_approve: Arc::new(AtomicBool::new(false)),
```
```rust
        let auto_approve = self.host_runtime.auto_approve.clone();
        let (consent_tx, consent_rx) = std::sync::mpsc::channel::<maho_host::ConsentPrompt>();
        let _ = std::thread::Builder::new()
            .name("maho-host-consent".into())
            .spawn(move || {
                while let Ok(prompt) = consent_rx.recv() {
                    let approved = auto_approve.load(Ordering::SeqCst);
                    if approved {
                        tracing::info!(client = %prompt.client_name, "Pairing request auto-approved by host policy");
                        prompt.respond(true);
                    } else {
                        tracing::warn!(client = %prompt.client_name, "Pairing request denied: host auto_approve disabled");
                        prompt.respond(false);
                    }
                }
            });
```
- **Impact**: A clean installation starts its host and returns a valid eight-digit PIN in host status, but even a client entering that PIN is immediately rejected with `DeniedByHost`. The application creates its own default state in `run`; none of the 22 registered commands changes `auto_approve` or delivers a consent prompt for a user decision. The flag is only initialized false and read. Existing host authorizations can reconnect, but first-time authorization through this desktop application is impossible.
- **Fix**: Deliver each pending consent request to the local UI with an identifier and add a command that approves/rejects that specific request, with expiry and one-shot ownership. If an explicit auto-approval preference is intended instead, expose a deliberate user-controlled setting for it. Do not solve the missing consent route by silently changing the default to allow all PIN holders.
- **Confidence**: high

### [P1] Audio device enumeration is permanently frozen after the first list
- **Location**: `clients/rust/tauri-shell/src-tauri/src/lib.rs:332` (secondary: `clients/rust/tauri-shell/src-tauri/src/lib.rs:1672`, `clients/rust/tauri-shell/src-tauri/src/lib.rs:1698`)
- **Evidence**:
```rust
    fn devices(&mut self) -> Result<Vec<DesktopAudioDevice>, String> {
        if self.devices.is_none() {
            self.devices = Some(CpalAudioOutput::output_devices().map_err(|e| e.to_string())?);
        }
        self.devices
            .as_ref()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, device)| {
                Ok(DesktopAudioDevice {
                    id: format!("output-{index}"),
                    name: device.name().map_err(|e| e.to_string())?,
                    supported: device.supports_pcm().map_err(|e| e.to_string())?,
                })
            })
            .collect()
    }
```
```rust
    #[tauri::command]
    pub async fn list_audio_devices(
        state: State<'_, AppState>,
    ) -> Result<DesktopAudioStatus, String> {
        let _lifecycle = state.lifecycle.lock().await;
        state.audio_request(AudioAction::List).await
    }
```
- **Impact**: Open the audio device list, then attach a USB/Bluetooth output or replace an unplugged device. Every later `list_audio_devices` command reuses the retained vector, so the newly available output never appears and cannot be selected by ID. The audio worker/backend survives disconnect and reconnect, so reconnecting does not refresh the list either; restarting the application is required for explicit selection of the new device. A removed retained handle can additionally make name/config queries reject the entire list, preventing normal device-recovery UI from listing surviving outputs.
- **Fix**: Re-enumerate on `AudioAction::List` and maintain non-reused process-local tokens for the resulting handles. Preserve the identity of existing explicit selections, invalidate removed tokens, and assign fresh tokens to new devices; do not reuse vector indices for a different physical output.
- **Confidence**: high

### [P2] Odd-height frames lose a chroma row and move cursor bytes into pixel data
- **Location**: `clients/rust/tauri-shell/src-tauri/src/lib.rs:1418` (secondary: `clients/rust/tauri-shell/src-tauri/src/lib.rs:1436`, `clients/rust/tauri-shell/src-tauri/src/lib.rs:1934`; `clients/rust/maho-decode/src/lib.rs:431`; `clients/rust/maho-app/src/agent_input.rs:69`)
- **Evidence**:
```rust
                            let width = nv12.width as usize;
                            let height = nv12.height as usize;
                            let y_len = width * height;
                            let uv_len = width * (height / 2);
                            let total_bytes = 16 + y_len + uv_len + 9;
```
```rust
                            if nv12.uv_plane.len() >= uv_len {
                                buffer.extend_from_slice(&nv12.uv_plane[..uv_len]);
                            } else {
                                buffer.extend_from_slice(&nv12.uv_plane);
                                buffer.resize(16 + y_len + uv_len, 128);
                            }

                            let cursor =
                                decode_cursor_target.lock().map(|c| *c).unwrap_or_default();
                            buffer.extend_from_slice(&cursor.x.to_le_bytes());
                            buffer.extend_from_slice(&cursor.y.to_le_bytes());
                            buffer.push(cursor.cursor_type);
```
```rust
        let mut y_plane = vec![0_u8; width * height];
        let mut uv_plane = vec![0_u8; width * height.div_ceil(2)];
```
```rust
        encode_nv12_screenshot(frame.width, frame.height, &frame.buffer[16..], fmt)
```
- **Impact**: Both decoder adapters retain `ceil(height / 2)` chroma rows, but this IPC packer truncates to `floor(height / 2)` without rejecting odd dimensions. It still advertises the original height and immediately appends the cursor trailer. Thus an odd-height decoded frame has missing chroma data, and consumers treating the declared dimensions as a complete NV12 image can read cursor metadata as chroma or reject the short buffer. The screenshot path passes that trailer to the pixel converter too: a 16x17 frame has 144 decoder UV bytes but only 128 packed UV bytes; its 409-byte body including cursor passes the converter's 408-byte minimum check, yet its last UV index is 415. That causes an indexing panic inside the screenshot worker, surfaced as a failed capture. The arithmetic witness was checked with a read-only Python command; no odd-dimension codec fixture or frontend runtime was executed.
- **Fix**: Enforce a documented nonzero, even-width/even-height NV12 contract before publication and return a decode error for unsupported dimensions, or implement an explicit rounded chroma layout consistently in packing and consumers. Pass only the validated pixel span, excluding the nine-byte cursor trailer, to `encode_nv12_screenshot` and make its bounds check match the actual chroma indexing.
- **Confidence**: high

### [P2] Cursor state from the previous host survives disconnect and reconnect
- **Location**: `clients/rust/tauri-shell/src-tauri/src/lib.rs:658` (secondary: `clients/rust/tauri-shell/src-tauri/src/lib.rs:1443`, `clients/rust/tauri-shell/src-tauri/src/lib.rs:1487`, `clients/rust/tauri-shell/src-tauri/src/lib.rs:2207`)
- **Evidence**:
```rust
    pub fn clear_metrics(&self) {
        self.frames_received.store(0, Ordering::Relaxed);
        self.frames_decoded.store(0, Ordering::Relaxed);
        self.audio_packets_received.store(0, Ordering::Relaxed);
        if let Ok(mut lat) = self.latency.lock() {
            lat.clear();
        }
        if let Ok(mut frame) = self.latest_raw_frame.lock() {
            *frame = FrameMailbox::default();
        }
    }
```
```rust
                            let cursor =
                                decode_cursor_target.lock().map(|c| *c).unwrap_or_default();
                            buffer.extend_from_slice(&cursor.x.to_le_bytes());
                            buffer.extend_from_slice(&cursor.y.to_le_bytes());
                            buffer.push(cursor.cursor_type);
```
```rust
    #[tauri::command]
    pub fn get_cursor_position(state: State<'_, AppState>) -> Result<CursorState, String> {
        state
            .latest_cursor
            .lock()
            .map(|c| *c)
            .map_err(|e| e.to_string())
    }
```
- **Impact**: After host A publishes a cursor, disconnect clears frames and counters but never resets `latest_cursor`. The cursor command continues returning A's coordinates/type while disconnected. If host B produces video before its first cursor update, those old coordinates/type are also packed into B's new frame trailer; this can persist if B does not send cursor updates. The initial hidden cursor sentinel (`CursorState::default()` uses -1 coordinates) is never restored at session boundaries.
- **Fix**: Reset `latest_cursor` to `CursorState::default()` after the old media worker has joined and before a new worker starts. Keep that reset inside the serialized lifecycle teardown, not before joining, because an in-flight old UDP cursor event can otherwise overwrite it.
- **Confidence**: high

## Non-findings checked

- All 22 command annotations have matching entries in the invoke handler; `main.rs` delegates directly to the reviewed `run` function.
- The normal even-dimension frame layout is 16 header bytes, `width * height` Y bytes, `width * (height / 2)` UV bytes, then exactly nine cursor bytes (two little-endian f32 values and a u8); the producer initializes each published byte.
- `poll_frame_raw` is binary IPC, not a truly zero-copy Rust path: the mailbox shares an `Arc`, but the response explicitly clones `p.buffer` after releasing the mailbox lock. No borrowed buffer escapes its owner.
- `encode_capture`'s `[16..]` header removal is safe for frames created by the production publisher, which always writes the full header; short synthetic test payloads do not establish a production panic.
- No synchronous `std::sync::MutexGuard` is held across an await in the reviewed command paths. The lifecycle guard is an asynchronous Tokio mutex intentionally held across session operations.
- Input submission and teardown consistently acquire the outer session lock before the agent tracker and position locks; the media/frame paths do not acquire those locks in reverse order.
- `send_input` and `agent_execute_action` check `stop_media_flag` while owning the session lock. A submission already in flight precedes teardown's reset rather than sending a new key-down after that reset.
- Media publication and cursor dispatch can finish an in-flight event after the stop flag changes, but normal teardown joins media before clearing frames and before releasing the lifecycle lock. Missing per-write flag checks alone therefore do not prove a stale frame survives completed teardown; the distinct cursor-reset defect is reported above.
- Teardown attempts input reset, audio stop, transport disconnect, TCP runtime stop, and media join, then aggregates their errors instead of abandoning later cleanup after the first worker failure.
- The TCP event mailbox is bounded and the frame poll consumes at most four observations per call; error/closed-runtime results reject IPC rather than silently serving a stale frame.
- PIN validation requires exactly eight ASCII digits after trimming; reconnect selects a saved record by exact ID and does not silently fall back to bootstrap authorization after a transport failure.
- `forget_pairing` uses a fixed application-data store, and the supplied ID is compared against record IDs, not interpolated into a filesystem path. The reviewed commands expose no arbitrary path read/write operation.
- `connect` intentionally accepts a caller-selected network endpoint, but session setup still authenticates through the PIN or saved PSK path. No command accepts an arbitrary shell program or shell fragment; Tailscale uses fixed arguments and a five-second timeout with child termination on drop.
- Pointer payloads reject nonfinite fields and unknown event kinds; supported keys are mapped explicitly; bitrate clamps to 1-300 Mbps without integer overflow.
- Agent click, drag, and text expansion is capped by the imported converter's 4096-event budget; audio volume is validated as finite and within 0..=1 even without an active output.
- PCM admission checks stereo alignment and finiteness before mutating a queue whose capacity is fixed at 100 ms; device stop detaches its producer queue before releasing the output.
- Production `unwrap` calls on initialized audio option slots follow explicit initialization; private mutex poison assumptions were not reported without a concrete reachable poisoning input. Test-only unwraps were not treated as findings.
- Verification was source/contract analysis plus an arithmetic witness, not a new build, native GUI exercise, or codec/network reproduction. Frontend source was outside the permitted read scope and was not inspected.
