use super::*;
use maho_app::AssembledFrame;
use maho_proto::{ControlMessage, FrameHeader};
use std::sync::mpsc;

const DEADLINE: Duration = Duration::from_secs(3);

fn frame(id: u32, key: bool) -> SessionEvent {
    SessionEvent::Frame(AssembledFrame {
        header: FrameHeader {
            frame_id: id,
            width: 2,
            height: 2,
            is_key_frame: key,
            total_chunks: 1,
            total_size: 1,
        },
        data: vec![1],
        timestamp_ms: id,
    })
}

fn encoded_frame(id: u32, fixture_index: usize) -> SessionEvent {
    let hex = include_str!("../../../maho-app/tests/fixtures/hevc-continuity.hex")
        .lines()
        .nth(fixture_index)
        .unwrap();
    let data: Vec<_> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let key = maho_decode::parse_length_prefixed_nalus(&data)
        .unwrap()
        .iter()
        .any(|nalu| matches!(nalu.nal_type, 19 | 20));
    SessionEvent::Frame(AssembledFrame {
        header: FrameHeader {
            frame_id: id,
            width: 32,
            height: 32,
            is_key_frame: key,
            total_chunks: 1,
            total_size: data.len() as u32,
        },
        data,
        timestamp_ms: id,
    })
}

#[test]
fn media_gap_requests_once_and_suppresses_dependents() {
    // Subscribe to decode and receive barriers before triggering the actual
    // connect media dispatch with the production codec adapter and real HEVC.
    // Only pixel publication and transport are replaced by channel observers.
    let (input_tx, input_rx) = mpsc::channel();
    let (decoded_tx, decoded_rx) = mpsc::channel();
    let (barrier_tx, barrier_rx) = mpsc::channel();
    let submitted = Arc::new(Mutex::new(Vec::new()));
    let decoded = submitted.clone();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let sent = requests.clone();
    let worker = thread::spawn(move || {
        let mut decoder = None;
        run_media_pipeline(
            Arc::new(AtomicBool::new(false)),
            || {
                input_rx
                    .recv_timeout(DEADLINE)
                    .expect("media event deadline")
            },
            move |message| {
                sent.lock().unwrap().push(message);
                Ok(())
            },
            move |frame, _| {
                let id = frame.header.frame_id;
                decoded.lock().unwrap().push(id);
                let pixels = decode_media_frame(&mut decoder, &frame)?;
                assert_eq!(pixels.len(), 1);
                assert_eq!(pixels[0].timestamp_ms, id as i64);
                if matches!(id, 0 | 4 | 5) {
                    decoded_tx.send(id).unwrap();
                }
                Ok(())
            },
            move |event| {
                assert!(matches!(event, SessionEvent::Ping));
                barrier_tx.send(()).unwrap();
            },
        );
    });
    input_tx.send(Ok(encoded_frame(0, 0))).unwrap();
    assert_eq!(decoded_rx.recv_timeout(DEADLINE).unwrap(), 0);
    input_tx.send(Ok(encoded_frame(2, 2))).unwrap();
    input_tx.send(Ok(encoded_frame(3, 3))).unwrap();
    input_tx.send(Ok(SessionEvent::Ping)).unwrap();
    barrier_rx.recv_timeout(DEADLINE).unwrap();
    let before_keyframe = requests.lock().unwrap().clone();
    input_tx.send(Ok(encoded_frame(4, 8))).unwrap();
    assert_eq!(decoded_rx.recv_timeout(DEADLINE).unwrap(), 4);
    input_tx.send(Ok(encoded_frame(5, 9))).unwrap();
    assert_eq!(decoded_rx.recv_timeout(DEADLINE).unwrap(), 5);
    input_tx.send(Err(SessionError::NotReady)).unwrap();
    worker.join().unwrap();
    assert_eq!(*submitted.lock().unwrap(), [0, 4, 5]);
    assert_eq!(before_keyframe, [ControlMessage::RequestKeyFrame]);
    assert_eq!(*requests.lock().unwrap(), [ControlMessage::RequestKeyFrame]);
}

#[test]
fn blocked_decode_does_not_block_reception_and_stop_joins() {
    let state = AppState::default();
    let stop = state.worker_stop_flag();
    let decode_stop = stop.clone();
    let (input_tx, input_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (barrier_tx, barrier_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        run_media_pipeline(
            stop,
            || {
                input_rx
                    .recv_timeout(DEADLINE)
                    .expect("media event deadline")
            },
            |_| Ok(()),
            move |_, _| {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                assert!(decode_stop.load(Ordering::SeqCst));
                Ok(())
            },
            move |_| barrier_tx.send(()).unwrap(),
        );
        done_tx.send(()).unwrap();
    });
    input_tx.send(Ok(frame(0, true))).unwrap();
    entered_rx.recv_timeout(DEADLINE).unwrap();
    input_tx.send(Ok(SessionEvent::Ping)).unwrap();
    let received_while_decoding = barrier_rx.recv_timeout(DEADLINE);
    state.stop_media_flag.store(true, Ordering::SeqCst);
    // Wake the channel transport, just as session.disconnect wakes UDP.
    input_tx.send(Err(SessionError::NotReady)).unwrap();
    release_tx.send(()).unwrap();
    done_rx.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();
    assert!(
        received_while_decoding.is_ok(),
        "decode blocked UDP event reception"
    );
}

#[test]
fn decode_failure_requests_recovery_and_suppresses_dependents() {
    for initialization_failure in [false, true] {
        let (input_tx, input_rx) = mpsc::channel();
        let (decoded_tx, decoded_rx) = mpsc::channel();
        let (request_tx, request_rx) = mpsc::channel();
        let (barrier_tx, barrier_rx) = mpsc::channel();
        let submitted = Arc::new(Mutex::new(Vec::new()));
        let observed = submitted.clone();
        let worker = thread::spawn(move || {
            let mut decoder = None;
            run_media_pipeline(
                Arc::new(AtomicBool::new(false)),
                || {
                    input_rx
                        .recv_timeout(DEADLINE)
                        .expect("media event deadline")
                },
                move |message| {
                    request_tx.send(message).unwrap();
                    Ok(())
                },
                move |frame, _| {
                    observed.lock().unwrap().push(frame.header.frame_id);
                    match decode_media_frame(&mut decoder, &frame) {
                        Ok(pixels) => {
                            assert_eq!(pixels.len(), 1);
                            decoded_tx.send(pixels[0].timestamp_ms).unwrap();
                            Ok(())
                        }
                        Err(error) => {
                            assert!(decoder.is_none(), "failed decoder must be reset");
                            Err(error)
                        }
                    }
                },
                move |_| barrier_tx.send(()).unwrap(),
            );
        });
        if initialization_failure {
            input_tx.send(Ok(frame(0, true))).unwrap();
        } else {
            input_tx.send(Ok(encoded_frame(0, 0))).unwrap();
            assert_eq!(decoded_rx.recv_timeout(DEADLINE).unwrap(), 0);
            // Truncated AU reaches the already initialized production decoder.
            input_tx.send(Ok(frame(1, false))).unwrap();
        }
        assert_eq!(
            request_rx.recv_timeout(DEADLINE).unwrap(),
            ControlMessage::RequestKeyFrame
        );
        for id in 2..4 {
            input_tx.send(Ok(encoded_frame(id, id as usize))).unwrap();
        }
        input_tx.send(Ok(SessionEvent::Ping)).unwrap();
        barrier_rx.recv_timeout(DEADLINE).unwrap();
        input_tx.send(Ok(encoded_frame(4, 8))).unwrap();
        assert_eq!(decoded_rx.recv_timeout(DEADLINE).unwrap(), 4);
        input_tx.send(Ok(encoded_frame(5, 9))).unwrap();
        assert_eq!(decoded_rx.recv_timeout(DEADLINE).unwrap(), 5);
        input_tx.send(Err(SessionError::NotReady)).unwrap();
        worker.join().unwrap();
        let expected = if initialization_failure {
            vec![0, 4, 5]
        } else {
            vec![0, 1, 4, 5]
        };
        assert_eq!(*submitted.lock().unwrap(), expected);
        assert_eq!(request_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }
}

#[test]
fn decoder_panic_reaches_media_owner_after_join() {
    let (input_tx, input_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let result = std::panic::catch_unwind(|| {
            run_media_pipeline(
                Arc::new(AtomicBool::new(false)),
                || {
                    input_rx
                        .recv_timeout(DEADLINE)
                        .expect("media event deadline")
                },
                |_| Ok(()),
                move |_, _| {
                    entered_tx.send(()).unwrap();
                    panic!("decoder fixture failure");
                },
                |_| {},
            );
        });
        done_tx.send(result.is_err()).unwrap();
    });
    input_tx.send(Ok(frame(0, true))).unwrap();
    entered_rx.recv_timeout(DEADLINE).unwrap();
    input_tx.send(Err(SessionError::NotReady)).unwrap();
    assert!(done_rx.recv_timeout(DEADLINE).unwrap());
    worker.join().unwrap();
}

#[test]
fn desktop_client_retains_pairing_for_worker_respawn_recovery() {
    // Given: the Windows host service replaced its session worker, which is what
    // entering or leaving the logon, lock or UAC secure desktop does. The
    // desktop client must be able to re-handshake, which requires it to have
    // kept the pairing id from the original connect.
    let state = crate::AppState::default();
    *state.active_pairing_id.lock().unwrap() = Some("pairing-abc".to_string());

    let retained = state.active_pairing_id.lock().unwrap().clone();
    assert_eq!(retained.as_deref(), Some("pairing-abc"));

    // Then: worker-loss failures are the ones a reconnect can fix.
    assert!(maho_app::should_reconnect(
        &maho_app::SessionError::NotReady
    ));
    assert!(!maho_app::should_reconnect(
        &maho_app::SessionError::NoAddress
    ));
}

fn sample_input_payload() -> crate::InputPayload {
    crate::InputPayload {
        event_type: "mouseMove".to_string(),
        x: 0.5,
        y: 0.5,
        view_width: 1920.0,
        view_height: 1080.0,
        key_code: None,
        modifiers: 0,
        scroll_dx: 0.0,
        scroll_dy: 0.0,
    }
}

#[test]
fn send_input_refuses_while_disconnecting_and_without_a_session() {
    use std::sync::atomic::Ordering;

    let state = crate::AppState::default();

    // Given: no session yet. Input is refused instead of panicking on None.
    let error = match crate::commands::send_input_guarded(&state, &sample_input_payload()) {
        Ok(_) => panic!("input without a session must fail"),
        Err(error) => error,
    };
    assert!(error.contains("not initialized"), "unexpected: {error}");

    // Given: teardown has begun. A late key-down here would escape after
    // cleanup already released the host's keys, so it is refused first.
    state.stop_media_flag.store(true, Ordering::SeqCst);
    let error = match crate::commands::send_input_guarded(&state, &sample_input_payload()) {
        Ok(_) => panic!("input during teardown must fail"),
        Err(error) => error,
    };
    assert!(error.contains("disconnecting"), "unexpected: {error}");
}
