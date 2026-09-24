# Lane: host-session

## Scope reviewed

Current working-tree content was reviewed, including the uncommitted changes. No source files were modified.

- `clients/rust/maho-host/src/session.rs`: 5,194 lines, read in full, including inline tests.
- `clients/rust/maho-host/src/lib.rs`: 53 lines, read in full.
- `clients/rust/maho-host/src/main.rs`: 301 lines, read in full.
- `clients/rust/maho-host/src/host_trace.rs`: 92 lines, read in full.
- Direct dependency `clients/rust/maho-host/src/native_pipeline.rs`: 714 lines, read in full.
- Direct dependency `clients/rust/maho-host/src/encode_vt.rs`: 722 lines, read in full.
- Direct dependency `clients/rust/maho-host/src/inject_macos.rs`: 688 lines, read in full.
- Direct dependency `clients/rust/maho-host/src/inject_windows.rs`: 294 lines, read in full.
- Direct dependency `clients/rust/maho-host/src/inject_linux.rs`: 688 total lines; read lines 303-602 (300 lines) and searched teardown sites.
- Direct dependency `clients/rust/maho-net/src/tls_psk.rs`: 972 total lines; read lines 240-304 and 420-624 (270 lines), covering deadline-based acceptance and framed reads/writes.

This is a static review. The lead-provided passing compilation/test baselines were not rerun. Windows-specific socket behavior and native input effects were not exercised on this macOS workstation. Source-path tracing was chosen over a synthetic reproduction because these findings concern actual orchestration and platform boundaries that mocks could hide.

## Findings

### [P0] Oversized unauthenticated UDP packets disconnect Windows sessions
- **Location**: `clients/rust/maho-host/src/session.rs:2775` (secondary sites: `clients/rust/maho-host/src/session.rs:2798`, `clients/rust/maho-host/src/session.rs:2275`)
- **Evidence**:
```rust
        let mut buffer = [0_u8; 2_048];
        for _ in 0..MAX_UDP_DISCOVERY_BURST {
            match self.udp_socket.recv_from(&mut buffer) {
                Ok((length, peer)) if peer.ip() == tcp_peer.ip() => {
```
```rust
                Err(error)
                    if error.kind() == io::ErrorKind::ConnectionReset
                        || error.raw_os_error() == Some(10054) =>
                {
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(SessionError::Io(error)),
```
```rust
                    self.discover_udp_peer(tcp_peer, &mut udp_peer, c2h_cipher.as_mut())?;
```
- **Impact**: On Windows, receiving a UDP datagram larger than the supplied buffer produces WSAEMSGSIZE (10040). A network sender that can reach the host UDP port can send a payload exceeding 2,048 bytes and make discovery return an error, which terminates the active authenticated TCP/media session. The IP and cipher checks are only in the successful receive arm, so the attacker does not need the client's IP or a PSK. Discovery continues running even after endpoint registration, making an established stream vulnerable too. Repeating the packet denies streaming service. This is Windows-specific; Unix truncation behavior does not establish safety on Winsock.
- **Fix**: Treat Winsock WSAEMSGSIZE as a discarded invalid datagram and continue within the bounded discovery burst, or receive into a buffer large enough for any UDP payload and reject oversized registrations before decryption. Do not propagate malformed-datagram errors as failures of the authenticated session.
- **Confidence**: high

### [P1] The host stop flag cannot cancel an active connection
- **Location**: `clients/rust/maho-host/src/session.rs:2094` (secondary site: `clients/rust/maho-host/src/session.rs:2263`)
- **Evidence**:
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
- **Impact**: Setting the public `serve_with_stop` flag while a viewer is connected does not stop capture, input injection, or networking and does not return from the server. `handle_connection` runs synchronously and never receives or observes the flag. A healthy client answering heartbeats can keep that call alive indefinitely, so an owner waiting for shutdown cannot complete until the client disconnects. The existing stop test sets the flag without first establishing an active connection and does not cover this case.
- **Fix**: Pass the cancellation signal into the connection owner and check it in its loop, routing cancellation through the existing common teardown. Make admission/consent waits cancellation-aware as well so shutdown does not have to consume their full deadline. Keep the existing consumer-before-producer join order.
- **Confidence**: high

### [P1] Media failures terminate only the sender thread, leaving a live but unusable session
- **Location**: `clients/rust/maho-host/src/session.rs:2345` (secondary sites: `clients/rust/maho-host/src/session.rs:2276`, `clients/rust/maho-host/src/session.rs:2360`)
- **Evidence**:
```rust
                                        Ok(MediaEvent::Error(err)) => {
                                            warn!(%err, "Media event error in sender thread");
                                            break;
                                        }
                                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                                            warn!("Media receiver disconnected, exiting sender thread");
                                            break;
                                        }
                                    }
                                }
                                drop(receiver);
                                #[cfg(test)]
                                media_source.sender_exited();
                            });
                            sender_thread = Some(handle);
```
- **Impact**: A capture/encoder initialization failure or later media failure exits this worker but is never reported to the owning connection loop. `sender_thread` remains `Some`, its completion is not checked, and TCP Ping/Pong and input handling continue. A client that continues answering heartbeats can remain on a permanently black/frozen stream indefinitely; the serial host also cannot accept a replacement session. Before UDP registration, media errors are not consumed at all because the only consumer starts after discovery. A log message is not a protocol failure notification or lifecycle transition.
- **Fix**: Add a bounded terminal-status channel from the media/sender worker to the connection owner and inspect it independently of UDP registration. On critical media failure or unexpected sender exit, report the failure over TCP where possible and exit through common cleanup. Do not rely on heartbeat expiry to detect an otherwise responsive client with a failed media pipeline.
- **Confidence**: high

### [P1] Configuration responses claim dimensions and frame rates that were never applied
- **Location**: `clients/rust/maho-host/src/session.rs:2683` (secondary sites: `clients/rust/maho-host/src/session.rs:572`, `clients/rust/maho-host/src/session.rs:2281`)
- **Evidence**:
```rust
                                    None => {
                                        if let Some(media) = &media_handle {
                                            let _ = media.update_bitrate(req.desired.bitrate);
                                        }
                                        send_tcp_control(
                                            &mut stream,
                                            ControlMessage::StreamConfigResponse(
                                                StreamConfigurationResponse {
                                                    request_id: req.request_id,
                                                    active: req.desired,
                                                },
                                            ),
                                        )?;
                                    }
```
- **Impact**: Every in-range request is acknowledged as fully active even though only a bitrate command is submitted. Capture size, encoder size, sender frame dimensions, and capture frame rate remain the original host configuration. For example, requesting 2560x1440 at 30 fps from a 1920x1080/60 host returns a successful 2560x1440/30 response while the host keeps sending 1920x1080/60. Even a failed bitrate submission is discarded and acknowledged as active. Clients receive false negotiation state and cannot determine the actual applied stream configuration from this response.
- **Fix**: Until resizing/frame-rate reconfiguration is implemented, reject requests that differ from the supported active dimensions or frame rate. Track actual applied configuration and populate `active` from it, not from the request. Propagate bitrate submission failures as a configuration rejection and use encoder confirmation if the response promises that an asynchronous update has already become active.
- **Confidence**: high

### [P1] Disconnect cleanup does not release held injected keys and mouse buttons
- **Location**: `clients/rust/maho-host/src/session.rs:2753` (secondary sites: `clients/rust/maho-host/src/inject_macos.rs:138`, `clients/rust/maho-host/src/inject_windows.rs:175`)
- **Evidence**:
```rust
        if let Some(mut media) = media_handle {
            media.stop();
        }
        state = SessionState::Closed;
        if let Some(trace) = host_trace::enabled() {
            if let Err(error) = trace.dump() {
                warn!(%error, "Host diagnostic dump failed");
            }
        }
        debug!(?state, "session closed");
        result
```
```rust
        if event.event_type == InputEventType::Reset {
            let buttons = self.active_mouse_buttons.borrow().clone();
            let keys = self.active_keys.borrow().clone();
```
```rust
impl Drop for WindowsInputInjector {
    fn drop(&mut self) {
        let inputs = modifier_inputs(self.modifiers, Modifiers::empty());
        let _ = send_inputs(&inputs);
    }
}
```
- **Impact**: If a remote user is holding a key or dragging when the connection drops, teardown stops media but never resets injected input. The macOS injector tracks held keys/buttons but releases them only on an explicit peer-sent Reset and has no Drop cleanup. Windows Drop releases only tracked modifiers, not ordinary keys or mouse buttons. Consequently, disconnecting after KeyDown or LeftMouseDown can leave the host repeating a key or dragging after the session has ended; a network failure cannot be expected to deliver the matching release/Reset. This finding applies to macOS and Windows; Linux virtual-device removal has different OS cleanup semantics and is not assumed to have the same defect.
- **Fix**: Give each injector an explicit release-all operation that tracks successfully injected key/button downs and emits matching ups without normal input throttling. Invoke it on every connection exit before discarding injector state, with Drop as a fallback and cleanup errors logged. Windows must track ordinary keys/buttons too; merely invoking its current modifier-only Drop is insufficient.
- **Confidence**: high

## Non-findings checked

- No `async`, `.await`, Tokio task spawn, or `force_keyflag` exists in the four scoped files. `main` calls a synchronous server; its blocking calls are not evidence of blocking a Tokio worker. The imported TLS layer offers synchronous deadline adapters.
- macOS keyframe intent and latest bitrate are protected by the same mutex. Full encoder command queues leave pending controls intact; successful enqueue clears the pending value while still holding that mutex.
- Native Windows/Linux controls merge keyframe intent under the handoff mutex and reselect pending work after bitrate changes; no load-then-store AtomicBool keyframe-loss race exists in this current implementation.
- The Windows `first_keyframe_emitted` atomic is a progress hint for repeat/jiggle cadence, not the force-keyframe request mechanism and not a publication fence for frame data.
- The common connection epilogue drops an unclaimed media receiver, or signals and joins its sender owner, before stopping/joining media producers. Ordinary disconnect and protocol-error paths therefore release full output queues before producer joins.
- Native worker activation is gated until all spawns succeed; partial startup drops the worker owner, signals cancellation, and joins workers before they can block publishing media.
- macOS media publication and encoder output queues are bounded; cancellation wakes the owning worker, and encoded output is released before encoder shutdown.
- The session media channel is bounded to 16 events; native raw video has a single replaceable slot and Linux audio has a single latest-block slot. The unbounded sender-stop channel receives at most one shutdown message per connection.
- Trace records are capped at 65,536 entries with a saturating overflow counter, rather than growing for the lifetime of the daemon.
- UDP discovery processes at most 32 packets per iteration, authenticates the registration Ping, checks the TCP peer IP on successful receives, and does not replace a registered endpoint. The Windows oversized-receive error path above bypasses those successful-receive protections.
- Bootstrap pairing requires approval and a matching newly granted pairing ID before media starts; paired TLS identities must match the application-handshake pairing ID. Duplicate authenticated handshakes are rejected.
- One absolute admission deadline bounds TLS acceptance, preauth reads, and consent waiting. Authenticated sessions use short read timeouts and a finite write timeout; these do not, by themselves, implement host cancellation or detect media-worker completion.
- Sender packetization bounds video/audio fragment sizes to the 1,200-byte encrypted datagram budget and checks narrowing conversions for frame/chunk counts.
