// allow: SIZE_OK — centralized iOS shell native state machine and supervisor
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

use maho_app::{
    ClientSession, IpcError, IpcErrorCode, IpcErrorStage, PairingEndpoint, PairingStore,
    ReadySession, SessionConfig, SessionError, SessionEvent, SessionRuntime,
};
use maho_mobile::{TouchGestureHandler, TouchMode, TouchPhase, TouchPoint, ViewportState};
use maho_proto::{ControlMessage, InputEvent, InputEventType, Modifiers};
use maho_render::{AudioOutputEvent, AudioQueue, CpalAudioOutput};
use serde::{Deserialize, Serialize};

use crate::frame::repack_nv12_frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerKind {
    Supervisor,
    Audio,
    Media,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerCompletion {
    pub generation: u64,
    pub kind: WorkerKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Idle,
    Connecting,
    Ready,
    Disconnected,
    Error,
}

impl ConnectionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Connecting => "connecting",
            Self::Ready => "ready",
            Self::Disconnected => "disconnected",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionStats {
    pub state: String,
    pub host: Option<String>,
    pub frames_received: u64,
    pub frames_decoded: u64,
    pub audio_packets_received: u64,
    pub audio_samples_played: u64,
    pub width: u32,
    pub height: u32,
    pub last_error: Option<String>,
}

pub struct SessionInner {
    pub generation: u64,
    pub state: ConnectionState,
    pub host: Option<String>,
    pub session: Option<ClientSession>,
    pub touch_handler: TouchGestureHandler,
    pub touch_mode: TouchMode,
    pub audio_queue: AudioQueue,
    pub tcp_runtime: Option<SessionRuntime>,
    pub worker_handles: Vec<thread::JoinHandle<()>>,
    pub stop_flag: Arc<AtomicBool>,
    pub frames_received: Arc<AtomicU64>,
    pub frames_decoded: Arc<AtomicU64>,
    pub audio_packets_received: Arc<AtomicU64>,
    pub audio_samples_played: Arc<AtomicU64>,
    pub video_width: Arc<AtomicU32>,
    pub video_height: Arc<AtomicU32>,
    pub frame_sequence: Arc<AtomicU64>,
    pub last_polled_sequence: Arc<AtomicU64>,
    pub latest_frame: Arc<Mutex<Option<Arc<Vec<u8>>>>>,
    pub last_error: Arc<Mutex<Option<String>>>,
    pub first_frame_presented: Arc<AtomicBool>,
    pub worker_completions: Arc<Mutex<Vec<mpsc::Sender<WorkerCompletion>>>>,
    pub pairing_store: Option<PairingStore>,
    pub headless_audio: bool,
    pub audio_events_sender: Arc<Mutex<Option<mpsc::SyncSender<AudioOutputEvent>>>>,
}

impl Default for SessionInner {
    fn default() -> Self {
        let viewport = ViewportState::new(1.0, 1.0).unwrap_or_default();
        Self {
            generation: 0,
            state: ConnectionState::Idle,
            host: None,
            session: None,
            touch_handler: TouchGestureHandler::new(viewport, TouchMode::DirectTouch),
            touch_mode: TouchMode::DirectTouch,
            audio_queue: AudioQueue::default(),
            tcp_runtime: None,
            worker_handles: Vec::new(),
            stop_flag: Arc::new(AtomicBool::new(false)),
            frames_received: Arc::new(AtomicU64::new(0)),
            frames_decoded: Arc::new(AtomicU64::new(0)),
            audio_packets_received: Arc::new(AtomicU64::new(0)),
            audio_samples_played: Arc::new(AtomicU64::new(0)),
            video_width: Arc::new(AtomicU32::new(0)),
            video_height: Arc::new(AtomicU32::new(0)),
            frame_sequence: Arc::new(AtomicU64::new(0)),
            last_polled_sequence: Arc::new(AtomicU64::new(0)),
            latest_frame: Arc::new(Mutex::new(None)),
            last_error: Arc::new(Mutex::new(None)),
            first_frame_presented: Arc::new(AtomicBool::new(false)),
            worker_completions: Arc::new(Mutex::new(Vec::new())),
            pairing_store: None,
            headless_audio: false,
            audio_events_sender: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(Clone)]
pub struct ActiveConnectState {
    pub generation: u64,
    pub cancel_flag: Arc<AtomicBool>,
    pub session: Arc<Mutex<Option<ClientSession>>>,
}

#[derive(Default)]
pub struct TeardownCoordInner {
    pub in_flight_generation: Option<u64>,
    pub done_generation: Option<u64>,
    pub last_result: Option<Result<(), String>>,
}

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Mutex<SessionInner>>,
    pub lifecycle_lock: Arc<tokio::sync::Mutex<()>>,
    pub active_connect: Arc<Mutex<Option<ActiveConnectState>>>,
    pub discovery: Arc<Mutex<Option<maho_net::discovery::LanDiscovery>>>,
    pub cancel_pending: Arc<AtomicBool>,
    pub teardown_coord: Arc<(Mutex<TeardownCoordInner>, std::sync::Condvar)>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

fn notify_worker_completion(
    senders: &Arc<Mutex<Vec<mpsc::Sender<WorkerCompletion>>>>,
    completion: WorkerCompletion,
) {
    if let Ok(mut list) = senders.lock() {
        list.retain(|sender| sender.send(completion).is_ok());
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SessionInner::default())),
            lifecycle_lock: Arc::new(tokio::sync::Mutex::new(())),
            active_connect: Arc::new(Mutex::new(None)),
            discovery: Arc::new(Mutex::new(None)),
            cancel_pending: Arc::new(AtomicBool::new(false)),
            teardown_coord: Arc::new((
                Mutex::new(TeardownCoordInner::default()),
                std::sync::Condvar::new(),
            )),
        }
    }

    pub fn with_pairing_store(store: PairingStore) -> Self {
        let s = Self::new();
        if let Ok(mut inner) = s.inner.lock() {
            inner.pairing_store = Some(store);
        }
        s
    }

    pub fn set_headless_audio(&self, headless: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.headless_audio = headless;
        }
    }

    pub fn inject_audio_event_for_test(&self, event: AudioOutputEvent) -> Result<(), String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "State mutex is poisoned".to_string())?;
        let sender_guard = inner
            .audio_events_sender
            .lock()
            .map_err(|_| "Audio events sender mutex is poisoned".to_string())?;
        if let Some(ref sender) = *sender_guard {
            sender
                .send(event)
                .map_err(|e| format!("Failed to send audio event: {e}"))
        } else {
            Err("No active audio event sender".to_string())
        }
    }

    pub fn subscribe_worker_completions(&self) -> mpsc::Receiver<WorkerCompletion> {
        let (tx, rx) = mpsc::channel();
        if let Ok(inner) = self.inner.lock() {
            if let Ok(mut list) = inner.worker_completions.lock() {
                list.push(tx);
            }
        }
        rx
    }

    pub fn cancel_active_connect(&self) {
        self.cancel_pending.store(true, Ordering::SeqCst);
        let active = self.active_connect.lock().ok().and_then(|mut g| g.take());
        if let Some(connect_state) = active {
            connect_state.cancel_flag.store(true, Ordering::SeqCst);
            if let Ok(sess_guard) = connect_state.session.lock() {
                if let Some(ref session) = *sess_guard {
                    session.interrupt();
                }
            }
        }
        if let Ok(inner) = self.inner.lock() {
            inner.stop_flag.store(true, Ordering::SeqCst);
        }
    }

    pub fn handle_terminal_shutdown(
        &self,
        generation: u64,
        target_state: ConnectionState,
        terminal_reason: Option<String>,
    ) -> Result<(), String> {
        let (lock, cvar) = &*self.teardown_coord;
        let mut coord = lock
            .lock()
            .map_err(|e| format!("Teardown lock poisoned: {e}"))?;

        // 1. If another thread is currently tearing down THIS generation, wait for it to finish!
        while coord.in_flight_generation == Some(generation) {
            coord = cvar
                .wait(coord)
                .map_err(|e| format!("Teardown condvar poisoned: {e}"))?;
        }

        // 2. If this generation was already reaped:
        if coord.done_generation == Some(generation) {
            let last_res = coord.last_result.clone().unwrap_or(Ok(()));
            if let Err(ref e) = last_res {
                return Err(e.clone());
            }
            if target_state == ConnectionState::Idle {
                let mut inner = self
                    .inner
                    .lock()
                    .map_err(|e| format!("State mutex poisoned: {e}"))?;
                if inner.generation == generation {
                    inner.state = ConnectionState::Idle;
                    inner.session = None;
                    inner.host = None;
                    inner.first_frame_presented.store(false, Ordering::Relaxed);
                }
            }
            return Ok(());
        }

        // 3. We become the teardown owner for this generation:
        coord.in_flight_generation = Some(generation);
        drop(coord);

        let (reap_result, applied) =
            match self.execute_teardown(generation, target_state, terminal_reason) {
                Ok(applied) => (Ok(()), applied),
                Err(e) => (Err(e), true),
            };

        // Record completion and wake up all waiting threads:
        let mut coord = lock
            .lock()
            .map_err(|e| format!("Teardown lock poisoned: {e}"))?;
        coord.in_flight_generation = None;
        // Only claim the completion slot when this teardown actually applied to the current
        // generation: a late worker from a stale generation must never overwrite the record of a
        // newer generation, otherwise the idempotency check above would re-run its teardown.
        if applied {
            coord.done_generation = Some(generation);
            coord.last_result = Some(reap_result.clone());
        }
        cvar.notify_all();

        reap_result
    }

    /// Returns `Ok(true)` when the teardown applied to the current generation, `Ok(false)` when the
    /// event was stale (a newer generation is live) and nothing was torn down.
    fn execute_teardown(
        &self,
        generation: u64,
        target_state: ConnectionState,
        terminal_reason: Option<String>,
    ) -> Result<bool, String> {
        let (sess, tcp_rt, handles) = {
            let mut inner = match self.inner.lock() {
                Ok(g) => g,
                Err(e) => return Err(format!("State mutex poisoned: {e}")),
            };

            // Generation-bound check: stale events from prior generations are ignored
            if inner.generation != generation {
                tracing::debug!(
                    inner_generation = inner.generation,
                    event_generation = generation,
                    "Ignoring stale terminal event from previous generation"
                );
                return Ok(false);
            }

            // If already fully torn down to Idle, nothing to do.
            // Do not let late errors restore terminal state after idle!
            if inner.state == ConnectionState::Idle
                && inner.session.is_none()
                && inner.worker_handles.is_empty()
            {
                return Ok(true);
            }

            // Preserve primary terminal reason: if already Disconnected (e.g. from TCP remote close),
            // secondary worker shutdowns (e.g. media receiver unblocking) must not overwrite it.
            if inner.state == ConnectionState::Disconnected
                && target_state == ConnectionState::Error
            {
                return Ok(true);
            }

            // Propagate terminal reason
            if target_state != ConnectionState::Idle {
                inner.state = target_state;
                if let Some(ref reason) = terminal_reason {
                    if let Ok(mut err_guard) = inner.last_error.lock() {
                        *err_guard = Some(reason.clone());
                    }
                }
            }

            // Signal workers to stop
            inner.stop_flag.store(true, Ordering::SeqCst);

            // Release held input
            let release_evt = inner.touch_handler.set_mode(TouchMode::DirectTouch);
            if let Some(ref session) = inner.session {
                if let Some(evt) = release_evt {
                    let _ = session.send_input(evt);
                }
                let _ = session.send_input(InputEvent {
                    event_type: InputEventType::Reset,
                    x: 0.0,
                    y: 0.0,
                    key_code: 0,
                    modifiers: Modifiers::empty(),
                    scroll_dx: 0.0,
                    scroll_dy: 0.0,
                });
            }

            let _ = inner.audio_queue.clear();
            // Drop the terminated session's audio event sender so Idle/Error
            // states report no sender instead of writing into an orphaned
            // channel, and an exiting audio thread can observe disconnection.
            if let Ok(mut sender_guard) = inner.audio_events_sender.lock() {
                *sender_guard = None;
            }
            if let Ok(mut frame_guard) = inner.latest_frame.lock() {
                *frame_guard = None;
            }

            let sess = inner.session.take();
            let tcp_rt = inner.tcp_runtime.take();
            let handles = std::mem::take(&mut inner.worker_handles);

            (sess, tcp_rt, handles)
        };

        // CRITICAL INVARIANT: Stop runtime, disconnect session, and join worker handles OUTSIDE of `inner.lock()`.
        // Native cleanup must never block or self-join while holding `inner`.
        let mut cleanup_errors = Vec::new();

        if let Some(ref session) = sess {
            if let Err(e) = session.disconnect() {
                cleanup_errors.push(format!("Session disconnect error: {e}"));
            }
        }

        if let Some(mut runtime) = tcp_rt {
            if let Err(e) = runtime.stop() {
                cleanup_errors.push(format!("TCP runtime stop error: {e}"));
            }
        }

        let cur_id = thread::current().id();
        for handle in handles {
            if handle.thread().id() != cur_id {
                if let Err(e) = handle.join() {
                    cleanup_errors.push(format!("Worker thread panicked during join: {e:?}"));
                }
            }
        }

        if !cleanup_errors.is_empty() {
            let err_summary = cleanup_errors.join("; ");
            if let Ok(mut inner) = self.inner.lock() {
                inner.state = ConnectionState::Error;
                if let Ok(mut err_guard) = inner.last_error.lock() {
                    *err_guard = Some(format!("Cleanup failed: {err_summary}"));
                }
            }
            return Err(err_summary);
        }

        if target_state == ConnectionState::Idle {
            if let Ok(mut inner) = self.inner.lock() {
                inner.state = ConnectionState::Idle;
                inner.session = None;
                inner.host = None;
                inner.first_frame_presented.store(false, Ordering::Relaxed);
            }
        }

        Ok(true)
    }

    pub async fn list_discovered_hosts(
        &self,
    ) -> Result<Vec<maho_net::discovery::DiscoveredHost>, String> {
        let state_clone = self.clone();
        tokio::task::spawn_blocking(move || state_clone.discovery_snapshot_blocking())
            .await
            .map_err(|e| format!("Discovery task failed: {e}"))?
    }

    pub async fn stop_discovery(&self) -> Result<(), String> {
        let state_clone = self.clone();
        tokio::task::spawn_blocking(move || state_clone.stop_discovery_blocking())
            .await
            .map_err(|e| format!("Stop discovery task failed: {e}"))?
    }

    pub fn stop_discovery_blocking(&self) -> Result<(), String> {
        let mut disc_guard = self
            .discovery
            .lock()
            .map_err(|_| "Discovery mutex is poisoned".to_string())?;
        if disc_guard.is_some() {
            tracing::info!("Stopping and dropping LAN discovery browser");
            *disc_guard = None;
        }
        Ok(())
    }

    pub fn discovery_snapshot_blocking(
        &self,
    ) -> Result<Vec<maho_net::discovery::DiscoveredHost>, String> {
        let mut disc_guard = self
            .discovery
            .lock()
            .map_err(|_| "Discovery mutex is poisoned".to_string())?;
        if disc_guard.is_none() {
            let browser = match maho_net::discovery::LanDiscovery::new() {
                Ok(b) => b,
                Err(e) => {
                    return Err(format!("LAN discovery initialization error: {e}"));
                }
            };
            *disc_guard = Some(browser);
        }

        if let Some(ref browser) = *disc_guard {
            match browser.snapshot() {
                Ok(hosts) => {
                    tracing::info!(count = hosts.len(), "Observed live LAN discovery snapshot");
                    Ok(hosts)
                }
                Err(e) => {
                    *disc_guard = None;
                    Err(format!("LAN discovery snapshot error: {e}"))
                }
            }
        } else {
            Ok(Vec::new())
        }
    }

    pub fn stats(&self) -> Result<SessionStats, String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "State mutex is poisoned".to_string())?;
        let last_err = inner.last_error.lock().ok().and_then(|g| g.clone());
        let mut state_str = inner.state.as_str().to_string();
        if inner.state == ConnectionState::Ready {
            if let Some(ref session) = inner.session {
                if let Ok(sess_state) = session.state() {
                    if sess_state == maho_app::SessionState::Disconnected {
                        state_str = "disconnected".to_string();
                    }
                }
            }
        }
        Ok(SessionStats {
            state: state_str,
            host: inner.host.clone(),
            frames_received: inner.frames_received.load(Ordering::Relaxed),
            frames_decoded: inner.frames_decoded.load(Ordering::Relaxed),
            audio_packets_received: inner.audio_packets_received.load(Ordering::Relaxed),
            audio_samples_played: inner.audio_samples_played.load(Ordering::Relaxed),
            width: inner.video_width.load(Ordering::Relaxed),
            height: inner.video_height.load(Ordering::Relaxed),
            last_error: last_err,
        })
    }

    pub fn poll_frame(&self) -> Result<Option<Arc<Vec<u8>>>, String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "State mutex is poisoned".to_string())?;
        let latest_seq = inner.frame_sequence.load(Ordering::Relaxed);
        let last_polled = inner.last_polled_sequence.load(Ordering::Relaxed);
        if latest_seq == 0 || latest_seq <= last_polled {
            return Ok(None);
        }
        let frame = inner.latest_frame.lock().ok().and_then(|g| g.clone());
        if frame.is_some() {
            inner
                .last_polled_sequence
                .store(latest_seq, Ordering::Relaxed);
        }
        Ok(frame)
    }

    pub fn set_muted(&self, muted: bool) -> Result<(), String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "State mutex is poisoned".to_string())?;
        inner
            .audio_queue
            .set_muted(muted)
            .map_err(|e| format!("Failed to set audio muted: {e}"))
    }

    pub fn set_touch_mode(&self, mode_str: &str) -> Result<(), String> {
        let new_mode = match mode_str {
            "direct" => TouchMode::DirectTouch,
            "trackpad" => TouchMode::TrackpadRelative,
            other => return Err(format!("Invalid touch mode: {other}")),
        };

        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "State mutex is poisoned".to_string())?;

        if inner.touch_mode == new_mode {
            return Ok(());
        }

        let release_evt = inner.touch_handler.set_mode(new_mode);
        inner.touch_mode = new_mode;

        if let Some(evt) = release_evt {
            if let Some(ref session) = inner.session {
                let _ = session.send_input(evt);
            }
        }
        Ok(())
    }

    pub fn handle_touch(&self, id: u64, x: f32, y: f32, phase_str: &str) -> Result<(), String> {
        let evt = match self.process_touch_for_test(id, x, y, phase_str)? {
            Some(evt) => evt,
            None => return Ok(()),
        };
        let inner = self
            .inner
            .lock()
            .map_err(|_| "State mutex is poisoned".to_string())?;
        if let Some(ref session) = inner.session {
            session
                .send_input(evt)
                .map_err(|e| format!("Failed to send touch input: {e}"))?;
        }
        Ok(())
    }

    pub fn process_touch_for_test(
        &self,
        id: u64,
        x: f32,
        y: f32,
        phase_str: &str,
    ) -> Result<Option<InputEvent>, String> {
        let phase = match phase_str {
            "began" => TouchPhase::Began,
            "moved" => TouchPhase::Moved,
            "ended" => TouchPhase::Ended,
            "cancelled" => TouchPhase::Cancelled,
            other => return Err(format!("Invalid touch phase: {other}")),
        };

        if (phase == TouchPhase::Began || phase == TouchPhase::Moved)
            && (!x.is_finite() || !y.is_finite())
        {
            return Err("Non-finite touch coordinates".to_string());
        }

        let point = TouchPoint { id, x, y, phase };

        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "State mutex is poisoned".to_string())?;

        let evt_res = inner.touch_handler.process_touch(point);
        let mut evt = match evt_res {
            Ok(Some(evt)) => evt,
            Ok(None) => return Ok(None),
            Err(err) => return Err(format!("Touch processing error: {err}")),
        };

        if evt.event_type == InputEventType::RelativeMove {
            let w = inner.video_width.load(Ordering::Relaxed);
            let h = inner.video_height.load(Ordering::Relaxed);
            let (scale_w, scale_h) = if w > 0 && h > 0 {
                (w as f32, h as f32)
            } else {
                (1920.0, 1080.0)
            };
            evt.scroll_dx *= scale_w;
            evt.scroll_dy *= scale_h;
        }

        Ok(Some(evt))
    }

    pub fn handle_key(&self, key_code: u16, down: bool, modifiers_bits: u16) -> Result<(), String> {
        if key_code == 0 {
            return Err("Invalid key code 0".to_string());
        }
        const VALID_MODIFIERS_MASK: u16 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4);
        if modifiers_bits & !VALID_MODIFIERS_MASK != 0 {
            return Err(format!("Invalid modifier bits: 0x{modifiers_bits:04x}"));
        }
        let modifiers = Modifiers::from_bits_retain(modifiers_bits);

        let event = InputEvent {
            event_type: if down {
                InputEventType::KeyDown
            } else {
                InputEventType::KeyUp
            },
            x: 0.0,
            y: 0.0,
            key_code,
            modifiers,
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };

        let inner = self
            .inner
            .lock()
            .map_err(|_| "State mutex is poisoned".to_string())?;

        if let Some(ref session) = inner.session {
            session
                .send_input(event)
                .map_err(|e| format!("Failed to send key input: {e}"))?;
        }
        Ok(())
    }

    pub async fn disconnect_async(&self) -> Result<(), String> {
        self.cancel_active_connect();
        let _lifecycle_guard = self.lifecycle_lock.lock().await;
        let state_clone = self.clone();
        tokio::task::spawn_blocking(move || state_clone.disconnect_blocking())
            .await
            .map_err(|e| format!("Disconnect task failed: {e}"))?
    }

    pub fn disconnect_blocking(&self) -> Result<(), String> {
        self.cancel_pending.store(false, Ordering::SeqCst);
        let current_gen = {
            let inner = self
                .inner
                .lock()
                .map_err(|_| "State mutex is poisoned".to_string())?;
            inner.generation
        };

        self.handle_terminal_shutdown(current_gen, ConnectionState::Idle, None)?;

        tracing::info!("iOS session disconnect completed cleanly");
        Ok(())
    }

    pub async fn connect_async(
        &self,
        host: String,
        tcp_port: Option<u16>,
        udp_port: Option<u16>,
        pin: Option<String>,
        pairing_id: Option<String>,
    ) -> Result<SessionStats, IpcError> {
        if self.cancel_pending.swap(false, Ordering::SeqCst) {
            return Err(IpcError::new(
                IpcErrorCode::Cancelled,
                IpcErrorStage::Connect,
                "Connection cancelled before registration",
            ));
        }
        let _lifecycle_guard = self.lifecycle_lock.lock().await;
        let state_clone = self.clone();
        tokio::task::spawn_blocking(move || {
            state_clone.connect_blocking(host, tcp_port, udp_port, pin, pairing_id)
        })
        .await
        .map_err(|e| {
            IpcError::connection_failed(
                IpcErrorStage::Runtime,
                format!("Connect task panicked: {e}"),
            )
        })?
    }

    fn connect_blocking(
        &self,
        host: String,
        tcp_port: Option<u16>,
        udp_port: Option<u16>,
        pin: Option<String>,
        pairing_id: Option<String>,
    ) -> Result<SessionStats, IpcError> {
        if self.cancel_pending.swap(false, Ordering::SeqCst) {
            return Err(IpcError::new(
                IpcErrorCode::Cancelled,
                IpcErrorStage::Connect,
                "Connection cancelled before registration",
            ));
        }

        self.disconnect_blocking()
            .map_err(|e| IpcError::new(IpcErrorCode::CleanupFailed, IpcErrorStage::Cleanup, e))?;

        if self.cancel_pending.swap(false, Ordering::SeqCst) {
            return Err(IpcError::new(
                IpcErrorCode::Cancelled,
                IpcErrorStage::Connect,
                "Connection cancelled before registration",
            ));
        }

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let connect_session_holder = Arc::new(Mutex::new(None));

        let current_generation = {
            let mut inner = self.inner.lock().map_err(|_| {
                IpcError::connection_failed(IpcErrorStage::Client, "State mutex is poisoned")
            })?;
            inner.generation += 1;
            inner.state = ConnectionState::Connecting;
            inner.host = Some(host.clone());
            inner.frames_received.store(0, Ordering::Relaxed);
            inner.frames_decoded.store(0, Ordering::Relaxed);
            inner.audio_packets_received.store(0, Ordering::Relaxed);
            inner.audio_samples_played.store(0, Ordering::Relaxed);
            inner.video_width.store(0, Ordering::Relaxed);
            inner.video_height.store(0, Ordering::Relaxed);
            inner.frame_sequence.store(0, Ordering::Relaxed);
            inner.last_polled_sequence.store(0, Ordering::Relaxed);
            inner.first_frame_presented.store(false, Ordering::Relaxed);
            *inner.latest_frame.lock().unwrap() = None;
            *inner.last_error.lock().unwrap() = None;
            inner.stop_flag = Arc::new(AtomicBool::new(false));
            inner.generation
        };

        *self.active_connect.lock().unwrap() = Some(ActiveConnectState {
            generation: current_generation,
            cancel_flag: cancel_flag.clone(),
            session: connect_session_holder.clone(),
        });

        struct ConnectScopeGuard {
            active_connect: Arc<Mutex<Option<ActiveConnectState>>>,
            generation: u64,
        }
        impl Drop for ConnectScopeGuard {
            fn drop(&mut self) {
                if let Ok(mut guard) = self.active_connect.lock() {
                    if let Some(ref active) = *guard {
                        if active.generation == self.generation {
                            *guard = None;
                        }
                    }
                }
            }
        }
        let _connect_scope_guard = ConnectScopeGuard {
            active_connect: self.active_connect.clone(),
            generation: current_generation,
        };

        let is_cancelled =
            || cancel_flag.load(Ordering::SeqCst) || self.cancel_pending.load(Ordering::SeqCst);

        if is_cancelled() {
            return Err(IpcError::new(
                IpcErrorCode::Cancelled,
                IpcErrorStage::Connect,
                "Connection cancelled",
            ));
        }

        let trimmed_pin = pin.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let trimmed_id = pairing_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        if trimmed_pin.is_none() && trimmed_id.is_none() {
            let err = IpcError::pairing_required("PIN required for initial authorization");
            self.set_error(current_generation, err.message.clone());
            return Err(err);
        }

        let custom_store = {
            let inner = self.inner.lock().map_err(|_| {
                IpcError::connection_failed(IpcErrorStage::Client, "State mutex is poisoned")
            })?;
            inner.pairing_store.clone()
        };

        let stored_record: Option<maho_app::PairingRecord> = if trimmed_pin.is_none() {
            let id = trimmed_id.unwrap();
            let store = match custom_store.as_ref() {
                Some(s) => s.clone(),
                None => PairingStore::open_default().map_err(|e| {
                    let err = IpcError::connection_failed(
                        IpcErrorStage::Client,
                        format!("Failed to open pairing store: {e}"),
                    );
                    self.set_error(current_generation, err.message.clone());
                    err
                })?,
            };
            match store.load(id) {
                Ok(Some(record)) => Some(record),
                Ok(None) => {
                    let err = IpcError::pairing_required(format!(
                        "Unknown pairing ID '{id}'; PIN required"
                    ));
                    self.set_error(current_generation, err.message.clone());
                    return Err(err);
                }
                Err(e) => {
                    let err = IpcError::connection_failed(
                        IpcErrorStage::Client,
                        format!("Keychain error: {e}"),
                    );
                    self.set_error(current_generation, err.message.clone());
                    return Err(err);
                }
            }
        } else {
            None
        };

        let mut config = match build_session_config(&host, tcp_port, udp_port, "MahoRD iOS") {
            Ok(c) => c,
            Err(e) => {
                let err = IpcError::connection_failed(
                    IpcErrorStage::Client,
                    format!("Configuration error: {e}"),
                );
                self.set_error(current_generation, err.message.clone());
                return Err(err);
            }
        };
        if let Some(ref store) = custom_store {
            config.pairing_store_path = Some(store.path().to_path_buf());
        }

        tracing::info!(
            host = %config.host,
            tcp_port = config.tcp_port,
            udp_port = config.udp_port,
            "Connecting to host endpoint"
        );

        if is_cancelled() {
            return Err(IpcError::new(
                IpcErrorCode::Cancelled,
                IpcErrorStage::Connect,
                "Connection cancelled",
            ));
        }

        let session = match ClientSession::new(config.clone()) {
            Ok(s) => s,
            Err(e) => {
                let err = classify_session_error(&e);
                self.set_error(current_generation, err.message.clone());
                return Err(err);
            }
        };

        *connect_session_holder.lock().unwrap() = Some(session.clone());

        if is_cancelled() {
            let _ = session.disconnect();
            return Err(IpcError::new(
                IpcErrorCode::Cancelled,
                IpcErrorStage::Connect,
                "Connection cancelled",
            ));
        }

        let ready_session: ReadySession = if let Some(p) = trimmed_pin {
            match session.pair_with_pin(p) {
                Ok(ready) => {
                    tracing::info!(host = %host, "Authenticated Ready session established via PIN");
                    ready
                }
                Err(e) => {
                    if is_cancelled() {
                        let _ = session.disconnect();
                        return Err(IpcError::new(
                            IpcErrorCode::Cancelled,
                            IpcErrorStage::Connect,
                            "Connection cancelled",
                        ));
                    }
                    let err = classify_session_error(&e);
                    self.set_error(current_generation, err.message.clone());
                    return Err(err);
                }
            }
        } else if let Some(record) = stored_record {
            match session.connect_with_pairing(record) {
                Ok(ready) => {
                    tracing::info!(host = %host, "Authenticated Ready session established via stored pairing");
                    ready
                }
                Err(e) => {
                    if is_cancelled() {
                        let _ = session.disconnect();
                        return Err(IpcError::new(
                            IpcErrorCode::Cancelled,
                            IpcErrorStage::Connect,
                            "Connection cancelled",
                        ));
                    }
                    let err = classify_session_error(&e);
                    self.set_error(current_generation, err.message.clone());
                    return Err(err);
                }
            }
        } else {
            let err = IpcError::pairing_required("PIN required for initial authorization");
            self.set_error(current_generation, err.message.clone());
            return Err(err);
        };

        if is_cancelled() {
            let _ = session.disconnect();
            return Err(IpcError::new(
                IpcErrorCode::Cancelled,
                IpcErrorStage::Connect,
                "Connection cancelled",
            ));
        }

        // Persist verified endpoint metadata through shared API:
        let endpoint = PairingEndpoint::new(config.host.clone(), config.tcp_port, config.udp_port);
        let store = match custom_store.as_ref() {
            Some(s) => s.clone(),
            None => match PairingStore::open_default() {
                Ok(s) => s,
                Err(e) => {
                    let _ = session.disconnect();
                    let err = IpcError::connection_failed(
                        IpcErrorStage::Client,
                        format!("Failed to open pairing store: {e}"),
                    );
                    self.set_error(current_generation, err.message.clone());
                    self.disconnect_blocking().ok();
                    return Err(err);
                }
            },
        };
        match store.remember_endpoint(
            &ready_session.pairing.id,
            &ready_session.pairing.key,
            endpoint,
        ) {
            Ok(persisted) => {
                if !persisted {
                    tracing::warn!(id = %ready_session.pairing.id, "remember_endpoint returned false (key mismatch or deleted record)");
                }
            }
            Err(e) => {
                let _ = session.disconnect();
                let err = IpcError::connection_failed(
                    IpcErrorStage::Client,
                    format!("Failed to persist endpoint metadata: {e}"),
                );
                self.set_error(current_generation, err.message.clone());
                self.disconnect_blocking().ok();
                return Err(err);
            }
        }

        let (audio_init_tx, audio_init_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let (audio_events_tx, audio_events_rx) = mpsc::sync_channel::<AudioOutputEvent>(128);
        let audio_queue = {
            let inner = self.inner.lock().map_err(|_| {
                IpcError::connection_failed(IpcErrorStage::Client, "State mutex is poisoned")
            })?;
            inner.audio_queue.clone()
        };

        let tcp_runtime = match session.spawn_tcp_runtime() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = session.disconnect();
                let err = classify_session_error(&e);
                self.set_error(current_generation, err.message.clone());
                return Err(err);
            }
        };

        if is_cancelled() {
            drop(tcp_runtime);
            let _ = session.disconnect();
            return Err(IpcError::new(
                IpcErrorCode::Cancelled,
                IpcErrorStage::Connect,
                "Connection cancelled",
            ));
        }

        let (stop_flag, worker_completions) = {
            let mut inner = self.inner.lock().map_err(|_| {
                IpcError::connection_failed(IpcErrorStage::Client, "State mutex is poisoned")
            })?;
            if inner.generation != current_generation || is_cancelled() {
                drop(tcp_runtime);
                let _ = session.disconnect();
                return Err(IpcError::new(
                    IpcErrorCode::Cancelled,
                    IpcErrorStage::Connect,
                    "Connection cancelled by newer session",
                ));
            }
            inner.state = ConnectionState::Ready;
            inner.session = Some(session.clone());
            inner.tcp_runtime = Some(tcp_runtime);
            (inner.stop_flag.clone(), inner.worker_completions.clone())
        };

        // 1. Supervisor worker consuming RuntimeEvents:
        let state_supervisor = self.clone();
        let stop_supervisor = stop_flag.clone();
        let event_rx = {
            let inner = self.inner.lock().unwrap();
            inner.tcp_runtime.as_ref().unwrap().events().clone()
        };
        let supervisor_completions = worker_completions.clone();
        let supervisor_worker = thread::Builder::new()
            .name("maho-ios-supervisor".to_string())
            .spawn(move || {
                while !stop_supervisor.load(Ordering::Relaxed) {
                    match event_rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(Ok(SessionEvent::Clipboard(_))) => {}
                        Ok(Ok(SessionEvent::Ping)) => {}
                        Ok(Ok(SessionEvent::Ignored)) => {}
                        Ok(Ok(_)) => {}
                        Ok(Err(err)) => {
                            if stop_supervisor.load(Ordering::Relaxed) {
                                break;
                            }
                            tracing::warn!(%err, "TCP runtime terminal error event");
                            let ipc_err = classify_session_error(&err);
                            let reason = if ipc_err.code == IpcErrorCode::RemoteClosed {
                                "remote-closed".to_string()
                            } else {
                                ipc_err.message
                            };
                            let _ = state_supervisor.handle_terminal_shutdown(
                                current_generation,
                                ConnectionState::Disconnected,
                                Some(reason),
                            );
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            if stop_supervisor.load(Ordering::Relaxed) {
                                break;
                            }
                            tracing::info!("TCP runtime event channel closed; host disconnected");
                            let _ = state_supervisor.handle_terminal_shutdown(
                                current_generation,
                                ConnectionState::Disconnected,
                                Some("remote-closed".to_string()),
                            );
                            break;
                        }
                    }
                }
                notify_worker_completion(
                    &supervisor_completions,
                    WorkerCompletion {
                        generation: current_generation,
                        kind: WorkerKind::Supervisor,
                    },
                );
            })
            .map_err(|e| {
                let err_msg = format!("Failed to spawn supervisor worker: {e}");
                // The TCP runtime and session are already stored in `inner`; tear them down so the
                // failed connect does not leak the runtime or leave the session stuck in Ready.
                let _ = self.handle_terminal_shutdown(
                    current_generation,
                    ConnectionState::Error,
                    Some(err_msg.clone()),
                );
                IpcError::connection_failed(IpcErrorStage::Runtime, err_msg)
            })?;

        {
            let mut inner = self.inner.lock().map_err(|_| {
                IpcError::connection_failed(IpcErrorStage::Client, "State mutex is poisoned")
            })?;
            inner.worker_handles.push(supervisor_worker);
            if let Ok(mut sender_guard) = inner.audio_events_sender.lock() {
                *sender_guard = Some(audio_events_tx.clone());
            };
        }

        // 2. Audio worker:
        let headless_audio = {
            let inner = self.inner.lock().unwrap();
            inner.headless_audio
        };
        let state_audio = self.clone();
        let stop_audio = stop_flag.clone();
        let audio_completions = worker_completions.clone();
        let audio_worker = thread::Builder::new()
            .name("maho-ios-audio".to_string())
            .spawn(move || {
                let headless_tx = if headless_audio {
                    Some(audio_events_tx.clone())
                } else {
                    None
                };

                let mut audio_output = None;
                if !headless_audio {
                    if let Err(e) = maho_render::activate_ios_audio_session() {
                        let err_msg = format!("Failed to activate iOS audio session: {e}");
                        tracing::error!(%err_msg);
                        let _ = audio_init_tx.send(Err(err_msg));
                        notify_worker_completion(
                            &audio_completions,
                            WorkerCompletion {
                                generation: current_generation,
                                kind: WorkerKind::Audio,
                            },
                        );
                        return;
                    }

                    match CpalAudioOutput::start_with_events(audio_queue, None, audio_events_tx) {
                        Ok(out) => {
                            let _ = audio_init_tx.send(Ok(()));
                            audio_output = Some(out);
                        }
                        Err(e) => {
                            let err_msg = format!("Failed to start CPAL audio output: {e}");
                            tracing::error!(%err_msg);
                            let _ = audio_init_tx.send(Err(err_msg));
                            notify_worker_completion(
                                &audio_completions,
                                WorkerCompletion {
                                    generation: current_generation,
                                    kind: WorkerKind::Audio,
                                },
                            );
                            return;
                        }
                    }
                } else {
                    // Explicit headless audio injection mode:
                    let _ = audio_init_tx.send(Ok(()));
                }

                while !stop_audio.load(Ordering::Relaxed) {
                    match audio_events_rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(AudioOutputEvent::Callback {
                            consumed_samples: _,
                            total_consumed_samples,
                        }) => {
                            if let Ok(inner) = state_audio.inner.lock() {
                                if inner.generation == current_generation {
                                    inner
                                        .audio_samples_played
                                        .store(total_consumed_samples, Ordering::Relaxed);
                                }
                            }
                        }
                        Ok(AudioOutputEvent::Error(err)) => {
                            tracing::error!(%err, "Audio output error event");
                            let _ = state_audio.handle_terminal_shutdown(
                                current_generation,
                                ConnectionState::Error,
                                Some(format!("Audio error: {err}")),
                            );
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }

                drop(headless_tx);
                drop(audio_output);
                notify_worker_completion(
                    &audio_completions,
                    WorkerCompletion {
                        generation: current_generation,
                        kind: WorkerKind::Audio,
                    },
                );
            })
            .map_err(|e| {
                let _ = self.handle_terminal_shutdown(
                    current_generation,
                    ConnectionState::Error,
                    Some(format!("Failed to spawn audio worker: {e}")),
                );
                IpcError::connection_failed(
                    IpcErrorStage::Runtime,
                    format!("Failed to spawn audio worker: {e}"),
                )
            })?;

        {
            let mut inner = self.inner.lock().map_err(|_| {
                IpcError::connection_failed(IpcErrorStage::Client, "State mutex is poisoned")
            })?;
            inner.worker_handles.push(audio_worker);
        }

        match audio_init_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                tracing::info!("iOS CPAL audio output started");
            }
            Ok(Err(e)) => {
                let _ = self.handle_terminal_shutdown(
                    current_generation,
                    ConnectionState::Error,
                    Some(e.clone()),
                );
                return Err(IpcError::connection_failed(IpcErrorStage::Runtime, e));
            }
            Err(e) => {
                let err_msg = format!("Audio initialization timed out: {e}");
                let _ = self.handle_terminal_shutdown(
                    current_generation,
                    ConnectionState::Error,
                    Some(err_msg.clone()),
                );
                return Err(IpcError::connection_failed(IpcErrorStage::Runtime, err_msg));
            }
        }

        // 3. Media worker:
        let state_media = self.clone();
        let stop_media = stop_flag.clone();
        let session_udp = session.clone();
        let media_completions = worker_completions.clone();
        let media_worker = thread::Builder::new()
            .name("maho-ios-media".to_string())
            .spawn(move || {
                let mut decoder: Option<maho_decode::HevcDecoder> = None;
                let mut frames_received_local = 0u64;
                let mut frames_decoded_local = 0u64;
                let mut audio_packets_local = 0u64;
                let mut consecutive_decode_failures = 0u32;

                while !stop_media.load(Ordering::Relaxed) {
                    match session_udp.receive_udp_event() {
                        Ok(SessionEvent::Frame(assembled_frame)) => {
                            if stop_media.load(Ordering::Relaxed) {
                                break;
                            }
                            frames_received_local += 1;

                            let mut decode_failed = false;
                            if decoder.is_none() {
                                match maho_decode::detect_codec(&assembled_frame.data) {
                                    Ok(kind) => match kind {
                                        maho_decode::CodecKind::Hevc => {
                                            match maho_decode::HevcDecoder::from_keyframe(&assembled_frame.data) {
                                                Ok(dec) => {
                                                    tracing::info!("Decoder initialized from HEVC keyframe");
                                                    decoder = Some(dec);
                                                }
                                                Err(e) => {
                                                    tracing::debug!(%e, "HEVC keyframe init failed");
                                                    decode_failed = true;
                                                }
                                            }
                                        }
                                        maho_decode::CodecKind::H264 => {
                                            match maho_decode::h264_parameter_set_blob(&assembled_frame.data)
                                                .and_then(|ps| maho_decode::HevcDecoder::new_h264(&ps))
                                            {
                                                Ok(dec) => {
                                                    tracing::info!("Decoder initialized from H.264 parameter sets");
                                                    decoder = Some(dec);
                                                }
                                                Err(e) => {
                                                    tracing::debug!(%e, "H264 decoder init failed");
                                                    decode_failed = true;
                                                }
                                            }
                                        }
                                    },
                                    Err(e) => {
                                        tracing::debug!(%e, "Parameter set detection failed");
                                        decode_failed = true;
                                    }
                                }
                            }

                            if let Some(ref mut dec) = decoder {
                                match dec.decode(&assembled_frame.data, assembled_frame.timestamp_ms as i64) {
                                    Ok(nv12_frames) => {
                                        consecutive_decode_failures = 0;
                                        for frame in nv12_frames {
                                            frames_decoded_local += 1;
                                            let (width, height) = (frame.width, frame.height);
                                            let inner = match state_media.inner.lock() {
                                                Ok(guard) => guard,
                                                Err(_) => return,
                                            };
                                            if inner.generation != current_generation || stop_media.load(Ordering::Relaxed) {
                                                return;
                                            }
                                            let seq = inner.frame_sequence.fetch_add(1, Ordering::Relaxed) + 1;
                                            let repacked = repack_nv12_frame(&frame, seq);
                                            inner.video_width.store(width, Ordering::Relaxed);
                                            inner.video_height.store(height, Ordering::Relaxed);
                                            *inner.latest_frame.lock().unwrap() = Some(Arc::new(repacked));
                                            inner.frames_received.store(frames_received_local, Ordering::Relaxed);
                                            inner.frames_decoded.store(frames_decoded_local, Ordering::Relaxed);

                                            if frames_decoded_local == 1 || frames_decoded_local % 60 == 0 {
                                                tracing::info!(frames_decoded = frames_decoded_local, seq, width, height, "Decoded frame progress");
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::debug!(%e, "Frame decode error");
                                        decode_failed = true;
                                    }
                                }
                            }

                            if decode_failed {
                                decoder = None;
                                consecutive_decode_failures += 1;
                                if consecutive_decode_failures >= 15 {
                                    let err_msg = "Video decode failed repeatedly: exceeded keyframe retry threshold".to_string();
                                    tracing::error!(%err_msg);
                                    let _ = state_media.handle_terminal_shutdown(
                                        current_generation,
                                        ConnectionState::Error,
                                        Some(err_msg),
                                    );
                                    break;
                                }
                                let _ = session_udp.send_control(ControlMessage::RequestKeyFrame);
                            }
                        }
                        Ok(SessionEvent::Audio(pcm)) => {
                            if stop_media.load(Ordering::Relaxed) {
                                break;
                            }
                            audio_packets_local += 1;
                            let inner = match state_media.inner.lock() {
                                Ok(guard) => guard,
                                Err(_) => return,
                            };
                            if inner.generation == current_generation {
                                inner.audio_packets_received.store(audio_packets_local, Ordering::Relaxed);
                                let _ = inner.audio_queue.push_pcm_bytes(&pcm);
                            }
                        }
                        Ok(SessionEvent::Ping) | Ok(SessionEvent::Cursor(_)) | Ok(SessionEvent::Ignored) | Ok(SessionEvent::InputAck { .. }) => {}
                        Ok(SessionEvent::Clipboard(_)) | Ok(SessionEvent::StreamConfig(_)) => {}
                        Err(err) => {
                            if stop_media.load(Ordering::Relaxed) || matches!(err, SessionError::NotReady) {
                                break;
                            }
                            if let SessionError::Io(ref io_err) = err {
                                if matches!(io_err.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) {
                                    thread::yield_now();
                                    continue;
                                }
                            }
                            let err_msg = format!("UDP media receiver stopped: {err}");
                            tracing::error!(%err_msg);
                            let _ = state_media.handle_terminal_shutdown(
                                current_generation,
                                ConnectionState::Error,
                                Some(err_msg),
                            );
                            break;
                        }
                    }
                }

                notify_worker_completion(
                    &media_completions,
                    WorkerCompletion {
                        generation: current_generation,
                        kind: WorkerKind::Media,
                    },
                );
            })
            .map_err(|e| {
                let _ = self.handle_terminal_shutdown(current_generation, ConnectionState::Error, Some(format!("Failed to spawn media receiver: {e}")));
                IpcError::connection_failed(IpcErrorStage::Runtime, format!("Failed to spawn media receiver: {e}"))
            })?;

        {
            let mut inner = self.inner.lock().map_err(|_| {
                IpcError::connection_failed(IpcErrorStage::Client, "State mutex is poisoned")
            })?;
            inner.worker_handles.push(media_worker);
        }

        let (current_state, current_last_err, server_w, server_h) = {
            let inner = self.inner.lock().map_err(|_| {
                IpcError::connection_failed(IpcErrorStage::Client, "State mutex is poisoned")
            })?;
            (
                inner.state,
                inner.last_error.lock().ok().and_then(|g| g.clone()),
                ready_session.server.width as u32,
                ready_session.server.height as u32,
            )
        };

        if current_state != ConnectionState::Ready {
            let msg = current_last_err
                .unwrap_or_else(|| "Connection terminated during startup".to_string());
            return Err(IpcError::connection_failed(IpcErrorStage::Runtime, msg));
        }

        Ok(SessionStats {
            state: "ready".to_string(),
            host: Some(host),
            frames_received: 0,
            frames_decoded: 0,
            audio_packets_received: 0,
            audio_samples_played: 0,
            width: server_w,
            height: server_h,
            last_error: None,
        })
    }

    pub fn presented(&self, sequence: u64) -> Result<(), String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "State mutex is poisoned".to_string())?;
        let latest_seq = inner.frame_sequence.load(Ordering::Relaxed);
        if sequence == 0 || sequence > latest_seq {
            return Err(format!(
                "Presented sequence {sequence} exceeds latest decoded sequence {latest_seq}"
            ));
        }
        let was_first = !inner.first_frame_presented.swap(true, Ordering::SeqCst);
        if was_first {
            tracing::info!(
                sequence,
                "First presented frame verified against decoded sequence"
            );
        } else {
            tracing::debug!(sequence, "Frame presented report confirmed");
        }
        Ok(())
    }

    fn set_error(&self, generation: u64, error_msg: String) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner.generation == generation {
                inner.state = ConnectionState::Error;
                *inner.last_error.lock().unwrap() = Some(error_msg);
            }
        }
    }
}

pub fn build_session_config(
    host: &str,
    tcp_port: Option<u16>,
    udp_port: Option<u16>,
    client_name: &str,
) -> Result<SessionConfig, String> {
    let mut trimmed = host.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        trimmed = trimmed[1..trimmed.len() - 1].trim();
    }
    if trimmed.is_empty() {
        return Err("Host cannot be empty".to_string());
    }

    if let Some(port) = tcp_port {
        if port == 0 {
            return Err("TCP port cannot be 0".to_string());
        }
    }
    if let Some(port) = udp_port {
        if port == 0 {
            return Err("UDP port cannot be 0".to_string());
        }
    }

    let mut config = SessionConfig::direct(trimmed, client_name);
    if let Some(port) = tcp_port {
        config.tcp_port = port;
    }
    if let Some(port) = udp_port {
        config.udp_port = port;
    }

    Ok(config)
}

pub fn classify_session_error(err: &SessionError) -> IpcError {
    match err {
        SessionError::PairingRejected(reason) => match reason {
            maho_proto::PairingRejectReason::DeniedByHost => IpcError::new(
                IpcErrorCode::PairingDenied,
                IpcErrorStage::Preauth,
                "Connection rejected by host",
            ),
            maho_proto::PairingRejectReason::LockedOut => IpcError::new(
                IpcErrorCode::PairingLockedOut,
                IpcErrorStage::Preauth,
                "Host locked out pairing due to excessive attempts",
            ),
            maho_proto::PairingRejectReason::PairingDisabled => IpcError::new(
                IpcErrorCode::PairingDisabled,
                IpcErrorStage::Preauth,
                "Host pairing window expired or pairing disabled",
            ),
        },
        SessionError::Cancelled => IpcError::new(
            IpcErrorCode::Cancelled,
            IpcErrorStage::Connect,
            "Connection cancelled",
        ),
        SessionError::PairingNotFound(_) => {
            IpcError::pairing_required("No saved pairing credential found")
        }
        SessionError::HandshakeAckTimeout => IpcError::new(
            IpcErrorCode::HandshakeTimeout,
            IpcErrorStage::Handshake,
            "Host did not acknowledge handshake within deadline",
        ),
        SessionError::MissingAuthenticatedRegistration => {
            IpcError::incompatible_peer("Host lacks authenticated UDP registration capability")
        }
        SessionError::Tls(maho_net::tls_psk::TlsPskError::Io(io_err)) => {
            classify_io_error(io_err, IpcErrorStage::TlsPsk)
        }
        SessionError::Tls(tls_err) => IpcError::new(
            IpcErrorCode::ConnectionFailed,
            IpcErrorStage::TlsPsk,
            format!("TLS connection failed: {tls_err}"),
        ),
        SessionError::Io(io_err) => classify_io_error(io_err, IpcErrorStage::Connect),
        SessionError::NoAddress => IpcError::new(
            IpcErrorCode::NetworkUnreachable,
            IpcErrorStage::Connect,
            "Address resolution returned no endpoints",
        ),
        other => IpcError::new(
            IpcErrorCode::ConnectionFailed,
            IpcErrorStage::Connect,
            format!("Connection failed: {other}"),
        ),
    }
}

fn classify_io_error(err: &std::io::Error, default_stage: IpcErrorStage) -> IpcError {
    let err_str = err.to_string();
    if matches!(
        err.kind(),
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::BrokenPipe
    ) || err_str.contains("unexpected EOF")
        || err_str.contains("connection reset")
        || err_str.contains("broken pipe")
    {
        return IpcError::new(IpcErrorCode::RemoteClosed, default_stage, "remote-closed");
    }
    match err.kind() {
        std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::HostUnreachable
        | std::io::ErrorKind::NetworkUnreachable => IpcError::new(
            IpcErrorCode::NetworkUnreachable,
            IpcErrorStage::Connect,
            err_str,
        ),
        std::io::ErrorKind::TimedOut => {
            IpcError::new(IpcErrorCode::HandshakeTimeout, default_stage, err_str)
        }
        _ => IpcError::new(IpcErrorCode::ConnectionFailed, default_stage, err_str),
    }
}
