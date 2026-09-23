use super::*;
use maho_net::{PskIdentity, TlsPskServer};
use maho_proto::{
    ClipboardSyncDirection, ClipboardSyncOrigin, ClipboardSyncUpdate, ControlMessage, PacketHeader,
    PacketType, WireCodec,
};
use std::{net::UdpSocket, sync::mpsc};

fn packet(kind: PacketType, payload: &[u8]) -> Vec<u8> {
    let mut bytes = PacketHeader::new(kind, 0, 0, 0).encode().unwrap();
    bytes.extend_from_slice(payload);
    bytes
}

// Real TLS/runtime fixture. Pong acknowledges that every preceding clipboard
// update was decoded and published, without consuming the mailbox under test.
struct Connected {
    state: AppState,
    release: mpsc::Sender<()>,
    server: Option<thread::JoinHandle<()>>,
}

impl Connected {
    fn new() -> Self {
        let key = [42; 32];
        let listener = TlsPskServer::new([PskIdentity::pairing("tauri-mailbox", &key).unwrap()])
            .unwrap()
            .bind("127.0.0.1:0")
            .unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut config = SessionConfig::direct("127.0.0.1", "mailbox-test");
        config.tcp_port = listener.local_addr().unwrap().port();
        config.udp_port = udp.local_addr().unwrap().port();
        config.pairing_store_path = Some(std::env::temp_dir().join("unused-tauri-pairing-store"));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            stream
                .ssl_stream()
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let handshake = stream.read_frame().unwrap();
            stream
                .write_frame(&packet(
                    PacketType::HandshakeAck,
                    &handshake[PacketHeader::SIZE..],
                ))
                .unwrap();
            for index in 0..128 {
                let update = ControlMessage::ClipboardSyncUpdate(ClipboardSyncUpdate {
                    request_id: index,
                    direction: ClipboardSyncDirection::HostToClient,
                    origin: ClipboardSyncOrigin::LocalPasteboard,
                    text: format!("clipboard-{index}"),
                });
                stream
                    .write_frame(&packet(PacketType::Control, &update.encode().unwrap()))
                    .unwrap();
            }
            stream.write_frame(&packet(PacketType::Ping, &[])).unwrap();
            let pong = stream.read_frame().unwrap();
            assert_eq!(
                ControlMessage::decode(&pong[PacketHeader::SIZE..]).unwrap(),
                ControlMessage::Pong
            );
            ready_tx.send(()).unwrap();
            // Sender drop also releases the server on an assertion failure.
            match release_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
                Err(error) => panic!("server release: {error}"),
            }
        });
        let session = ClientSession::new(config).unwrap();
        session
            .connect_with_pairing(maho_app::PairingRecord {
                id: "tauri-mailbox".into(),
                name: "host".into(),
                key: key.to_vec(),
                added_at_unix_ms: 0,
                last_endpoint: None,
                endpoint_aliases: Vec::new(),
                relay_url: None,
                relay_host_id: None,
            })
            .unwrap();
        let runtime = session.spawn_tcp_runtime().unwrap();
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let state = AppState::default();
        *state.session.lock().unwrap() = Some(session);
        *state.tcp_runtime.lock().unwrap() = Some(runtime);
        Self {
            state,
            release,
            server: Some(server),
        }
    }
}

impl Drop for Connected {
    fn drop(&mut self) {
        let _ = self.release.send(());
        if let Some(server) = self.server.take() {
            server.join().unwrap();
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn frame_consumer_applies_only_latest_clipboard() {
    // Given a stalled Tauri consumer and 128 real TCP updates.
    let fixture = Connected::new();
    let writes = Arc::new(Mutex::new(Vec::new()));
    let target = writes.clone();
    // When the command's real consumer runs twice.
    commands::poll_frame_with_clipboard(&fixture.state, move |text| {
        target.lock().unwrap().push(text.to_owned());
        Ok(())
    })
    .await
    .unwrap();
    commands::poll_frame_with_clipboard(&fixture.state, |_| panic!("clipboard replay"))
        .await
        .unwrap();
    // Then the latest clipboard is applied exactly once (not discarded).
    assert_eq!(*writes.lock().unwrap(), ["clipboard-127"]);
    disconnect_internal(&fixture.state).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn frame_consumer_rejects_closed_runtime_instead_of_returning_stale_frame() {
    // Given a real runtime whose worker has stopped and closed its mailbox.
    let fixture = Connected::new();
    fixture
        .state
        .tcp_runtime
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .stop()
        .unwrap();
    fixture
        .state
        .latest_raw_frame
        .lock()
        .unwrap()
        .publish(RawNv12Payload {
            buffer: vec![42],
            ..Default::default()
        });
    // When the existing frame command consumes the closed mailbox.
    let result = commands::poll_frame_with_clipboard(&fixture.state, |_| Ok(())).await;
    // Then the UI receives a rejection, not video from a dead TCP session.
    assert!(result.is_err(), "closed TCP mailbox must reject frame IPC");
    disconnect_internal(&fixture.state).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn tcp_stop_error_survives_full_media_and_frame_cleanup() {
    // Given a live real runtime and an owned media worker.
    let fixture = Connected::new();
    let joined = Arc::new(AtomicBool::new(false));
    let worker_joined = joined.clone();
    fixture
        .state
        .latest_raw_frame
        .lock()
        .unwrap()
        .publish(RawNv12Payload {
            buffer: vec![42],
            ..Default::default()
        });
    *fixture.state.media_handle.lock().unwrap() = Some(thread::spawn(move || {
        worker_joined.store(true, Ordering::SeqCst);
    }));
    // When stop supplies its typed failure to the actual teardown consumer.
    // Runtime panic conversion itself is covered by maho-app's private test.
    let result = disconnect_with_stop(&fixture.state, |runtime| {
        runtime.stop()?;
        Err(SessionError::TcpRuntimePanicked(
            "tauri-tcp-regression".into(),
        ))
    })
    .await;
    // Then the error reaches IPC only after every owned resource is cleaned.
    assert!(
        result.is_err(),
        "TCP stop error must reach the command caller"
    );
    assert!(result.unwrap_err().contains("tauri-tcp-regression"));
    assert!(joined.load(Ordering::SeqCst));
    assert!(fixture.state.media_handle.lock().unwrap().is_none());
    assert!(fixture.state.tcp_runtime.lock().unwrap().is_none());
    assert!(fixture.state.session.lock().unwrap().is_none());
    assert!(fixture.state.take_display_frame().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn clipboard_failure_is_reported_instead_of_silently_discarded() {
    // Given a pending remote clipboard and a failing platform adapter.
    let fixture = Connected::new();
    // When the real consumer applies that clipboard.
    let result = commands::poll_frame_with_clipboard(&fixture.state, |_| {
        Err("clipboard-fixture-error".into())
    })
    .await;
    // Then the frame IPC rejects with the original adapter failure.
    assert_eq!(result.unwrap_err(), "clipboard-fixture-error");
    disconnect_internal(&fixture.state).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn clipboard_worker_yields_executor_and_owns_generation_until_completion() {
    // Given a platform write explicitly gated on a channel, not a timer.
    let fixture = Connected::new();
    let executor = thread::current().id();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut entered_tx = Some(entered_tx);
    let poll = commands::poll_frame_with_clipboard(&fixture.state, move |_| {
        assert_ne!(thread::current().id(), executor);
        entered_tx.take().unwrap().send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(())
    });
    tokio::pin!(poll);
    // When clipboard application is running on the blocking worker.
    std::future::poll_fn(|cx| {
        use std::future::Future;
        assert!(poll.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .unwrap()
        .unwrap();
    // Then executor/mailbox remain responsive, but teardown/reconnect cannot
    // acquire this generation until its clipboard write has completed.
    assert!(fixture.state.lifecycle.try_lock().is_err());
    fixture
        .state
        .latest_raw_frame
        .lock()
        .unwrap()
        .publish(RawNv12Payload {
            buffer: vec![42],
            ..Default::default()
        });
    release_tx.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), poll)
            .await
            .unwrap()
            .unwrap(),
        [42]
    );
    assert!(fixture.state.lifecycle.try_lock().is_ok());
    disconnect_internal(&fixture.state).await.unwrap();
}
