# Phase A Task Implementation Report: R4 Public Key-Free Pairing Summary IPC & Client R11 Removal of Automatic Default-PIN Fallback

- Task ID: `st_01a08970`
- Worker: `hephaestus`
- Session: `01a0890a-69f1-7e5e-80cb-959dc3ddb61c` (Depth: 1)
- Date: 2026-09-10
- Base Commit: `71f8b05f5a0e9d53ce0249beb46ca864b7a836f8`
- Target Remote: `indo@100.91.254.71` (`/home/indo/projects/erd-pairing-20260910`)
- Provenance: Originates from planned implementation runs `st_01a0892b`, `st_01a08939`, and `st_01a08965`; fully audited, test-hardened, and proven under generation 5 task `st_01a08970` resolving coordinator acceptance gate findings.
- Deliverable: Tested Code (`clients/rust/tauri-shell/src-tauri/src/pairing_tests.rs`) + Report (`.omo/pairing-20260910/reports/a-ipc.md`)

---

## 1. Executive Summary

Implemented, hardened, and verified findings **R4** (public key-free pairing summary IPC) and **R11** (removal and permanent prohibition of automatic default-PIN fallback) in `clients/rust/tauri-shell/src-tauri/src/lib.rs` and `clients/rust/tauri-shell/src-tauri/src/pairing_tests.rs`:

### 1.1 Lead Gate Resolution (Generation 5)
Under task `st_01a08970`, the acceptance concerns identified in coordinator follow-up and generation 5 gate reviews were resolved:
1. **Elimination of `WorkerJoinGuard` Dummy Connect and Unbounded Joins:**
   - Deleted `WorkerJoinGuard`. It previously attempted a dummy loopback `TcpStream::connect_timeout` to unblock `listener.accept()` and executed an unbounded `handle.join()` in `Drop`.
   - Dummy TCP connect cannot cancel an already accepted in-flight TLS handshake and races with listener closure and port reuse.
   - Replaced with `CancellableServer` featuring an owned cancellable accept via Tokio `select!` with a dedicated `oneshot` cancellation channel.
   - `CancellableServer::drop` signals cancellation via `cancel_tx` only. It detaches the thread without any unbounded join in `Drop`, preventing hangs on timeout or panic unwind.
2. **Explicitly Bounded TLS Handshake:**
   - In `spawn_psk_server`, the accepted stream is handed to `TlsPskServer::accept_stream_until(stream, Instant::now() + Duration::from_secs(3))` with an explicit 3-second deadline.
   - TLS handshakes that fail (e.g. unknown PSK identity) reject immediately in OpenSSL; stalled or trickle handshakes are terminated deterministically by the deadline.
3. **Continuous Listener Ownership:**
   - The TCP listener is bound to `127.0.0.1:0` at server creation and its ownership is retained throughout the entire authentication attempt. The port is never unbound before the client connects.
4. **Zero Polling, Zero Sleeps:**
   - All server synchronization is strictly event-driven (oneshot channels, MPSC completion channels, Tokio select, and deadline timers). No `thread::sleep` or polling loops are used.
5. **Direct Proof of Cancellation and Normal Completion:**
   - Added dedicated unit tests proving both behaviors directly:
     - `test_cancellable_accept_cancels_cleanly_before_client_connection`: Cancelling accept before any client connects returns `ServerOutcome::Cancelled` cleanly and rapidly.
     - `test_cancellable_accept_normal_completion_with_real_client`: A connecting real `TlsPskClient` completes the handshake and yields `ServerOutcome::AcceptedBootstrap`.

### 1.2 Core Capabilities Delivered
1. **R4: Public Key-Free Pairing Summary IPC**:
   - Defined shell-local `PairingSummary` struct in `tauri-shell/src-tauri/src/lib.rs` with camelCase serialization fields (`id`, `hostName`, `addedAtUnixMs`, optional `lastEndpoint`). Secret 32-byte symmetric keys are completely omitted.
   - Implemented `From<erd_app::PairingRecord>` and `From<&erd_app::PairingRecord>` for `PairingSummary`.
   - Preserved flat IPC invocation signature for `commands::list_pairings` returning `Result<Vec<PairingSummary>, String>`.
   - Exposed `commands::list_pairings_internal(store: &PairingStore)` as a clean command/store seam for isolated testing.
   - Proved via serialized JSON inspection that output contains only public metadata (`id`, `hostName`, `addedAtUnixMs`) with zero occurrences of `"key"` or raw key material.

2. **Client R11: Removal and Prevention of Automatic Default-PIN Fallback**:
   - Extracted `commands::authenticate_client_session` and `commands::resolve_stored_pairing` in `lib.rs` to structure authentication cleanly without altering external IPC signatures.
   - Preserved all Phase C R8 credential-selection matching logic (exact/case-insensitive matching, prefix matching, and discovery name association).
   - If no PIN is supplied and no matching record exists in the store, authentication immediately returns `Err("No PIN provided and no previous pairing found in store".to_string())` without attempting bootstrap TLS (`erd-b1`) or defaulting to PIN `"12345678"`.
   - If stored reconnect fails (due to network error, closed port, or host TLS PSK rejection), the original error cause is preserved (`format!("Reconnect error: {e}")`), and under no circumstances does the client fall back to bootstrap pairing with a default PIN.
   - Behavioral loopback proof using a real loopback `TlsPskServer` confirms that reconnect rejection does not trigger any bootstrap connection attempt.
   - Verified via meaningful mutation proofs (injecting fallback to `pair_with_pin("12345678")` and demonstrating deterministic test failure).

---

## 2. File Ownership and Scope Compliance

Strict boundary discipline was maintained: zero modifications were made to shared `erd-app`, `erd-host`, or any UI files. All pre-existing dirty changes in the working tree (including WebGL, discovery, and input adjustments) were preserved intact.

| File Path | Status | Role & Changes |
|---|---|---|
| `clients/rust/tauri-shell/src-tauri/src/lib.rs` | Assigned Production Source | Defined shell-local `PairingSummary`; updated `commands::list_pairings` return type; provided `commands::list_pairings_internal`; extracted `authenticate_client_session` and `resolve_stored_pairing`; registered `#[cfg(test)] mod pairing_tests;`. Preserved flat signatures and pre-existing R8 discovery/hostname matching. Unchanged in this generation. |
| `clients/rust/tauri-shell/src-tauri/src/pairing_tests.rs` | Assigned Test Module | Scoped unit and integration tests covering R4 JSON secret exclusion, store seam serialization, missing PIN/record rejection, reconnect failure cause preservation, and real loopback TLS PSK server verification. Fully rewritten test server harness using `CancellableServer` (owned cancellable accept, bounded TLS handshake, continuous listener ownership, cancellation proof, normal completion proof). |
| `.omo/pairing-20260910/reports/a-ipc.md` | Deliverable Report | This document. |
| `/var/folders/zh/7cc25lt91b1_dj577306nwdh0000gn/T/ulw-20260910-111421.XXXXXX.md.pNqwGP19PH` | Append-only Notepad | Recorded task provenance, test hardening details, and RED mutation plan prior to execution. |

---

## 3. Regression Failures (RED) via Meaningful Mutation Proofs

### 3.1 RED 1: Cancellation Failure on `test_cancellable_accept_cancels_cleanly_before_client_connection`
To prove that cancellation before client connection is actively asserted and sensitive to failure, an intentional mismatch assertion (`assert_eq!(outcome, ServerOutcome::AcceptedBootstrap)`) was tested against the cancelled accept outcome.

**Remote Command**:
```bash
ssh indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   source \$HOME/.cargo/env; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib test_cancellable_accept"
```

**Exit Code**: `101`

**Failure Output**:
```text
failures:

---- pairing_tests::test_cancellable_accept_cancels_cleanly_before_client_connection stdout ----

thread 'pairing_tests::test_cancellable_accept_cancels_cleanly_before_client_connection' (4023385) panicked at tauri-shell/src-tauri/src/pairing_tests.rs:394:5:
assertion `left == right` failed: INTENTIONAL RED: expected AcceptedBootstrap on cancelled accept
  left: Cancelled
 right: AcceptedBootstrap
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    pairing_tests::test_cancellable_accept_cancels_cleanly_before_client_connection

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 57 filtered out; finished in 0.08s
```
**Diagnostic Proof**: When cancelled before any client connected, the worker cleanly returned `ServerOutcome::Cancelled`. The assertion caught the regression deterministically.

### 3.2 RED 2: Fallback to `"12345678"` on Reconnect Failure
Adversarial mutation injected into `authenticate_client_session`:
```rust
if let Some(record) = matched {
    match session.reconnect(&record.id) {
        Ok(ready) => Ok(ready),
        Err(_) => session
            .pair_with_pin("12345678")
            .map_err(|e| format!("Pairing error: {e}")),
    }
} else { ... }
```

**Remote Command**:
```bash
ssh indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   source \$HOME/.cargo/env; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib pairing"
```

**Exit Code**: `101`

**Failure Output**:
```text
failures:

---- pairing_tests::test_failed_reconnect_preserves_original_cause_and_never_falls_back_to_bootstrap_pin stdout ----

thread 'pairing_tests::test_failed_reconnect_preserves_original_cause_and_never_falls_back_to_bootstrap_pin' (4024389) panicked at tauri-shell/src-tauri/src/pairing_tests.rs:454:5:
Error must start with 'Reconnect error:', got: Pairing error: I/O failed: Connection refused (os error 111)
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- pairing_tests::test_loopback_reconnect_failure_does_not_trigger_bootstrap_request stdout ----

thread 'pairing_tests::test_loopback_reconnect_failure_does_not_trigger_bootstrap_request' (4024392) panicked at tauri-shell/src-tauri/src/pairing_tests.rs:534:5:
Error must start with 'Reconnect error:', got: Pairing error: I/O failed: Connection refused (os error 111)

failures:
    pairing_tests::test_failed_reconnect_preserves_original_cause_and_never_falls_back_to_bootstrap_pin
    pairing_tests::test_loopback_reconnect_failure_does_not_trigger_bootstrap_request

test result: FAILED. 9 passed; 2 failed; 0 ignored; 0 measured; 48 filtered out; finished in 0.15s
```
**Diagnostic Proof**: Both the owned failing peer test and the loopback TLS PSK test immediately caught the error rewrite and unauthorized bootstrap attempt.

### 3.3 RED 3: Fallback to `"12345678"` on Missing PIN and Missing Record
Adversarial mutation injected into `authenticate_client_session`:
```rust
} else {
    session
        .pair_with_pin("12345678")
        .map_err(|e| format!("Pairing error: {e}"))
}
```

**Remote Command**:
```bash
ssh indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   source \$HOME/.cargo/env; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib pairing"
```

**Exit Code**: `101`

**Failure Output**:
```text
failures:

---- pairing_tests::test_missing_pin_and_missing_record_fails_immediately_without_bootstrap stdout ----

thread 'pairing_tests::test_missing_pin_and_missing_record_fails_immediately_without_bootstrap' (4024754) panicked at tauri-shell/src-tauri/src/pairing_tests.rs:200:5:
assertion `left == right` failed: Must return explicit no PIN/pairing error
  left: "Pairing error: I/O failed: Connection refused (os error 111)"
 right: "No PIN provided and no previous pairing found in store"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    pairing_tests::test_missing_pin_and_missing_record_fails_immediately_without_bootstrap

test result: FAILED. 10 passed; 1 failed; 0 ignored; 0 measured; 48 filtered out; finished in 0.08s
```
**Diagnostic Proof**: Missing PIN and record test failed immediately when bootstrap fallback was attempted instead of returning the required error.

---

## 4. Hardened Test Implementation Details

### 4.1 `CancellableServer` Implementation (`pairing_tests.rs`)
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerOutcome {
    Cancelled,
    DroppedStream,
    AcceptedBootstrap,
    RejectedTls(String),
}

struct CancellableServer {
    local_addr: std::net::SocketAddr,
    cancel_tx: Option<tokio::sync::oneshot::Sender<()>>,
    outcome_rx: std::sync::mpsc::Receiver<ServerOutcome>,
    handle: Option<thread::JoinHandle<()>>,
}

impl CancellableServer {
    fn local_addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }

    fn cancel(&mut self) {
        if let Some(tx) = self.cancel_tx.take() {
            let _ = tx.send(());
        }
    }

    fn join(mut self, deadline: Duration) -> ServerOutcome {
        let outcome = self
            .outcome_rx
            .recv_timeout(deadline)
            .expect("server worker must complete within deadline");
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .expect("server worker thread must join cleanly without panicking");
        }
        outcome
    }

    fn spawn_failing_peer() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        listener.set_nonblocking(true).expect("set nonblocking");
        let local_addr = listener.local_addr().expect("local addr");

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let (outcome_tx, outcome_rx) = std::sync::mpsc::channel();

        let handle = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            let _guard = rt.enter();
            let tokio_listener =
                tokio::net::TcpListener::from_std(listener).expect("tokio listener");

            let accepted = rt.block_on(async {
                tokio::select! {
                    res = tokio_listener.accept() => {
                        match res {
                            Ok((stream, _peer_addr)) => {
                                let std_stream = stream.into_std().ok()?;
                                let _ = std_stream.set_nonblocking(false);
                                Some(std_stream)
                            }
                            Err(_) => None,
                        }
                    }
                    _ = cancel_rx => {
                        None
                    }
                }
            });
            drop(_guard);
            drop(rt);

            let outcome = match accepted {
                Some(stream) => {
                    // Drop stream immediately to deterministically fail transport
                    drop(stream);
                    ServerOutcome::DroppedStream
                }
                None => ServerOutcome::Cancelled,
            };
            let _ = outcome_tx.send(outcome);
        });

        Self {
            local_addr,
            cancel_tx: Some(cancel_tx),
            outcome_rx,
            handle: Some(handle),
        }
    }

    fn spawn_psk_server(psk: PskIdentity) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        listener.set_nonblocking(true).expect("set nonblocking");
        let local_addr = listener.local_addr().expect("local addr");

        let tls_server = TlsPskServer::new([psk]).expect("create tls psk server");
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let (outcome_tx, outcome_rx) = std::sync::mpsc::channel();

        let handle = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            let _guard = rt.enter();
            let tokio_listener =
                tokio::net::TcpListener::from_std(listener).expect("tokio listener");

            let accepted = rt.block_on(async {
                tokio::select! {
                    res = tokio_listener.accept() => {
                        match res {
                            Ok((stream, _peer_addr)) => {
                                let std_stream = stream.into_std().ok()?;
                                let _ = std_stream.set_nonblocking(false);
                                Some(std_stream)
                            }
                            Err(_) => None,
                        }
                    }
                    _ = cancel_rx => {
                        None
                    }
                }
            });
            drop(_guard);
            drop(rt);

            let outcome = match accepted {
                Some(stream) => {
                    // Explicitly bounded TLS handshake with strict deadline
                    let deadline = Instant::now() + Duration::from_secs(3);
                    match tls_server.accept_stream_until(stream, deadline) {
                        Ok(_tls_stream) => ServerOutcome::AcceptedBootstrap,
                        Err(e) => ServerOutcome::RejectedTls(e.to_string()),
                    }
                }
                None => ServerOutcome::Cancelled,
            };
            let _ = outcome_tx.send(outcome);
        });

        Self {
            local_addr,
            cancel_tx: Some(cancel_tx),
            outcome_rx,
            handle: Some(handle),
        }
    }
}

impl Drop for CancellableServer {
    fn drop(&mut self) {
        // Deterministically cancel the accept if still in-flight
        if let Some(tx) = self.cancel_tx.take() {
            let _ = tx.send(());
        }
        // Do NOT perform an unbounded join in drop!
        // Dropping `self.handle` detaches the JoinHandle. The thread will terminate
        // promptly because either `cancel_rx` triggers accept exit or `accept_stream_until`
        // expires at its strict deadline.
    }
}
```

### 4.2 Proof of Cancellation and Normal Completion Tests
```rust
#[test]
fn test_cancellable_accept_cancels_cleanly_before_client_connection() {
    let pin = "12345678";
    let psk = PskIdentity::bootstrap(pin).unwrap();
    let mut server = CancellableServer::spawn_psk_server(psk);

    // Cancel BEFORE any client connects:
    server.cancel();

    // The server worker must complete promptly and report Cancelled
    let outcome = server.join(Duration::from_secs(3));
    assert_eq!(
        outcome,
        ServerOutcome::Cancelled,
        "Server must report Cancelled when cancelled before client connection"
    );
}

#[test]
fn test_cancellable_accept_normal_completion_with_real_client() {
    let pin = "12345678";
    let psk = PskIdentity::bootstrap(pin).unwrap();
    let server = CancellableServer::spawn_psk_server(psk.clone());
    let addr = server.local_addr();

    // Connect with a matching real bootstrap client:
    let client = TlsPskClient::new(psk).expect("client");
    let client_stream = client.connect(addr).expect("client connect");
    drop(client_stream);

    let outcome = server.join(Duration::from_secs(3));
    assert_eq!(
        outcome,
        ServerOutcome::AcceptedBootstrap,
        "Worker must complete normal handshake and report AcceptedBootstrap"
    );
}
```

### 4.3 Reconnect Failure Cause Preservation & Loopback Reconnect Tests
```rust
#[test]
fn test_failed_reconnect_preserves_original_cause_and_never_falls_back_to_bootstrap_pin() {
    let guard = TempStoreGuard::new();
    guard.write_pairing_record(
        "RECONNECT-FAIL-UUID",
        "127.0.0.1",
        "c2VjcmV0LXNoYXJlZC1rZXktYnl0ZXMtMTIzNDU2Nzg=",
        1725900000000,
    );
    let store = guard.open_store();

    // Owned deterministic failing peer: keeps port bound and owned throughout the test
    let server = CancellableServer::spawn_failing_peer();
    let failing_port = server.local_addr().port();

    let mut config = SessionConfig::direct("127.0.0.1", "test-client");
    config.tcp_port = failing_port;
    config.udp_port = 19997;
    let session = ClientSession::new(config).expect("init session");

    let result = authenticate_client_session(&session, None, &store, "127.0.0.1", None);

    // Synchronize worker completion with bounded deadline
    let outcome = server.join(Duration::from_secs(3));
    assert_eq!(
        outcome,
        ServerOutcome::DroppedStream,
        "failing peer must accept and drop connection cleanly"
    );

    assert!(result.is_err(), "Must fail when reconnect target is unreachable or fails transport");
    let err = result.unwrap_err();

    // Must preserve original reconnect cause
    assert!(
        err.starts_with("Reconnect error:"),
        "Error must start with 'Reconnect error:', got: {err}"
    );
    assert!(
        err.contains("I/O failed")
            || err.contains("TLS transport error")
            || err.contains("handshake")
            || err.contains("Connection reset")
            || err.contains("os error")
            || err.contains("refused")
            || err.contains("UnexpectedEof"),
        "Error must preserve underlying connection failure cause, got: {err}"
    );

    // Prohibit falling back to bootstrap request or erasing the reconnect error
    assert!(
        !err.contains("Pairing error"),
        "Must NOT have attempted bootstrap PIN pairing on reconnect failure"
    );
}

#[test]
fn test_loopback_reconnect_failure_does_not_trigger_bootstrap_request() {
    // Real loopback peer test!
    // Server ONLY accepts bootstrap PIN "12345678", NOT paired reconnect.
    let pin = "12345678";
    let psk = PskIdentity::bootstrap(pin).unwrap();
    let server = CancellableServer::spawn_psk_server(psk);
    let tcp_port = server.local_addr().port();

    let guard = TempStoreGuard::new();
    // Stored pairing has an unknown pairing ID/key for this server
    guard.write_pairing_record(
        "STORED-UNKNOWN-ID",
        "127.0.0.1",
        "c2VjcmV0LXNoYXJlZC1rZXktYnl0ZXMtMTIzNDU2Nzg=",
        1725900000000,
    );
    let store = guard.open_store();

    let mut config = SessionConfig::direct("127.0.0.1", "test-client");
    config.tcp_port = tcp_port;
    config.udp_port = 19996;
    let session = ClientSession::new(config).expect("init session");

    // Connect with NO pin provided, targeting stored pairing
    let result = authenticate_client_session(&session, None, &store, "127.0.0.1", None);

    // Synchronize worker completion with bounded deadline
    let outcome = server.join(Duration::from_secs(3));

    // CRUCIAL: Server must NOT have seen a bootstrap connection fallback.
    // Checked strictly after synchronized worker completion!
    match &outcome {
        ServerOutcome::AcceptedBootstrap => {
            panic!("Must NOT fallback to bootstrap request with default PIN on reconnect failure!");
        }
        ServerOutcome::RejectedTls(err_str) => {
            assert!(
                err_str.contains("handshake")
                    || err_str.contains("alert")
                    || err_str.contains("unknown")
                    || err_str.contains("SSL")
                    || err_str.contains("error"),
                "Server must record TLS handshake rejection, got: {err_str}"
            );
        }
        ServerOutcome::Cancelled => {
            panic!("Server must not have been cancelled; client should have connected");
        }
        ServerOutcome::DroppedStream => {
            panic!("Server was not configured as a dropping peer");
        }
    }

    assert!(result.is_err(), "Must fail because server rejects unknown paired PSK identity");
    let err = result.unwrap_err();

    // Must preserve original reconnect cause
    assert!(
        err.starts_with("Reconnect error:"),
        "Error must start with 'Reconnect error:', got: {err}"
    );
    assert!(
        err.contains("TLS transport error") || err.contains("handshake"),
        "Must preserve TLS handshake failure cause, got: {err}"
    );

    // Prohibit falling back to bootstrap request or erasing the reconnect error
    assert!(
        !err.contains("Pairing error"),
        "Must NOT have attempted bootstrap PIN pairing on reconnect failure"
    );
}
```

---

## 5. Verification Receipts (GREEN)

Verified on remote host `indo@100.91.254.71`:

### 5.1 Scoped Pairing Tests Run
```bash
ssh indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   source \$HOME/.cargo/env; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib pairing"
```

**Exit Code**: `0`

**Output**:
```text
running 11 tests
test discovery_tests::source_dedup_merges_lan_and_tailscale_by_name_and_inherits_pairing ... ok
test discovery_tests::stored_pairings_and_self_do_not_create_observed_peers ... ok
test discovery_tests::pairing_does_not_infer_identity_from_testbed_ips_case_or_substrings ... ok
test desktop_integration_tests::pairing_store_round_trip_and_deletion ... ok
test pairing_tests::test_list_pairings_store_seam_isolated_store_serializes_only_allowed_metadata ... ok
test pairing_tests::test_missing_pin_and_missing_record_fails_immediately_without_bootstrap ... ok
test pairing_tests::test_failed_reconnect_preserves_original_cause_and_never_falls_back_to_bootstrap_pin ... ok
test pairing_tests::test_list_pairings_json_excludes_key_field ... ok
test pairing_tests::test_cancellable_accept_cancels_cleanly_before_client_connection ... ok
test pairing_tests::test_loopback_reconnect_failure_does_not_trigger_bootstrap_request ... ok
test pairing_tests::test_cancellable_accept_normal_completion_with_real_client ... ok

test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 48 filtered out; finished in 0.08s
```

### 5.2 Seam JSON Output Proof
Running `test_list_pairings_store_seam_isolated_store_serializes_only_allowed_metadata -- --nocapture`:
```bash
ssh indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   source \$HOME/.cargo/env; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib test_list_pairings_store_seam_isolated_store_serializes_only_allowed_metadata -- --nocapture"
```

**Exit Code**: `0`

**Output**:
```text
running 1 test
SEAM_SERIALIZED_JSON: [{"id":"SEAM-UUID-999","hostName":"desktop-host-target","addedAtUnixMs":1725999999000}]
test pairing_tests::test_list_pairings_store_seam_isolated_store_serializes_only_allowed_metadata ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 58 filtered out; finished in 0.00s
```
- Allowed metadata present: `id`, `hostName`, `addedAtUnixMs`.
- Forbidden secret keys: 0 occurrences of `"key"`, 0 occurrences of base64 key material.

### 5.3 Cancellation Before Client Connection Proof
Running `test_cancellable_accept_cancels_cleanly_before_client_connection`:
```bash
ssh indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   source \$HOME/.cargo/env; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib test_cancellable_accept_cancels_cleanly_before_client_connection -- --nocapture"
```

**Exit Code**: `0`

**Output**:
```text
running 1 test
test pairing_tests::test_cancellable_accept_cancels_cleanly_before_client_connection ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 58 filtered out; finished in 0.07s
```

### 5.4 Normal Completion With Real Client Proof
Running `test_cancellable_accept_normal_completion_with_real_client`:
```bash
ssh indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   source \$HOME/.cargo/env; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib test_cancellable_accept_normal_completion_with_real_client -- --nocapture"
```

**Exit Code**: `0`

**Output**:
```text
running 1 test
test pairing_tests::test_cancellable_accept_normal_completion_with_real_client ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 58 filtered out; finished in 0.07s
```

### 5.5 Strict Clippy Check
```bash
ssh indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   source \$HOME/.cargo/env; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo clippy --manifest-path clients/rust/Cargo.toml -p erd-host -p tauri-shell --no-deps -- -D warnings"
```

**Exit Code**: `0`

**Output**:
```text
    Checking tauri-shell v0.1.0 (/home/indo/projects/erd-pairing-20260910/clients/rust/tauri-shell)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.72s
```
0 errors, 0 warnings.

### 5.6 Full Tauri-Shell Test Suite Non-Regression
```bash
ssh indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   source \$HOME/.cargo/env; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib"
```

**Exit Code**: `0`

**Output**:
```text
test result: ok. 58 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.08s
```

### 5.7 Broad Test Suite Across Workspace
```bash
ssh indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   source \$HOME/.cargo/env; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo test --manifest-path clients/rust/Cargo.toml -p erd-host -p erd-app -p tauri-shell"
```

**Exit Code**: `0`

**Result**: 340 passed, 0 failed, 1 ignored (Tailscale observation), 3 doc-tests passed.

---

## 6. Cleanup Receipts

1. **Temporary Stores**: All test storage directories were managed via RAII `TempStoreGuard` and fully removed from disk on test exit.
2. **Network Resources**: Test ports bound to `127.0.0.1:0` were owned by `CancellableServer` and released immediately upon worker termination.
3. **Worker Threads**: All test worker threads were joined cleanly in normal test execution. During any panic or unwind, `Drop` sends cancellation via `cancel_tx` and drops the `JoinHandle` without blocking on an unbounded join.
4. **Synchronization Scope**: Only assigned files (`pairing_tests.rs`) were synced to the remote testbed `/home/indo/projects/erd-pairing-20260910`. Zero production source files or shared crates were altered.
5. **Git Workspace**: Zero git commits were generated; working tree dirty state preserved intact.
