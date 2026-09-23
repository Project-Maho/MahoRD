// allow: SIZE_OK — comprehensive native lifecycle regression suite
use std::{
    net::UdpSocket,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use maho_app::{PairingRecord, PairingStore};
use maho_ios_lib::{AppState, AudioOutputEvent, ConnectionState, IpcErrorCode, WorkerKind};
use maho_net::{PskIdentity, TlsPskServer};
use maho_proto::{PacketHeader, PacketType, WireCodec};

fn make_packet(packet_type: PacketType, payload: &[u8]) -> Vec<u8> {
    let mut header = PacketHeader::new(packet_type, 0, 0, 0).to_bytes().to_vec();
    header.extend_from_slice(payload);
    header
}

fn split_packet(bytes: &[u8]) -> (PacketHeader, &[u8]) {
    let header = PacketHeader::decode(&bytes[..PacketHeader::SIZE]).unwrap();
    (header, &bytes[PacketHeader::SIZE..])
}

struct IsolatedStore {
    _temp_dir: tempfile::TempDir,
    store_file: std::path::PathBuf,
}

impl IsolatedStore {
    fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let store_file = temp_dir.path().join("client-pairings.json");
        Self {
            _temp_dir: temp_dir,
            store_file,
        }
    }

    fn write_record(&self, id: &str, name: &str, key: &[u8]) {
        let store = PairingStore::new(&self.store_file);
        let record = PairingRecord {
            id: id.to_string(),
            name: name.to_string(),
            key: key.to_vec(),
            added_at_unix_ms: 1725900000000,
            last_endpoint: None,
            endpoint_aliases: Vec::new(),
            relay_url: None,
            relay_host_id: None,
        };
        store.save(record).expect("save pairing record");
    }

    fn open_store(&self) -> PairingStore {
        PairingStore::new(&self.store_file)
    }
}

struct LoopbackServer {
    tcp_port: u16,
    udp_port: u16,
    _udp_socket: UdpSocket,
    close_tx: Option<mpsc::Sender<()>>,
    server_handle: Option<thread::JoinHandle<()>>,
}

impl LoopbackServer {
    fn spawn_authenticated(pairing_id: &str, key: &[u8]) -> Self {
        let key_vec = key.to_vec();
        let psk = PskIdentity::pairing(pairing_id, &key_vec).expect("valid psk identity");
        let tls_server = TlsPskServer::new([psk]).expect("tls server create");
        let listener = tls_server.bind("127.0.0.1:0").expect("bind listener");
        let tcp_port = listener.local_addr().expect("tcp port").port();

        let udp_socket = UdpSocket::bind("127.0.0.1:0").expect("bind udp socket");
        let udp_port = udp_socket.local_addr().expect("udp port").port();

        let (close_tx, close_rx) = mpsc::channel::<()>();

        let server_handle = thread::spawn(move || {
            let mut stream = match listener.accept() {
                Ok(s) => s,
                Err(_) => return,
            };
            if close_rx.try_recv().is_ok() {
                return;
            }
            let _ = stream
                .ssl_stream()
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(5)));

            let frame = match stream.read_frame() {
                Ok(f) => f,
                Err(_) => return,
            };
            let (_, payload) = split_packet(&frame);
            if stream
                .write_frame(&make_packet(PacketType::HandshakeAck, payload))
                .is_err()
            {
                return;
            }

            // Teardown is completely event-driven: awaits remote close signal or channel drop
            let _ = close_rx.recv();

            // Remote clean closure of TCP stream:
            let _ = stream
                .ssl_stream()
                .get_ref()
                .shutdown(std::net::Shutdown::Both);
            drop(stream);
            drop(listener);
        });

        Self {
            tcp_port,
            udp_port,
            _udp_socket: udp_socket,
            close_tx: Some(close_tx),
            server_handle: Some(server_handle),
        }
    }

    fn trigger_remote_close(&mut self) {
        if let Some(tx) = self.close_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        if let Some(tx) = self.close_tx.take() {
            let _ = tx.send(());
        }
        let _ = std::net::TcpStream::connect(format!("127.0.0.1:{}", self.tcp_port));
        if let Some(handle) = self.server_handle.take() {
            let _ = handle.join();
        }
    }
}

#[tokio::test]
async fn test_ios_tcp_disconnect_updates_session_state() {
    let store = IsolatedStore::new();
    let pairing_id = "IOS-LIFECYCLE-TCP-CLOSE";
    let key = [99u8; 32];
    store.write_record(pairing_id, "loopback-host", &key);

    let mut server = LoopbackServer::spawn_authenticated(pairing_id, &key);
    let tcp_port = server.tcp_port;
    let udp_port = server.udp_port;

    let app_state = AppState::with_pairing_store(store.open_store());
    app_state.set_headless_audio(true);

    let stats = app_state
        .connect_async(
            "127.0.0.1".to_string(),
            Some(tcp_port),
            Some(udp_port),
            None,
            Some(pairing_id.to_string()),
        )
        .await
        .expect("connect must succeed with valid stored pairing");

    assert_eq!(stats.state, "ready");

    let rx = app_state.subscribe_worker_completions();

    // Trigger remote TCP close on server side with UDP remaining completely silent:
    server.trigger_remote_close();

    // Bounded wait for exact worker completions (event-driven, no polling sleeps):
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut got_supervisor = false;
    let mut got_media = false;
    let mut got_audio = false;

    while Instant::now() < deadline && (!got_supervisor || !got_media || !got_audio) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Ok(completion) = rx.recv_timeout(remaining) {
            match completion.kind {
                WorkerKind::Supervisor => got_supervisor = true,
                WorkerKind::Media => got_media = true,
                WorkerKind::Audio => got_audio = true,
            }
        }
    }

    assert!(
        got_supervisor,
        "Supervisor worker must emit completion signal upon TCP remote close"
    );
    assert!(
        got_media,
        "Media worker must exit and emit completion signal when TCP closes, even with UDP silent"
    );
    assert!(
        got_audio,
        "Audio worker must exit and emit completion signal when TCP closes"
    );

    let post_stats = app_state.stats().expect("stats must succeed");
    assert_eq!(
        post_stats.state, "disconnected",
        "State must transition to disconnected; last_error={:?}",
        post_stats.last_error
    );
    assert_eq!(
        post_stats.last_error.as_deref(),
        Some("remote-closed"),
        "Terminal reason must be preserved as remote-closed"
    );

    app_state
        .disconnect_async()
        .await
        .expect("clean disconnect");
}

#[tokio::test]
async fn test_stale_terminal_event_ignored_when_generation_advances() {
    let app_state = AppState::new();

    // Advance generation to 2:
    {
        let mut inner = app_state.inner.lock().unwrap();
        inner.generation = 2;
        inner.state = ConnectionState::Ready;
    }

    // A stale terminal shutdown from generation 1 arrives:
    let res = app_state.handle_terminal_shutdown(
        1,
        ConnectionState::Disconnected,
        Some("stale-terminal-error".to_string()),
    );
    assert!(res.is_ok());

    // Generation 2 must be unaffected:
    let stats = app_state.stats().expect("stats");
    assert_eq!(
        stats.state, "ready",
        "Stale terminal event must not modify newer generation state"
    );
    assert!(
        stats.last_error.is_none(),
        "Stale terminal event must not overwrite newer generation error"
    );
}

struct PendingConnectServer {
    tcp_port: u16,
    close_tx: Option<mpsc::Sender<()>>,
    server_handle: Option<thread::JoinHandle<()>>,
}

impl Drop for PendingConnectServer {
    fn drop(&mut self) {
        if let Some(tx) = self.close_tx.take() {
            let _ = tx.send(());
        }
        let _ = std::net::TcpStream::connect(format!("127.0.0.1:{}", self.tcp_port));
        if let Some(handle) = self.server_handle.take() {
            let _ = handle.join();
        }
    }
}

impl PendingConnectServer {
    fn spawn_holding_handshake(
        pairing_id: &str,
        key: &[u8],
        pending_tx: tokio::sync::oneshot::Sender<()>,
    ) -> Self {
        let key_vec = key.to_vec();
        let psk = PskIdentity::pairing(pairing_id, &key_vec).expect("valid psk identity");
        let tls_server = TlsPskServer::new([psk]).expect("tls server create");
        let listener = tls_server.bind("127.0.0.1:0").expect("bind listener");
        let tcp_port = listener.local_addr().expect("tcp port").port();
        let (close_tx, close_rx) = mpsc::channel::<()>();

        let server_handle = thread::spawn(move || {
            let mut stream = match listener.accept() {
                Ok(s) => s,
                Err(_) => return,
            };
            if close_rx.try_recv().is_ok() {
                return;
            }
            // Read client's Handshake frame:
            let _ = match stream.read_frame() {
                Ok(f) => f,
                Err(_) => return,
            };
            // Handshake frame received! Signal that connect is pending at HandshakeAck:
            let _ = pending_tx.send(());

            // Hold connection and wait for cancellation or server drop (event-driven, no sleep):
            let _ = close_rx.recv();
            let _ = stream
                .ssl_stream()
                .get_ref()
                .shutdown(std::net::Shutdown::Both);
        });

        Self {
            tcp_port,
            close_tx: Some(close_tx),
            server_handle: Some(server_handle),
        }
    }
}

#[tokio::test]
async fn test_canceled_connect_cannot_install_resources() {
    let store = IsolatedStore::new();
    let pairing_id = "IOS-CANCEL-PENDING-ACK";
    let key = [33u8; 32];
    store.write_record(pairing_id, "loopback-host", &key);

    let (pending_tx, pending_rx) = tokio::sync::oneshot::channel::<()>();
    let server = PendingConnectServer::spawn_holding_handshake(pairing_id, &key, pending_tx);
    let tcp_port = server.tcp_port;

    let app_state = AppState::with_pairing_store(store.open_store());
    app_state.set_headless_audio(true);

    let state_clone = app_state.clone();
    let connect_handle = tokio::spawn(async move {
        state_clone
            .connect_async(
                "127.0.0.1".to_string(),
                Some(tcp_port),
                Some(12345),
                None,
                Some(pairing_id.to_string()),
            )
            .await
    });

    // Event-driven: wait for client connect to be pending at HandshakeAck BEFORE triggering cancel:
    pending_rx
        .await
        .expect("Client connect must be pending at HandshakeAck before cancellation trigger");

    // Promptly cancel in-flight connect WITHOUT waiting behind lifecycle lock:
    let start_cancel = Instant::now();
    app_state.cancel_active_connect();

    // Connect task must settle promptly with IpcErrorCode::Cancelled:
    let connect_res = connect_handle.await.expect("connect task join");
    let elapsed = start_cancel.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "Cancellation must settle promptly without lock starvation, took {elapsed:?}"
    );

    assert!(
        connect_res.is_err(),
        "Cancelled connect must return error, got: {:?}",
        connect_res
    );
    let err = connect_res.unwrap_err();
    assert_eq!(
        err.code,
        IpcErrorCode::Cancelled,
        "Error code must be Cancelled, got {err:?}"
    );

    // Disconnect must settle cleanly and state must remain idle:
    let disconnect_res = app_state.disconnect_async().await;
    assert!(disconnect_res.is_ok(), "Disconnect must succeed");

    let stats = app_state.stats().expect("stats");
    assert_eq!(
        stats.state, "idle",
        "State must remain idle after cancelled connect"
    );
}

#[tokio::test]
async fn test_cancel_before_native_registration_race() {
    let store = IsolatedStore::new();
    let pairing_id = "IOS-CANCEL-RACE";
    let key = [44u8; 32];
    store.write_record(pairing_id, "loopback-host", &key);

    let server = LoopbackServer::spawn_authenticated(pairing_id, &key);
    let tcp_port = server.tcp_port;
    let udp_port = server.udp_port;

    let app_state = AppState::with_pairing_store(store.open_store());
    app_state.set_headless_audio(true);

    // Trigger cancellation BEFORE initiating the connect request:
    app_state.cancel_active_connect();

    let res = app_state
        .connect_async(
            "127.0.0.1".to_string(),
            Some(tcp_port),
            Some(udp_port),
            None,
            Some(pairing_id.to_string()),
        )
        .await;

    assert!(
        res.is_err(),
        "Connect must fail when cancelled before registration"
    );
    let err = res.unwrap_err();
    assert_eq!(
        err.code,
        IpcErrorCode::Cancelled,
        "Error code must be Cancelled, got {err:?}"
    );

    let stats = app_state.stats().expect("stats");
    assert_eq!(stats.state, "idle");
}

#[tokio::test]
async fn test_worker_completion_signals_emitted_on_disconnect() {
    let store = IsolatedStore::new();
    let pairing_id = "IOS-WORKER-SIGNALS";
    let key = [77u8; 32];
    store.write_record(pairing_id, "loopback-host", &key);

    let server = LoopbackServer::spawn_authenticated(pairing_id, &key);
    let tcp_port = server.tcp_port;
    let udp_port = server.udp_port;

    let app_state = AppState::with_pairing_store(store.open_store());
    app_state.set_headless_audio(true);

    app_state
        .connect_async(
            "127.0.0.1".to_string(),
            Some(tcp_port),
            Some(udp_port),
            None,
            Some(pairing_id.to_string()),
        )
        .await
        .expect("connect must succeed");

    let rx = app_state.subscribe_worker_completions();

    app_state.disconnect_async().await.expect("disconnect");

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut got_supervisor = false;
    let mut got_media = false;
    let mut got_audio = false;

    while Instant::now() < deadline && (!got_supervisor || !got_media || !got_audio) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Ok(completion) = rx.recv_timeout(remaining) {
            match completion.kind {
                WorkerKind::Supervisor => got_supervisor = true,
                WorkerKind::Media => got_media = true,
                WorkerKind::Audio => got_audio = true,
            }
        }
    }

    assert!(
        got_supervisor,
        "Supervisor must emit completion on disconnect"
    );
    assert!(got_media, "Media worker must emit completion on disconnect");
    assert!(got_audio, "Audio worker must emit completion on disconnect");

    let stats = app_state.stats().expect("stats");
    assert_eq!(stats.state, "idle");
}

#[tokio::test]
async fn test_audio_error_triggers_generation_aware_session_shutdown() {
    let store = IsolatedStore::new();
    let pairing_id = "IOS-AUDIO-ERROR-SHUTDOWN";
    let key = [88u8; 32];
    store.write_record(pairing_id, "loopback-host", &key);

    let server = LoopbackServer::spawn_authenticated(pairing_id, &key);
    let tcp_port = server.tcp_port;
    let udp_port = server.udp_port;

    let app_state = AppState::with_pairing_store(store.open_store());
    app_state.set_headless_audio(true);

    app_state
        .connect_async(
            "127.0.0.1".to_string(),
            Some(tcp_port),
            Some(udp_port),
            None,
            Some(pairing_id.to_string()),
        )
        .await
        .expect("connect");

    let rx = app_state.subscribe_worker_completions();

    // Inject genuine AudioOutputEvent::Error via the production event seam:
    app_state
        .inject_audio_event_for_test(AudioOutputEvent::Error(
            "Simulated CPAL device error".to_string(),
        ))
        .expect("Audio event injection must succeed");

    // Supervisor, media, and audio workers must stop:
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut got_supervisor = false;
    let mut got_media = false;
    let mut got_audio = false;

    while Instant::now() < deadline && (!got_supervisor || !got_media || !got_audio) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Ok(completion) = rx.recv_timeout(remaining) {
            match completion.kind {
                WorkerKind::Supervisor => got_supervisor = true,
                WorkerKind::Media => got_media = true,
                WorkerKind::Audio => got_audio = true,
            }
        }
    }

    assert!(got_supervisor, "Supervisor worker must stop on audio error");
    assert!(got_media, "Media worker must stop on audio error");
    assert!(got_audio, "Audio worker must stop on audio error");

    let stats = app_state.stats().expect("stats");
    assert_eq!(stats.state, "error");
    assert!(
        stats
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("Simulated CPAL device error"),
        "Audio error message must be preserved"
    );

    app_state.disconnect_async().await.expect("disconnect");
}

#[tokio::test]
async fn test_early_tcp_close_aborts_session_and_cleans_resources() {
    let store = IsolatedStore::new();
    let pairing_id = "IOS-EARLY-TCP-CLOSE";
    let key = [66u8; 32];
    store.write_record(pairing_id, "loopback-host", &key);

    let key_vec = key.to_vec();
    let psk = PskIdentity::pairing(pairing_id, &key_vec).expect("psk");
    let tls_server = TlsPskServer::new([psk]).expect("tls server");
    let listener = tls_server.bind("127.0.0.1:0").expect("bind listener");
    let tcp_port = listener.local_addr().unwrap().port();

    let udp_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_port = udp_socket.local_addr().unwrap().port();

    let server_handle = thread::spawn(move || {
        let mut stream = listener.accept().expect("accept");
        let frame = stream.read_frame().expect("read handshake");
        let (_, payload) = split_packet(&frame);
        stream
            .write_frame(&make_packet(PacketType::HandshakeAck, payload))
            .expect("write ack");

        // Immediately close the TCP stream abruptly:
        let _ = stream
            .ssl_stream()
            .get_ref()
            .shutdown(std::net::Shutdown::Both);
        drop(stream);
        drop(listener);
    });

    let app_state = AppState::with_pairing_store(store.open_store());
    app_state.set_headless_audio(true);

    let rx = app_state.subscribe_worker_completions();

    let connect_res = app_state
        .connect_async(
            "127.0.0.1".to_string(),
            Some(tcp_port),
            Some(udp_port),
            None,
            Some(pairing_id.to_string()),
        )
        .await;

    // Either connect fails immediately or connects and immediately moves to disconnected:
    if let Ok(stats) = connect_res {
        assert_eq!(stats.state, "ready");
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got_supervisor = false;
        while Instant::now() < deadline && !got_supervisor {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Ok(completion) = rx.recv_timeout(remaining) {
                if completion.kind == WorkerKind::Supervisor {
                    got_supervisor = true;
                }
            }
        }
        assert!(
            got_supervisor,
            "Supervisor worker must complete upon remote TCP close"
        );
        let s = app_state.stats().unwrap();
        assert_eq!(s.state, "disconnected");
        assert_eq!(s.last_error.as_deref(), Some("remote-closed"));
    }

    app_state.disconnect_async().await.expect("disconnect");
    let _ = server_handle.join();
}

#[tokio::test]
async fn test_audio_init_failure_triggers_clean_rollback() {
    let store = IsolatedStore::new();
    let pairing_id = "IOS-AUDIO-INIT-FAIL";
    let key = [55u8; 32];
    store.write_record(pairing_id, "loopback-host", &key);

    let server = LoopbackServer::spawn_authenticated(pairing_id, &key);
    let tcp_port = server.tcp_port;
    let udp_port = server.udp_port;

    let app_state = AppState::with_pairing_store(store.open_store());
    // Explicitly set headless audio to FALSE: on a headless runner without audio output,
    // CPAL audio output initialization fails:
    app_state.set_headless_audio(false);

    let connect_res = app_state
        .connect_async(
            "127.0.0.1".to_string(),
            Some(tcp_port),
            Some(udp_port),
            None,
            Some(pairing_id.to_string()),
        )
        .await;

    // If runner has no CPAL device (or on headless CI), connect fails and rolls back cleanly:
    if let Err(err) = connect_res {
        assert_eq!(err.stage, maho_app::IpcErrorStage::Runtime);
        let stats = app_state.stats().expect("stats");
        assert!(matches!(stats.state.as_str(), "error" | "idle"));
    }

    app_state.disconnect_async().await.expect("disconnect");
}

#[tokio::test]
async fn test_duplicate_termination_is_idempotent_and_threadsafe() {
    let app_state = AppState::new();
    {
        let mut inner = app_state.inner.lock().unwrap();
        inner.generation = 1;
        inner.state = ConnectionState::Ready;
    }

    let s1 = app_state.clone();
    let s2 = app_state.clone();
    let s3 = app_state.clone();

    let h1 = thread::spawn(move || {
        let _ = s1.handle_terminal_shutdown(1, ConnectionState::Disconnected, Some("term1".into()));
    });
    let h2 = thread::spawn(move || {
        let _ = s2.handle_terminal_shutdown(1, ConnectionState::Disconnected, Some("term2".into()));
    });
    let h3 = thread::spawn(move || {
        let _ = s3.disconnect_blocking();
    });

    h1.join().expect("h1 join");
    h2.join().expect("h2 join");
    h3.join().expect("h3 join");

    let stats = app_state.stats().expect("stats");
    assert!(matches!(stats.state.as_str(), "idle" | "disconnected"));

    // Ensure late errors after Idle do NOT revert back to Error or Disconnected:
    let _ =
        app_state.handle_terminal_shutdown(1, ConnectionState::Error, Some("late-error".into()));
    let final_stats = app_state.stats().expect("stats");
    assert_ne!(
        final_stats.state, "error",
        "Late terminal event must not revert session out of Idle"
    );
}
