use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use maho_app::{
    ClientSession, IpcErrorCode, IpcErrorStage, PairingEndpoint, PairingRecord, PairingStore,
    SessionConfig, SessionState,
};
use maho_net::{PskIdentity, TlsPskClient, TlsPskServer};
use maho_proto::WireCodec;

use super::commands::{self, authenticate_client_session, list_pairings_internal};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn make_packet(kind: maho_proto::PacketType, payload: &[u8]) -> Vec<u8> {
    let mut bytes = maho_proto::PacketHeader::new(kind, 0, 0, 0)
        .encode()
        .unwrap();
    bytes.extend_from_slice(payload);
    bytes
}

struct TempStoreGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    dir: PathBuf,
    store_file: PathBuf,
    prev_xdg: Option<String>,
    prev_home: Option<String>,
}

impl TempStoreGuard {
    fn new() -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("maho-pairing-test-{unique}"));
        let store_dir = dir.join("MahoRD");
        fs::create_dir_all(&store_dir).unwrap();

        let mac_store_dir = dir
            .join("Library")
            .join("Application Support")
            .join("MahoRD");
        fs::create_dir_all(&mac_store_dir).unwrap();

        let prev_xdg = std::env::var("XDG_DATA_HOME").ok();
        let prev_home = std::env::var("HOME").ok();

        std::env::set_var("XDG_DATA_HOME", &dir);
        std::env::set_var("HOME", &dir);

        let store_file = store_dir.join("pairing-keys.json");
        Self {
            _lock: lock,
            dir,
            store_file,
            prev_xdg,
            prev_home,
        }
    }

    fn write_pairing_record(&self, id: &str, name: &str, key_b64: &str, added_at: u64) {
        let json = format!(
            r#"[
  {{
    "id": "{id}",
    "name": "{name}",
    "key": "{key_b64}",
    "addedAt": {added_at}
  }}
]"#
        );
        let linux_client = self.dir.join("MahoRD").join("client-pairings.json");
        let linux_legacy = self.dir.join("MahoRD").join("pairing-keys.json");
        let mac_client = self
            .dir
            .join("Library")
            .join("Application Support")
            .join("MahoRD")
            .join("client-pairings.json");
        let mac_legacy = self
            .dir
            .join("Library")
            .join("Application Support")
            .join("MahoRD")
            .join("pairing-keys.json");

        let _ = fs::write(&linux_client, &json);
        let _ = fs::write(&linux_legacy, &json);
        let _ = fs::write(&mac_client, &json);
        let _ = fs::write(&mac_legacy, &json);
    }

    fn open_store(&self) -> PairingStore {
        PairingStore::new(&self.store_file)
    }
}

impl Drop for TempStoreGuard {
    fn drop(&mut self) {
        match &self.prev_xdg {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        match &self.prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn test_list_pairings_json_excludes_key_field() {
    let guard = TempStoreGuard::new();
    guard.write_pairing_record(
        "TEST-PAIRING-UUID",
        "mock-remote-host",
        "c2VjcmV0LXNoYXJlZC1rZXktYnl0ZXMtMTIzNDU2Nzg=", // 32 bytes base64
        1725900000000,
    );

    let pairings = commands::list_pairings().expect("list_pairings should succeed");
    let json = serde_json::to_string(&pairings).expect("serialization should succeed");

    // The serialized JSON must contain allowed public metadata fields:
    assert!(json.contains("\"id\""), "JSON must contain 'id': {json}");
    assert!(
        json.contains("\"hostName\""),
        "JSON must contain camelCase 'hostName': {json}"
    );
    assert!(
        json.contains("\"addedAtUnixMs\""),
        "JSON must contain camelCase 'addedAtUnixMs': {json}"
    );

    // The serialized JSON must NOT contain 'key' field or secret key material:
    assert!(
        !json.contains("\"key\""),
        "JSON must NOT contain 'key' field: {json}"
    );
    assert!(
        !json.contains("c2VjcmV0"),
        "JSON must NOT contain secret key bytes: {json}"
    );
}

#[test]
fn test_list_pairings_store_seam_isolated_store_serializes_only_allowed_metadata() {
    let guard = TempStoreGuard::new();
    guard.write_pairing_record(
        "SEAM-UUID-999",
        "desktop-host-target",
        "ZmFrZS1zZWNyZXQta2V5LTMyei1ieXRlcy12ZWN0b3I=",
        1725999999000,
    );

    let store = guard.open_store();
    let summaries = list_pairings_internal(&store).expect("store seam should succeed");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, "SEAM-UUID-999");
    assert_eq!(summaries[0].host_name, "desktop-host-target");
    assert_eq!(summaries[0].added_at_unix_ms, 1725999999000);
    assert_eq!(summaries[0].last_endpoint, None);

    let serialized = serde_json::to_string(&summaries).expect("serialize should succeed");
    println!("SEAM_SERIALIZED_JSON: {}", serialized);
    assert!(
        !serialized.contains("\"key\""),
        "Serialized seam JSON must NOT contain 'key': {serialized}"
    );
    assert!(
        !serialized.contains("ZmFrZS1zZWNyZXQ"),
        "Serialized seam JSON must NOT leak base64 key material: {serialized}"
    );

    let parsed: Vec<serde_json::Value> =
        serde_json::from_str(&serialized).expect("deserialization should succeed");
    assert_eq!(parsed.len(), 1);
    let obj = parsed[0].as_object().expect("entry must be a JSON object");

    // Allowed keys only
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["addedAtUnixMs", "hostName", "id"],
        "JSON must contain ONLY allowed metadata fields"
    );
}

#[test]
fn test_missing_pin_and_missing_record_fails_immediately_without_bootstrap() {
    let guard = TempStoreGuard::new();
    let store = guard.open_store();

    let mut config = SessionConfig::direct("127.0.0.1", "test-client");
    config.tcp_port = 19999;
    config.udp_port = 19998;
    let session = ClientSession::new(config).expect("init session");

    let result = authenticate_client_session(&session, None, &store, None);

    assert!(
        result.is_err(),
        "Must fail when no PIN and no record in store"
    );
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        IpcErrorCode::PairingRequired,
        "Must return explicit pairing-required error"
    );
    assert_eq!(
        err.message, "No PIN provided and no previous pairing found in store",
        "Must return explicit no PIN/pairing error"
    );

    // Ensure session remained disconnected and never attempted bootstrap request
    assert_eq!(
        session.state().unwrap_or(SessionState::Disconnected),
        SessionState::Disconnected,
        "Session must not have attempted bootstrap connection"
    );
}

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
                        Ok(mut tls_stream) => {
                            let _ = tls_stream
                                .ssl_stream()
                                .get_ref()
                                .set_read_timeout(Some(Duration::from_millis(500)));
                            if let Ok(frame) = tls_stream.read_frame() {
                                if frame.len() >= maho_proto::PacketHeader::SIZE {
                                    let _ = tls_stream.write_frame(&make_packet(
                                        maho_proto::PacketType::HandshakeAck,
                                        &frame[maho_proto::PacketHeader::SIZE..],
                                    ));
                                }
                            }
                            ServerOutcome::AcceptedBootstrap
                        }
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

    let result = authenticate_client_session(&session, None, &store, Some("RECONNECT-FAIL-UUID"));

    // Synchronize worker completion with bounded deadline
    let outcome = server.join(Duration::from_secs(3));
    assert_eq!(
        outcome,
        ServerOutcome::DroppedStream,
        "failing peer must accept and drop connection cleanly"
    );

    assert!(
        result.is_err(),
        "Must fail when reconnect target is unreachable or fails transport"
    );
    let err = result.unwrap_err();

    // Must preserve original reconnect cause
    assert_eq!(err.stage, IpcErrorStage::TlsPsk);
    assert!(
        err.message.contains("I/O failed")
            || err.message.contains("TLS transport error")
            || err.message.contains("handshake")
            || err.message.contains("Connection reset")
            || err.message.contains("os error")
            || err.message.contains("refused")
            || err.message.contains("UnexpectedEof"),
        "Error must preserve underlying connection failure cause, got: {err}"
    );

    // Prohibit falling back to bootstrap request or erasing the reconnect error
    assert_ne!(
        err.code,
        IpcErrorCode::PairingRequired,
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
    let result = authenticate_client_session(&session, None, &store, Some("STORED-UNKNOWN-ID"));

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

    assert!(
        result.is_err(),
        "Must fail because server rejects unknown paired PSK identity"
    );
    let err = result.unwrap_err();

    // Prohibit falling back to bootstrap request or erasing the reconnect error
    assert_ne!(
        err.code,
        IpcErrorCode::PairingRequired,
        "Must NOT have attempted bootstrap PIN pairing on reconnect failure"
    );
}

#[test]
fn test_stored_id_reconnect_across_store_and_recreated_session_boundary() {
    let key_bytes = [42u8; 32];
    let key_b64 = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=";
    let id = "STORED-RECONNECT-1234";

    let psk = PskIdentity::pairing(id, &key_bytes).unwrap();
    let server = CancellableServer::spawn_psk_server(psk.clone());
    let tcp_port = server.local_addr().port();

    let guard = TempStoreGuard::new();
    guard.write_pairing_record(id, "ServerHost", key_b64, 1725900000000);
    let store = guard.open_store();

    // Session 1: initial reconnect with explicit pairing_id
    let mut config = SessionConfig::direct("127.0.0.1", "test-client");
    config.tcp_port = tcp_port;
    config.udp_port = 19991;
    let session1 = ClientSession::new(config.clone()).expect("init session 1");

    let result1 = authenticate_client_session(&session1, None, &store, Some(id));
    let outcome1 = server.join(Duration::from_secs(3));
    assert_eq!(outcome1, ServerOutcome::AcceptedBootstrap);
    assert!(
        result1.is_ok(),
        "First reconnect must succeed with stored ID"
    );
    let ready1 = result1.unwrap();
    assert_eq!(ready1.pairing.id, id);

    // Persist verified endpoint metadata using shared remember_endpoint API
    let new_endpoint = PairingEndpoint::new("127.0.0.1", tcp_port, 19991);
    let persisted = store
        .remember_endpoint(id, &key_bytes, new_endpoint.clone())
        .expect("remember_endpoint should succeed");
    assert!(persisted, "Endpoint metadata must be persisted");

    // Session 2: recreated session across store and session boundary
    let server2 = CancellableServer::spawn_psk_server(psk);
    let tcp_port2 = server2.local_addr().port();
    config.tcp_port = tcp_port2;
    let session2 = ClientSession::new(config).expect("init session 2");

    let result2 = authenticate_client_session(&session2, None, &store, Some(id));
    let outcome2 = server2.join(Duration::from_secs(3));
    assert_eq!(outcome2, ServerOutcome::AcceptedBootstrap);
    assert!(
        result2.is_ok(),
        "Second reconnect must succeed with recreated session and store"
    );

    // Verify stored record retained metadata
    let reloaded = store
        .load(id)
        .expect("load record")
        .expect("record present");
    assert_eq!(reloaded.last_endpoint, Some(new_endpoint));
}

#[test]
fn test_wrong_stored_id_fails_before_transport_without_bootstrap() {
    let key_bytes = [42u8; 32];
    let key_b64 = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=";
    let valid_id = "STORED-VALID-ID";

    let psk = PskIdentity::pairing(valid_id, &key_bytes).unwrap();
    let mut server = CancellableServer::spawn_psk_server(psk);
    let tcp_port = server.local_addr().port();

    let guard = TempStoreGuard::new();
    guard.write_pairing_record(valid_id, "ServerHost", key_b64, 1725900000000);
    let store = guard.open_store();

    let mut config = SessionConfig::direct("127.0.0.1", "test-client");
    config.tcp_port = tcp_port;
    config.udp_port = 19992;
    let session = ClientSession::new(config).expect("init session");

    // Attempt connection with unknown/wrong pairing ID
    let result = authenticate_client_session(&session, None, &store, Some("UNKNOWN-WRONG-ID"));

    assert!(
        result.is_err(),
        "Must fail when pairing ID is not found in store"
    );
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        IpcErrorCode::PairingRequired,
        "Unknown ID must yield structured pairing-required error"
    );

    // Cancel server and verify ZERO transport connections were made
    server.cancel();
    let outcome = server.join(Duration::from_secs(3));
    assert_eq!(
        outcome,
        ServerOutcome::Cancelled,
        "Server must have observed zero connections (failure occurred before transport)"
    );
}

#[test]
fn test_missing_id_and_missing_pin_fails_before_transport() {
    let key_bytes = [42u8; 32];
    let key_b64 = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=";
    let valid_id = "STORED-VALID-ID";

    let psk = PskIdentity::pairing(valid_id, &key_bytes).unwrap();
    let mut server = CancellableServer::spawn_psk_server(psk);
    let tcp_port = server.local_addr().port();

    let guard = TempStoreGuard::new();
    guard.write_pairing_record(valid_id, "ServerHost", key_b64, 1725900000000);
    let store = guard.open_store();

    let mut config = SessionConfig::direct("127.0.0.1", "test-client");
    config.tcp_port = tcp_port;
    config.udp_port = 19993;
    let session = ClientSession::new(config).expect("init session");

    // Both PIN and pairing ID are None
    let result = authenticate_client_session(&session, None, &store, None);

    assert!(
        result.is_err(),
        "Must fail when neither PIN nor pairing ID is provided"
    );
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        IpcErrorCode::PairingRequired,
        "Missing PIN and missing ID must yield structured pairing-required error"
    );

    // Cancel server and verify ZERO transport connections were made
    server.cancel();
    let outcome = server.join(Duration::from_secs(3));
    assert_eq!(
        outcome,
        ServerOutcome::Cancelled,
        "Server must have observed zero connections (failure occurred before transport)"
    );
}

#[test]
fn test_same_name_records_select_exact_stored_id_and_key() {
    let _key_a = [1u8; 32];
    let key_b = [2u8; 32];
    let key_a_b64 = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";

    // Both records have the identical human-readable host name: "DESKTOP-1LAPJMP"
    let id_a = "ID-ALPHA-1111";
    let id_b = "ID-BETA-2222";

    // Server is configured ONLY with identity B (key_b)
    let psk_b = PskIdentity::pairing(id_b, &key_b).unwrap();
    let server = CancellableServer::spawn_psk_server(psk_b);
    let tcp_port = server.local_addr().port();

    let guard = TempStoreGuard::new();
    guard.write_pairing_record(id_a, "DESKTOP-1LAPJMP", key_a_b64, 1725900000000);
    // Add second record with same name
    let store = guard.open_store();
    let record_b = PairingRecord::new(id_b, "DESKTOP-1LAPJMP", key_b.to_vec(), 1725900005000);
    store.save(record_b).expect("save record B");

    let mut config = SessionConfig::direct("127.0.0.1", "test-client");
    config.tcp_port = tcp_port;
    config.udp_port = 19994;
    let session = ClientSession::new(config).expect("init session");

    // Requesting exact ID "ID-BETA-2222" must use key_b and succeed
    let result = authenticate_client_session(&session, None, &store, Some(id_b));
    let outcome = server.join(Duration::from_secs(3));
    assert_eq!(
        outcome,
        ServerOutcome::AcceptedBootstrap,
        "Server must accept client using exact matching ID and key B"
    );
    assert!(result.is_ok(), "Must succeed with exact ID-BETA");
    assert_eq!(result.unwrap().pairing.id, id_b);
}

#[test]
fn test_no_network_stored_id_fails_without_fallback_to_pin() {
    let _key_bytes = [42u8; 32];
    let key_b64 = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=";
    let id = "STORED-ID-NO-NET";

    // Bind and immediately drop listener to find an unused closed port
    let closed_port = {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().unwrap().port()
    };

    let guard = TempStoreGuard::new();
    guard.write_pairing_record(id, "ServerHost", key_b64, 1725900000000);
    let store = guard.open_store();

    let mut config = SessionConfig::direct("127.0.0.1", "test-client");
    config.tcp_port = closed_port;
    config.udp_port = 19995;
    let session = ClientSession::new(config).expect("init session");

    // Stored reconnect to closed port
    let result = authenticate_client_session(&session, None, &store, Some(id));

    assert!(result.is_err(), "Must fail when network endpoint is closed");
    let err = result.unwrap_err();

    // Must be classified as network unreachable or connection failed, NOT pairing-required
    assert_ne!(
        err.code,
        IpcErrorCode::PairingRequired,
        "Must NOT report pairing-required or attempt fallback to bootstrap PIN on network error"
    );
    assert!(
        err.code == IpcErrorCode::NetworkUnreachable || err.code == IpcErrorCode::ConnectionFailed,
        "Error code must be NetworkUnreachable or ConnectionFailed, got: {:?}",
        err.code
    );
}
