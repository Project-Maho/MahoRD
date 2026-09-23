use super::*;

#[derive(Default)]
struct DeviceProbe {
    queues: Vec<AudioQueue>,
    events: Vec<String>,
    callback_error: Option<String>,
}

struct FakeBackend(Arc<Mutex<DeviceProbe>>);
struct FakeOutput {
    queue: AudioQueue,
    probe: Arc<Mutex<DeviceProbe>>,
    device: String,
}

impl DesktopAudioOutput for FakeOutput {
    fn status(&self) -> Result<AudioOutputStatus, String> {
        Ok(AudioOutputStatus {
            last_error: self.probe.lock().unwrap().callback_error.clone(),
            ..Default::default()
        })
    }
}

impl Drop for FakeOutput {
    fn drop(&mut self) {
        self.queue.clear().unwrap();
        self.probe
            .lock()
            .unwrap()
            .events
            .push(format!("drop:{}", self.device));
    }
}

impl DesktopAudioBackend for FakeBackend {
    fn devices(&mut self) -> Result<Vec<DesktopAudioDevice>, String> {
        Ok(["selected", "removed"]
            .map(|id| DesktopAudioDevice {
                id: id.into(),
                name: id.into(),
                supported: true,
            })
            .to_vec())
    }

    fn open(
        &mut self,
        queue: AudioQueue,
        device: Option<&str>,
    ) -> Result<Box<dyn DesktopAudioOutput>, String> {
        let device = device.unwrap_or("default").to_string();
        let mut probe = self.0.lock().unwrap();
        probe.events.push(format!("open:{device}"));
        if device == "removed" {
            return Err("selected device removed".into());
        }
        probe.queues.push(queue.clone());
        Ok(Box::new(FakeOutput {
            queue,
            probe: self.0.clone(),
            device,
        }))
    }
}

fn audio_state() -> (AppState, Arc<Mutex<DeviceProbe>>) {
    let state = AppState::default();
    let probe = Arc::new(Mutex::new(DeviceProbe::default()));
    let observed = probe.clone();
    *state.audio_runtime.lock().unwrap() = Some(
        AudioRuntime::spawn_with_backend(state.audio_playback.clone(), move || {
            FakeBackend(observed)
        })
        .unwrap(),
    );
    (state, probe)
}

async fn request(state: &AppState, action: AudioAction) -> Result<DesktopAudioStatus, String> {
    tokio::time::timeout(Duration::from_secs(5), state.audio_request(action))
        .await
        .unwrap()
}

fn media(state: &AppState, samples: &[f32]) {
    dispatch_media_event(
        SessionEvent::Audio(
            samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect(),
        ),
        &state.audio_packets_received,
        &state.latest_cursor,
        &state.audio_playback,
    );
}

fn drain(queue: &AudioQueue) -> [f32; 4] {
    let mut samples = [123.0; 4];
    queue.drain_into(&mut samples).unwrap();
    samples
}

#[tokio::test]
async fn audio_commands_apply_gain_mute_and_validate() {
    let (state, probe) = audio_state();
    assert!(request(&state, AudioAction::Start).await.unwrap().active);
    let queue = probe.lock().unwrap().queues[0].clone();
    assert_eq!(
        request(&state, AudioAction::Volume(0.25))
            .await
            .unwrap()
            .volume,
        0.25
    );
    media(&state, &[0.5, -1.0]);
    assert_eq!(drain(&queue), [0.125, -0.25, 0.0, 0.0]);
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.1, 1.1] {
        assert!(request(&state, AudioAction::Volume(invalid)).await.is_err());
        assert_eq!(
            request(&state, AudioAction::Status).await.unwrap().volume,
            0.25
        );
    }
    assert!(
        request(&state, AudioAction::Muted(true))
            .await
            .unwrap()
            .muted
    );
    media(&state, &[0.5, -1.0]);
    assert_eq!(drain(&queue), [0.0; 4]);
    assert_eq!(queue.queued_samples().unwrap(), 0);
    request(&state, AudioAction::Muted(false)).await.unwrap();
    assert_eq!(drain(&queue), [0.0; 4]);
    for volume in [0.0, 1.0] {
        request(&state, AudioAction::Volume(volume)).await.unwrap();
        media(&state, &[0.5, -1.0]);
        assert_eq!(drain(&queue), [0.5 * volume, -volume, 0.0, 0.0]);
    }
}

#[tokio::test]
async fn device_switch_drops_old_output_and_never_replays_old_pcm() {
    let (state, probe) = audio_state();
    request(&state, AudioAction::Start).await.unwrap();
    let old = probe.lock().unwrap().queues[0].clone();
    media(&state, &[0.5, -1.0]);
    request(&state, AudioAction::Device(Some("selected".into())))
        .await
        .unwrap();
    let new = probe.lock().unwrap().queues[1].clone();
    assert_eq!(old.queued_samples().unwrap(), 0);
    assert_eq!(drain(&new), [0.0; 4]);
    old.push_samples([0.75, -0.75]).unwrap();
    assert_eq!(drain(&new), [0.0; 4], "old producer clone is isolated");
    media(&state, &[0.125, -0.25]);
    assert_eq!(drain(&new), [0.125, -0.25, 0.0, 0.0]);
    media(&state, &[0.5, -1.0]);
    assert_eq!(
        request(&state, AudioAction::Device(Some("removed".into())))
            .await
            .unwrap_err(),
        "selected device removed"
    );
    let status = request(&state, AudioAction::Status).await.unwrap();
    assert!(!status.active);
    assert_eq!(status.error.as_deref(), Some("selected device removed"));
    assert!(state.audio_playback.lock().unwrap().queue.is_none());
    assert_eq!(new.queued_samples().unwrap(), 0);
    media(&state, &[0.5, -1.0]);
    assert_eq!(new.queued_samples().unwrap(), 0);
    assert_eq!(
        probe.lock().unwrap().events,
        [
            "open:default",
            "drop:default",
            "open:selected",
            "drop:selected",
            "open:removed"
        ]
    );
    request(&state, AudioAction::Device(None)).await.unwrap();
    assert!(request(&state, AudioAction::Device(Some("unknown".into())))
        .await
        .is_err());
    assert!(!request(&state, AudioAction::Status).await.unwrap().active);
}

#[tokio::test]
async fn stop_and_restart_isolate_audio_and_failed_start() {
    let (state, probe) = audio_state();
    request(&state, AudioAction::Device(Some("selected".into())))
        .await
        .unwrap();
    assert!(
        probe.lock().unwrap().queues.is_empty(),
        "preference alone cannot start playback"
    );
    request(&state, AudioAction::Start).await.unwrap();
    let old = probe.lock().unwrap().queues[0].clone();
    media(&state, &[0.5, -1.0]);
    disconnect_internal(&state).await.unwrap();
    assert_eq!(old.queued_samples().unwrap(), 0);
    media(&state, &[0.5, -1.0]);
    request(&state, AudioAction::Start).await.unwrap();
    assert_eq!(drain(&probe.lock().unwrap().queues[1]), [0.0; 4]);
    request(&state, AudioAction::Stop).await.unwrap();
    request(&state, AudioAction::Device(Some("removed".into())))
        .await
        .unwrap();
    assert!(request(&state, AudioAction::Start).await.is_err());
    disconnect_internal(&state).await.unwrap();
    assert!(state.audio_playback.lock().unwrap().queue.is_none());
    assert!(!request(&state, AudioAction::Status).await.unwrap().active);
    assert_eq!(
        probe.lock().unwrap().events,
        [
            "open:selected",
            "drop:selected",
            "open:selected",
            "drop:selected",
            "open:removed"
        ]
    );
}

#[tokio::test]
async fn malformed_media_audio_is_atomic_and_visible() {
    let (state, probe) = audio_state();
    request(&state, AudioAction::Start).await.unwrap();
    let queue = probe.lock().unwrap().queues[0].clone();
    media(&state, &[0.25, -0.5]);
    media(&state, &[0.75, -1.0, f32::NAN, 0.0]);
    assert!(request(&state, AudioAction::Status)
        .await
        .unwrap()
        .error
        .is_some());
    assert_eq!(drain(&queue), [0.25, -0.5, 0.0, 0.0]);
    media(&state, &[0.25]);
    assert_eq!(queue.queued_samples().unwrap(), 0);
    assert!(request(&state, AudioAction::Status)
        .await
        .unwrap()
        .error
        .unwrap()
        .contains("aligned"));
    media(&state, &vec![0.125; 10_000]);
    assert_eq!(
        queue.queued_samples().unwrap(),
        maho_render::AUDIO_QUEUE_CAPACITY
    );
}

#[tokio::test]
async fn callback_error_is_visible_and_stops_failed_output() {
    let (state, probe) = audio_state();
    request(&state, AudioAction::Start).await.unwrap();
    media(&state, &[0.25, -0.5]);
    probe.lock().unwrap().callback_error = Some("physical output disappeared".into());
    let status = request(&state, AudioAction::Status).await.unwrap();
    assert!(!status.active);
    assert_eq!(status.error.as_deref(), Some("physical output disappeared"));
    assert_eq!(probe.lock().unwrap().queues[0].queued_samples().unwrap(), 0);
    assert!(state.audio_playback.lock().unwrap().queue.is_none());
}

#[tokio::test]
async fn unavailable_session_output_can_be_explicitly_recovered() {
    let (state, probe) = audio_state();
    request(&state, AudioAction::Device(Some("removed".into())))
        .await
        .unwrap();
    state.start_session_audio().await;
    let status = request(&state, AudioAction::Status).await.unwrap();
    assert!(!status.active);
    assert_eq!(status.error.as_deref(), Some("selected device removed"));
    assert_eq!(probe.lock().unwrap().events, ["open:removed"]);
    // The actual connect audio seam keeps video usable, but never falls back.
    // The user can explicitly select default from the session controls.
    let recovered = request(&state, AudioAction::Device(None)).await.unwrap();
    assert!(recovered.active);
    assert!(recovered.error.is_none());
    disconnect_internal(&state).await.unwrap();
    assert_eq!(
        probe.lock().unwrap().events,
        ["open:removed", "open:default", "drop:default"]
    );
}

#[tokio::test]
async fn disconnect_clears_agent_keys_even_without_transport() {
    let state = AppState::default();
    state
        .agent_tracker
        .lock()
        .unwrap()
        .record_key_down(0, Modifiers::SHIFT);
    *state.agent_pos.lock().unwrap() = (0.25, 0.75);
    disconnect_internal(&state).await.unwrap();
    assert!(state.agent_tracker.lock().unwrap().is_empty());
    assert_eq!(*state.agent_pos.lock().unwrap(), (0.5, 0.5));
    assert!(state.stop_media_flag.load(Ordering::SeqCst));
}

fn payload(kind: &str) -> InputPayload {
    serde_json::from_value(serde_json::json!({
        "event_type": kind, "x": 32.0, "y": 16.0,
        "view_width": 128.0, "view_height": 64.0,
        "scroll_dx": 24.0, "scroll_dy": -16.0
    }))
    .unwrap()
}

#[test]
fn middle_and_relative_payloads_use_legacy_variants() {
    for (kind, expected, id) in [
        ("MiddleMouseDown", InputEventType::MiddleMouseDown, 11),
        ("MiddleMouseUp", InputEventType::MiddleMouseUp, 12),
        ("RelativeMove", InputEventType::RelativeMove, 14),
    ] {
        let event = convert_input_payload(&payload(kind)).unwrap();
        assert_eq!(event.event_type, expected);
        assert_eq!(event.event_type as u8, id);
        assert_eq!((event.scroll_dx, event.scroll_dy), (24.0, -16.0));
        assert_eq!((event.x, event.y), (0.25, 0.75));
    }
}

#[test]
fn gamepad_and_pen_payloads_convert_properly() {
    // Gamepad axis
    let gp_axis = InputPayload {
        event_type: "GamepadAxis".into(),
        x: 0.5,
        y: -0.5,
        key_code: Some(0x0102),
        modifiers: 0,
        view_width: 0.0,
        view_height: 0.0,
        scroll_dx: 0.0,
        scroll_dy: 0.0,
    };
    let event = convert_input_payload(&gp_axis).unwrap();
    assert_eq!(event.event_type, InputEventType::GamepadAxis);
    assert_eq!(event.x, 0.5);
    assert_eq!(event.y, -0.5);
    assert_eq!(event.key_code, 0x0102);

    // Gamepad button
    let gp_btn = InputPayload {
        event_type: "GamepadButtonDown".into(),
        x: 0.0,
        y: 0.0,
        key_code: Some(0x0001),
        modifiers: 0,
        view_width: 0.0,
        view_height: 0.0,
        scroll_dx: 0.0,
        scroll_dy: 0.0,
    };
    let event = convert_input_payload(&gp_btn).unwrap();
    assert_eq!(event.event_type, InputEventType::GamepadButtonDown);
    assert_eq!(event.key_code, 0x0001);

    // Pen down with pressure and tilt
    let pen_payload = InputPayload {
        event_type: "PenDown".into(),
        x: 50.0,
        y: 25.0,
        key_code: Some(10), // tilt_y
        modifiers: 0,
        view_width: 100.0,
        view_height: 100.0,
        scroll_dx: 0.85, // pressure
        scroll_dy: 12.0, // tilt_x
    };
    let event = convert_input_payload(&pen_payload).unwrap();
    assert_eq!(event.event_type, InputEventType::PenDown);
    assert_eq!(event.x, 0.5);
    assert_eq!(event.y, 0.75); // flipped Y
    assert_eq!(event.scroll_dx, 0.85);
    assert_eq!(event.scroll_dy, 12.0);
    assert_eq!(event.key_code, 10);
}

#[test]
fn unknown_buttons_reject() {
    for kind in [
        "ExtraMouseDown",
        "BackMouseDown",
        "ForwardMouseDown",
        "15",
        "",
    ] {
        assert!(convert_input_payload(&payload(kind)).is_err());
    }
}

#[test]
fn non_video_dispatch_characterization() {
    let state = AppState::default();
    let cursor = CursorState {
        x: 0.25,
        y: 0.75,
        cursor_type: 1,
    };
    for event in [
        SessionEvent::Audio(vec![0; 8]),
        SessionEvent::Cursor(cursor),
        SessionEvent::Ping,
    ] {
        dispatch_media_event(
            event,
            &state.audio_packets_received,
            &state.latest_cursor,
            &state.audio_playback,
        );
    }
    assert_eq!(state.audio_packets_received.load(Ordering::Relaxed), 1);
    let observed = *state.latest_cursor.lock().unwrap();
    assert_eq!(
        (observed.x, observed.y, observed.cursor_type),
        (0.25, 0.75, 1)
    );
}

#[test]
fn media_audio_reaches_native_queue() {
    let state = AppState::default();
    let queue = AudioQueue::default();
    state.audio_playback.lock().unwrap().queue = Some(queue.clone());
    let samples = [0.25f32, -0.5, 0.75, -1.0];
    let bytes = samples.into_iter().flat_map(f32::to_le_bytes).collect();
    let mut input = [Ok(SessionEvent::Audio(bytes)), Err(SessionError::NotReady)].into_iter();
    run_media_pipeline(
        state.worker_stop_flag(),
        || input.next().unwrap(),
        |_| Ok(()),
        |_, _| panic!("audio must not enter video decode"),
        |event| {
            dispatch_media_event(
                event,
                &state.audio_packets_received,
                &state.latest_cursor,
                &state.audio_playback,
            )
        },
    );
    assert_eq!(state.audio_packets_received.load(Ordering::Relaxed), 1);
    let mut rendered = [0.0; 6];
    let consumed = queue.drain_into(&mut rendered).unwrap();
    assert_eq!(
        rendered,
        [0.25, -0.5, 0.75, -1.0, 0.0, 0.0],
        "actual media dispatch must enqueue PCM rather than only count it"
    );
    assert_eq!(consumed, 4);
}

#[tokio::test(flavor = "current_thread")]
async fn disconnect_yields_while_input_submission_holds_session_lock() {
    // Given an input submission holding the actual reset/teardown mutex.
    let state = Arc::new(AppState::default());
    let input_state = state.clone();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let input = thread::spawn(move || {
        let _session = input_state.session.lock().unwrap();
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).is_ok()
    });
    entered_rx.await.unwrap();

    // When teardown is polled, it must leave the executor free to release input.
    let disconnect = disconnect_internal(&state);
    tokio::pin!(disconnect);
    let yielded = std::future::poll_fn(|cx| {
        use std::future::Future;
        std::task::Poll::Ready(disconnect.as_mut().poll(cx).is_pending())
    })
    .await;
    let released = release_tx.send(()).is_ok();
    let released_by_executor = input.join().unwrap();
    tokio::time::timeout(Duration::from_secs(5), disconnect)
        .await
        .unwrap()
        .unwrap();

    // Then progress, not the deadlock watchdog, unblocks the input submission.
    assert!(yielded, "teardown must yield while input is in flight");
    assert!(
        released && released_by_executor,
        "teardown blocked the executor on the input submission mutex"
    );
}

#[test]
fn pairing_store_round_trip_and_deletion() {
    // Given: isolated temporary pairing store with records.
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("maho-pairing-test-{id}"));
    let _ = std::fs::remove_dir_all(&directory);
    let store = PairingStore::new(directory.join("pairings.json"));
    let record1 = maho_app::PairingRecord {
        id: "id-1".into(),
        name: "host-1".into(),
        key: vec![1; 32],
        added_at_unix_ms: 1000,
        last_endpoint: None,
        endpoint_aliases: Vec::new(),
        relay_url: None,
        relay_host_id: None,
    };
    let record2 = maho_app::PairingRecord {
        id: "id-2".into(),
        name: "host-2".into(),
        key: vec![2; 32],
        added_at_unix_ms: 2000,
        last_endpoint: None,
        endpoint_aliases: Vec::new(),
        relay_url: None,
        relay_host_id: None,
    };
    store.save(record1.clone()).unwrap();
    store.save(record2.clone()).unwrap();

    // When: records are loaded.
    let loaded = store.load_all().unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(store.load("id-1").unwrap(), Some(record1));

    // When: one record is deleted.
    store.delete("id-1").unwrap();

    // Then: deleted record is forgotten and second record persists.
    assert_eq!(store.load("id-1").unwrap(), None);
    assert_eq!(store.load("id-2").unwrap(), Some(record2));
    assert_eq!(store.load_all().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&directory);
}
