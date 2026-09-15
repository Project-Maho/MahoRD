use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
use maho_app::ClipboardMonitor;
use maho_app::{
    agent_input::{
        convert_agent_action_to_events, encode_nv12_screenshot, AgentAction, InputStateTracker,
        ScreenInfo, ScreenshotFormat,
    },
    key_event, normalize_pointer, pointer_event, ClientSession, CursorState, InputKey,
    PairingStore, ReadySession, SessionConfig, SessionError, SessionEvent, SessionRuntime,
    SessionState, DEFAULT_TCP_PORT, DEFAULT_UDP_PORT,
};
use maho_decode::HevcDecoder;
#[cfg(target_os = "macos")]
use maho_proto::{ClipboardSyncDirection, ClipboardSyncOrigin, ClipboardSyncUpdate};
use maho_proto::{InputEvent, InputEventType, Modifiers};
use maho_render::{AudioOutputDevice, AudioOutputStatus, AudioQueue, CpalAudioOutput};
use serde::{Deserialize, Serialize};
use tauri::State;

#[cfg(target_os = "macos")]
use maho_app::platform::SystemClipboard;

#[cfg(test)]
mod discovery_tests;
#[cfg(test)]
mod pairing_tests;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostItem {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub os: String,
    /// Tailscale presence only; this does not establish MahoRD service readiness.
    pub online: bool,
    /// Legacy exact-hostname store match, not proof of peer identity.
    pub paired: bool,
    pub last_seen: Option<String>,
    pub tcp_port: Option<u16>,
    pub udp_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectResponse {
    pub pairing_id: String,
    pub host_name: String,
    pub server_name: String,
}

pub use maho_app::{IpcError, IpcErrorCode, IpcErrorStage, PairingEndpoint, PairingSummary};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionStats {
    pub connected: bool,
    pub state: String,
    pub frames_received: u64,
    pub frames_decoded: u64,
    pub audio_packets_received: u64,
    pub latency_p50_ms: Option<f64>,
    pub latency_p99_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFramePayload {
    pub width: u32,
    pub height: u32,
    pub timestamp_ms: i64,
    pub jpeg_base64: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InputPayload {
    pub event_type: String,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub view_width: f32,
    #[serde(default)]
    pub view_height: f32,
    #[serde(default)]
    pub key_code: Option<u16>,
    #[serde(default)]
    pub modifiers: u16,
    #[serde(default)]
    pub scroll_dx: f32,
    #[serde(default)]
    pub scroll_dy: f32,
}

#[derive(Clone, Default)]
pub struct RawNv12Payload {
    pub width: u32,
    pub height: u32,
    pub timestamp_ms: i64,
    pub buffer: Vec<u8>,
}

#[derive(Default)]
pub struct FrameMailbox {
    latest: Option<Arc<RawNv12Payload>>,
    pending_display: bool,
}

impl FrameMailbox {
    fn publish(&mut self, payload: RawNv12Payload) {
        self.latest = Some(Arc::new(payload));
        self.pending_display = true;
    }

    fn take_snapshot(&mut self) -> Option<Arc<RawNv12Payload>> {
        if std::mem::take(&mut self.pending_display) {
            self.latest.clone()
        } else {
            None
        }
    }
}

#[derive(Default)]
pub struct LatencyTracker {
    samples: VecDeque<(Instant, f64)>,
}

impl LatencyTracker {
    pub fn record(&mut self, latency_ms: f64) {
        let now = Instant::now();
        self.samples.push_back((now, latency_ms));
        while self
            .samples
            .front()
            .is_some_and(|(t, _)| now.saturating_duration_since(*t) > Duration::from_secs(10))
        {
            self.samples.pop_front();
        }
    }

    pub fn percentiles(&self) -> (Option<f64>, Option<f64>) {
        if self.samples.is_empty() {
            return (None, None);
        }
        let mut vals: Vec<f64> = self.samples.iter().map(|(_, l)| *l).collect();
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let len = vals.len();
        let p50_idx = ((len as f64) * 0.50).floor() as usize;
        let p99_idx = ((len as f64) * 0.99).floor() as usize;
        let p50 = vals.get(p50_idx.min(len - 1)).copied();
        let p99 = vals.get(p99_idx.min(len - 1)).copied();
        (p50, p99)
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStatus {
    pub running: bool,
    pub ip: String,
    pub port: u16,
    pub pin: String,
    pub auto_approve: bool,
    /// Name of the bootstrap client waiting for an explicit approval, if any.
    pub pending_client: Option<String>,
}

/// The stop command must return to the webview even when the host thread is
/// still winding an active connection down; the caller re-reads the status.
const HOST_STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(3);

pub struct HostRuntime {
    pub running: Arc<AtomicBool>,
    pub pin: Arc<Mutex<String>>,
    pub port: u16,
    pub stop_flag: Arc<AtomicBool>,
    pub thread_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub auto_approve: Arc<AtomicBool>,
    /// A bootstrap request parked for the operator's decision. The host side
    /// bounds its own wait, so a never-answered prompt expires there.
    pub pending_consent: Arc<Mutex<Option<maho_host::ConsentPrompt>>>,
}

impl Default for HostRuntime {
    fn default() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            pin: Arc::new(Mutex::new(maho_host::random_pin())),
            port: maho_host::session::DEFAULT_TCP_PORT,
            stop_flag: Arc::new(AtomicBool::new(false)),
            thread_handle: Mutex::new(None),
            auto_approve: Arc::new(AtomicBool::new(false)),
            pending_consent: Arc::new(Mutex::new(None)),
        }
    }
}

impl Drop for HostRuntime {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.thread_handle.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
        self.running.store(false, Ordering::SeqCst);
    }
}

pub fn get_local_ip() -> String {
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                let ip = addr.ip();
                if !ip.is_unspecified() && !ip.is_loopback() {
                    return ip.to_string();
                }
            }
        }
    }
    "127.0.0.1".to_string()
}

pub struct AppState {
    lifecycle: tokio::sync::Mutex<()>,
    pub session: Arc<Mutex<Option<ClientSession>>>,
    /// Pairing of the live session, kept so input can re-handshake when the
    /// Windows host service swaps its session worker on a desktop switch.
    pub active_pairing_id: Arc<Mutex<Option<String>>>,
    pub tcp_runtime: Mutex<Option<SessionRuntime>>,
    pub stop_media_flag: Arc<AtomicBool>,
    pub media_handle: Mutex<Option<thread::JoinHandle<()>>>,
    #[cfg(target_os = "macos")]
    clipboard_monitor: Arc<Mutex<Option<ClipboardMonitor<SystemClipboard>>>>,
    pub frames_received: Arc<AtomicU64>,
    pub frames_decoded: Arc<AtomicU64>,
    pub audio_packets_received: Arc<AtomicU64>,
    pub latency: Arc<Mutex<LatencyTracker>>,
    pub latest_raw_frame: Arc<Mutex<FrameMailbox>>,
    pub latest_cursor: Arc<Mutex<CursorState>>,
    pub agent_tracker: Arc<Mutex<InputStateTracker>>,
    pub agent_pos: Arc<Mutex<(f32, f32)>>,
    audio_playback: Arc<Mutex<AudioPlayback>>,
    audio_runtime: Mutex<Option<AudioRuntime>>,
    pub discovery: Arc<DesktopDiscoveryState>,
    pub host_runtime: Arc<HostRuntime>,
}

pub struct DesktopDiscoveryState {
    pub lan_browser: Mutex<Option<maho_net::discovery::LanDiscovery>>,
    pub tailscale_cache: Mutex<Option<TailscaleCache>>,
    pub tailscale_refreshing: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct TailscaleCache {
    pub result: Result<Vec<HostItem>, String>,
    pub updated_at: Instant,
}

impl Default for DesktopDiscoveryState {
    fn default() -> Self {
        Self {
            lan_browser: Mutex::new(None),
            tailscale_cache: Mutex::new(None),
            tailscale_refreshing: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Default)]
struct AudioPlayback {
    queue: Option<AudioQueue>,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DesktopAudioDevice {
    id: String,
    name: String,
    supported: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DesktopAudioStatus {
    active: bool,
    volume: f32,
    muted: bool,
    device_id: Option<String>,
    devices: Vec<DesktopAudioDevice>,
    consumed_samples: u64,
    error: Option<String>,
}

enum AudioAction {
    Start,
    Stop,
    Status,
    List,
    Volume(f32),
    Muted(bool),
    Device(Option<String>),
}

// The only substituted boundary in deterministic tests is the physical device.
// PCM decoding, queue ownership, commands and media dispatch remain native.
trait DesktopAudioOutput {
    fn status(&self) -> Result<AudioOutputStatus, String>;
}

impl DesktopAudioOutput for CpalAudioOutput {
    fn status(&self) -> Result<AudioOutputStatus, String> {
        CpalAudioOutput::status(self).map_err(|e| e.to_string())
    }
}

trait DesktopAudioBackend {
    fn devices(&mut self) -> Result<Vec<DesktopAudioDevice>, String>;
    fn open(
        &mut self,
        queue: AudioQueue,
        device: Option<&str>,
    ) -> Result<Box<dyn DesktopAudioOutput>, String>;
}

#[derive(Default)]
struct CpalDesktopBackend {
    // IDs are process-local tokens into retained handles, never device names or
    // indices into a newly enumerated list. Explicit selection never falls back.
    devices: Option<Vec<AudioOutputDevice>>,
}

impl DesktopAudioBackend for CpalDesktopBackend {
    fn devices(&mut self) -> Result<Vec<DesktopAudioDevice>, String> {
        // Re-enumerate on every listing so hot-plugged outputs appear. The
        // retained handles are replaced together with the ids that index them.
        self.devices = Some(CpalAudioOutput::output_devices().map_err(|e| e.to_string())?);
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

    fn open(
        &mut self,
        queue: AudioQueue,
        device: Option<&str>,
    ) -> Result<Box<dyn DesktopAudioOutput>, String> {
        let selected = match device {
            None => None,
            Some(id) => {
                let devices = self.devices()?;
                let index = devices
                    .iter()
                    .position(|d| d.id == id)
                    .ok_or_else(|| format!("Unknown audio output: {id}"))?;
                Some(&self.devices.as_ref().unwrap()[index])
            }
        };
        CpalAudioOutput::start_on_device(queue, selected)
            .map(|output| Box::new(output) as Box<dyn DesktopAudioOutput>)
            .map_err(|e| e.to_string())
    }
}

struct DesktopAudio<B: DesktopAudioBackend> {
    backend: B,
    playback: Arc<Mutex<AudioPlayback>>,
    output: Option<Box<dyn DesktopAudioOutput>>,
    volume: f32,
    muted: bool,
    device_id: Option<String>,
    devices: Vec<DesktopAudioDevice>,
    session_active: bool,
}

impl<B: DesktopAudioBackend> DesktopAudio<B> {
    fn new(backend: B, playback: Arc<Mutex<AudioPlayback>>) -> Self {
        Self {
            backend,
            playback,
            output: None,
            volume: 1.0,
            muted: false,
            device_id: None,
            devices: Vec::new(),
            session_active: false,
        }
    }

    fn stop_output(&mut self) -> Result<(), String> {
        // Detach the producer BEFORE releasing the old callback. A racing media
        // event sees no queue; it cannot repopulate an old/new device backlog.
        let queue = self
            .playback
            .lock()
            .map_err(|e| e.to_string())?
            .queue
            .take();
        self.output.take();
        if let Some(queue) = queue {
            queue.clear().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn start_output(&mut self) -> Result<(), String> {
        self.stop_output()?;
        let queue = AudioQueue::default();
        queue.set_volume(self.volume).map_err(|e| e.to_string())?;
        queue.set_muted(self.muted).map_err(|e| e.to_string())?;
        let output = self
            .backend
            .open(queue.clone(), self.device_id.as_deref())?;
        self.output = Some(output);
        let mut playback = self.playback.lock().map_err(|e| e.to_string())?;
        playback.queue = Some(queue);
        playback.error = None;
        Ok(())
    }

    fn apply(&mut self, action: AudioAction) -> Result<DesktopAudioStatus, String> {
        let result = (|| {
            match action {
                AudioAction::Start => {
                    self.session_active = true;
                    self.start_output()?;
                }
                AudioAction::Stop => {
                    self.session_active = false;
                    self.stop_output()?;
                }
                AudioAction::Status => {}
                AudioAction::List => {
                    self.devices = self.backend.devices()?;
                }
                AudioAction::Volume(volume) => {
                    // Validate even when no session/output is active.
                    let queue = self
                        .playback
                        .lock()
                        .map_err(|e| e.to_string())?
                        .queue
                        .clone()
                        .unwrap_or_default();
                    queue.set_volume(volume).map_err(|e| e.to_string())?;
                    self.volume = volume;
                }
                AudioAction::Muted(muted) => {
                    if let Some(queue) = &self.playback.lock().map_err(|e| e.to_string())?.queue {
                        queue.set_muted(muted).map_err(|e| e.to_string())?;
                    }
                    self.muted = muted;
                }
                AudioAction::Device(device) => {
                    // Even a rejected switch cannot leave the former device playing.
                    self.stop_output()?;
                    if let Some(id) = &device {
                        self.devices = self.backend.devices()?;
                        if !self.devices.iter().any(|d| &d.id == id && d.supported) {
                            return Err(format!("Unknown or unsupported audio output: {id}"));
                        }
                    }
                    self.device_id = device;
                    if self.session_active {
                        self.start_output()?;
                    }
                }
            }
            self.snapshot()
        })();
        if let Err(error) = &result {
            tracing::error!(%error, "Desktop audio command failed");
            self.playback
                .lock()
                .expect("private audio endpoint lock poisoned")
                .error = Some(error.clone());
        }
        result
    }

    fn snapshot(&mut self) -> Result<DesktopAudioStatus, String> {
        let status = self
            .output
            .as_ref()
            .map(|out| out.status())
            .transpose()?
            .unwrap_or_default();
        if let Some(error) = status.last_error {
            self.stop_output()?;
            self.playback.lock().map_err(|e| e.to_string())?.error = Some(error);
        }
        Ok(DesktopAudioStatus {
            active: self.output.is_some(),
            volume: self.volume,
            muted: self.muted,
            device_id: self.device_id.clone(),
            devices: self.devices.clone(),
            consumed_samples: status.consumed_samples,
            error: self
                .playback
                .lock()
                .map_err(|e| e.to_string())?
                .error
                .clone(),
        })
    }
}

impl<B: DesktopAudioBackend> Drop for DesktopAudio<B> {
    fn drop(&mut self) {
        if let Err(error) = self.stop_output() {
            tracing::error!(%error, "Audio worker cleanup failed");
        }
    }
}

type AudioRequest = (
    AudioAction,
    tokio::sync::oneshot::Sender<Result<DesktopAudioStatus, String>>,
);

struct AudioRuntime {
    sender: Option<std::sync::mpsc::Sender<AudioRequest>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl AudioRuntime {
    fn spawn(playback: Arc<Mutex<AudioPlayback>>) -> Result<Self, String> {
        Self::spawn_with_backend(playback, CpalDesktopBackend::default)
    }

    fn spawn_with_backend<B: DesktopAudioBackend + 'static>(
        playback: Arc<Mutex<AudioPlayback>>,
        backend: impl FnOnce() -> B + Send + 'static,
    ) -> Result<Self, String> {
        let (sender, receiver) = std::sync::mpsc::channel::<AudioRequest>();
        let worker = thread::Builder::new()
            .name("maho-audio-output".into())
            .spawn(move || {
                // CPAL Stream is deliberately thread-affine on some platforms.
                // Create, control and drop it on this owner, never on Tokio/UI threads.
                let mut audio = DesktopAudio::new(backend(), playback);
                while let Ok((action, reply)) = receiver.recv() {
                    // A cancelled command receiver does not cancel cleanup ownership.
                    let _ = reply.send(audio.apply(action));
                }
            })
            .map_err(|e| format!("Failed to start audio worker: {e}"))?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }
}

impl Drop for AudioRuntime {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                tracing::error!("Audio worker panicked");
            }
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            lifecycle: tokio::sync::Mutex::new(()),
            session: Arc::new(Mutex::new(None)),
            active_pairing_id: Arc::new(Mutex::new(None)),
            tcp_runtime: Mutex::new(None),
            stop_media_flag: Arc::new(AtomicBool::new(false)),
            media_handle: Mutex::new(None),
            frames_received: Arc::new(AtomicU64::new(0)),
            frames_decoded: Arc::new(AtomicU64::new(0)),
            audio_packets_received: Arc::new(AtomicU64::new(0)),
            #[cfg(target_os = "macos")]
            clipboard_monitor: Arc::new(Mutex::new(None)),
            latency: Arc::new(Mutex::new(LatencyTracker::default())),
            latest_raw_frame: Arc::new(Mutex::new(FrameMailbox::default())),
            latest_cursor: Arc::new(Mutex::new(CursorState::default())),
            agent_tracker: Arc::new(Mutex::new(InputStateTracker::default())),
            agent_pos: Arc::new(Mutex::new((0.5, 0.5))),
            audio_playback: Arc::new(Mutex::new(AudioPlayback::default())),
            audio_runtime: Mutex::new(None),
            discovery: Arc::new(DesktopDiscoveryState::default()),
            host_runtime: Arc::new(HostRuntime::default()),
        }
    }
}

impl AppState {
    async fn start_session_audio(&self) {
        if let Err(error) = self.audio_request(AudioAction::Start).await {
            // An unavailable preferred output must not lock the user out of the
            // remote video and its device-recovery controls. The status command
            // exposes this error; no alternate device is started implicitly.
            tracing::error!(%error, "Session audio unavailable");
            self.audio_playback
                .lock()
                .expect("private audio endpoint lock poisoned")
                .error = Some(error);
        }
    }

    async fn audio_request(&self, action: AudioAction) -> Result<DesktopAudioStatus, String> {
        let (reply, received) = tokio::sync::oneshot::channel();
        {
            let mut runtime = self.audio_runtime.lock().map_err(|e| e.to_string())?;
            if runtime.is_none() {
                *runtime = Some(AudioRuntime::spawn(self.audio_playback.clone())?);
            }
            runtime
                .as_ref()
                .unwrap()
                .sender
                .as_ref()
                .unwrap()
                .send((action, reply))
                .map_err(|e| format!("Audio worker unavailable: {e}"))?;
        }
        received
            .await
            .map_err(|e| format!("Audio worker failed: {e}"))?
    }

    fn worker_stop_flag(&self) -> Arc<AtomicBool> {
        self.stop_media_flag.store(false, Ordering::SeqCst);
        self.stop_media_flag.clone()
    }

    #[cfg(test)]
    fn publish_frame(&self, payload: RawNv12Payload) {
        self.latest_raw_frame.lock().unwrap().publish(payload);
    }

    #[cfg(test)]
    fn take_display_frame(&self) -> Vec<u8> {
        let frame = {
            let mut mailbox = self
                .latest_raw_frame
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            mailbox.take_snapshot()
        };
        frame.map(|p| p.buffer.clone()).unwrap_or_default()
    }

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
        // Session-scoped: the previous host's cursor must not be reported while
        // disconnected, nor packed into the next host's first frame trailer.
        // Teardown calls this after the media worker has joined, so an in-flight
        // cursor event cannot revive the stale value.
        if let Ok(mut cursor) = self.latest_cursor.lock() {
            *cursor = CursorState::default();
        }
    }

    pub fn get_host_status(&self) -> Result<HostStatus, String> {
        let pin = self
            .host_runtime
            .pin
            .lock()
            .map_err(|e| format!("Failed to lock PIN: {e}"))?
            .clone();

        Ok(HostStatus {
            running: self.host_runtime.running.load(Ordering::SeqCst),
            ip: get_local_ip(),
            port: self.host_runtime.port,
            pin,
            auto_approve: self.host_runtime.auto_approve.load(Ordering::SeqCst),
            pending_client: self
                .host_runtime
                .pending_consent
                .lock()
                .map_err(|e| format!("Failed to lock pending consent: {e}"))?
                .as_ref()
                .map(|prompt| prompt.client_name.clone()),
        })
    }

    pub fn start_host(&self) -> Result<HostStatus, String> {
        if self.host_runtime.running.load(Ordering::SeqCst) {
            return self.get_host_status();
        }

        if let Ok(mut guard) = self.host_runtime.thread_handle.lock() {
            if let Some(old_handle) = guard.take() {
                let _ = old_handle.join();
            }
        }

        let pin = {
            let mut guard = self
                .host_runtime
                .pin
                .lock()
                .map_err(|e| format!("Failed to lock PIN: {e}"))?;
            if guard.is_empty() {
                let fresh = maho_host::random_pin();
                *guard = fresh.clone();
                fresh
            } else {
                guard.clone()
            }
        };

        let store = maho_host::PairingStore::host_default()
            .map_err(|e| format!("Failed to open host pairing store: {e}"))?;

        #[cfg(target_os = "macos")]
        let mut config = maho_host::HostConfig::macos_default(Some(pin), store)
            .map_err(|e| format!("Failed to create macOS host config: {e}"))?;

        #[cfg(target_os = "windows")]
        let mut config = maho_host::HostConfig::windows_default(Some(pin), store)
            .map_err(|e| format!("Failed to create Windows host config: {e}"))?;

        #[cfg(target_os = "linux")]
        let mut config = {
            let monitors = maho_host::probe_hyprland_monitors();
            let output = maho_host::resolve_output_target(
                None,
                std::env::var("MAHO_OUTPUT").ok(),
                monitors.as_ref(),
            );
            maho_host::HostConfig::linux_default(Some(pin), store, output)
                .map_err(|e| format!("Failed to create Linux host config: {e}"))?
        };

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        return Err("Host server is not supported on this platform".to_string());

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
        config.consent_sender = Some(consent_tx);
        config.tcp_addr = std::net::SocketAddr::from(([0, 0, 0, 0], self.host_runtime.port));

        let server = maho_host::HostServer::bind(config).map_err(|e| e.to_string())?;

        self.host_runtime.stop_flag.store(false, Ordering::SeqCst);
        self.host_runtime.running.store(true, Ordering::SeqCst);

        let stop_flag = Arc::clone(&self.host_runtime.stop_flag);
        let running_flag = Arc::clone(&self.host_runtime.running);

        let handle = std::thread::Builder::new()
            .name("maho-host-server".into())
            .spawn(move || {
                let _ = server.serve_with_stop(stop_flag);
                running_flag.store(false, Ordering::SeqCst);
            })
            .map_err(|e| {
                self.host_runtime.running.store(false, Ordering::SeqCst);
                format!("Failed to spawn host server thread: {e}")
            })?;

        if let Ok(mut guard) = self.host_runtime.thread_handle.lock() {
            *guard = Some(handle);
        }

        self.get_host_status()
    }

    pub fn set_auto_approve(&self, enabled: bool) -> Result<HostStatus, String> {
        self.host_runtime
            .auto_approve
            .store(enabled, Ordering::SeqCst);
        self.get_host_status()
    }

    pub fn stop_host(&self) -> Result<HostStatus, String> {
        self.host_runtime.stop_flag.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.host_runtime.thread_handle.lock() {
            if let Some(handle) = guard.take() {
                // The host thread may still be winding an active connection down.
                // Wait only briefly, then hand the thread back so the IPC call
                // returns to the webview instead of blocking it indefinitely;
                // the caller re-reads the status to observe the final state.
                let deadline = std::time::Instant::now() + HOST_STOP_JOIN_TIMEOUT;
                while !handle.is_finished() && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(25));
                }
                if handle.is_finished() {
                    let _ = handle.join();
                } else {
                    *guard = Some(handle);
                    return self.get_host_status();
                }
            }
        }
        self.host_runtime.running.store(false, Ordering::SeqCst);
        self.get_host_status()
    }
}

pub fn convert_input_payload(event: &InputPayload) -> Result<InputEvent, String> {
    let event_type = match event.event_type.as_str() {
        "MouseMove" | "mousemove" | "0" => InputEventType::MouseMove,
        "LeftMouseDown" | "leftmousedown" | "1" => InputEventType::LeftMouseDown,
        "LeftMouseUp" | "leftmouseup" | "2" => InputEventType::LeftMouseUp,
        "RightMouseDown" | "rightmousedown" | "3" => InputEventType::RightMouseDown,
        "RightMouseUp" | "rightmouseup" | "4" => InputEventType::RightMouseUp,
        "ScrollWheel" | "scrollwheel" | "5" => InputEventType::ScrollWheel,
        "KeyDown" | "keydown" | "6" => InputEventType::KeyDown,
        "KeyUp" | "keyup" | "7" => InputEventType::KeyUp,
        "FlagsChanged" | "flagschanged" | "8" => InputEventType::FlagsChanged,
        "LeftMouseDragged" | "leftmousedragged" | "9" => InputEventType::LeftMouseDragged,
        "RightMouseDragged" | "rightmousedragged" | "10" => InputEventType::RightMouseDragged,
        "MiddleMouseDown" | "middlemousedown" | "11" => InputEventType::MiddleMouseDown,
        "MiddleMouseUp" | "middlemouseup" | "12" => InputEventType::MiddleMouseUp,
        "RelativeMove" | "relativemove" | "14" => InputEventType::RelativeMove,
        "GamepadAxis" | "gamepadaxis" => InputEventType::GamepadAxis,
        "GamepadButtonDown" | "gamepadbuttondown" => InputEventType::GamepadButtonDown,
        "GamepadButtonUp" | "gamepadbuttonup" => InputEventType::GamepadButtonUp,
        "PenMove" | "penmove" => InputEventType::PenMove,
        "PenDown" | "pendown" => InputEventType::PenDown,
        "PenUp" | "penup" => InputEventType::PenUp,
        other => return Err(format!("Unknown event type: {other}")),
    };

    let modifiers = Modifiers::from_bits_retain(event.modifiers);

    if ![
        event.x,
        event.y,
        event.view_width,
        event.view_height,
        event.scroll_dx,
        event.scroll_dy,
    ]
    .into_iter()
    .all(f32::is_finite)
    {
        return Err("Input coordinates and deltas must be finite".to_string());
    }

    match event_type {
        InputEventType::KeyDown | InputEventType::KeyUp => {
            let key_code = event.key_code.unwrap_or(0);
            key_event(event_type, InputKey::WindowsVirtualKey(key_code), modifiers)
                .ok_or_else(|| format!("Unsupported key code: {key_code}"))
        }
        InputEventType::GamepadAxis
        | InputEventType::GamepadButtonDown
        | InputEventType::GamepadButtonUp => Ok(InputEvent {
            event_type,
            x: event.x,
            y: event.y,
            key_code: event.key_code.unwrap_or(0),
            modifiers,
            scroll_dx: event.scroll_dx,
            scroll_dy: event.scroll_dy,
        }),
        InputEventType::PenMove | InputEventType::PenDown | InputEventType::PenUp => {
            let vw = if event.view_width > 0.0 {
                event.view_width
            } else {
                1280.0
            };
            let vh = if event.view_height > 0.0 {
                event.view_height
            } else {
                800.0
            };
            let (norm_x, norm_y) = normalize_pointer(event.x, event.y, vw, vh)
                .ok_or_else(|| "Invalid pointer coordinates".to_string())?;
            Ok(InputEvent {
                event_type,
                x: norm_x,
                y: norm_y,
                key_code: event.key_code.unwrap_or(0),
                modifiers,
                scroll_dx: event.scroll_dx,
                scroll_dy: event.scroll_dy,
            })
        }
        _ => {
            let vw = if event.view_width > 0.0 {
                event.view_width
            } else {
                1280.0
            };
            let vh = if event.view_height > 0.0 {
                event.view_height
            } else {
                800.0
            };
            pointer_event(
                event_type,
                event.x,
                event.y,
                vw,
                vh,
                modifiers,
                event.scroll_dx,
                event.scroll_dy,
            )
            .ok_or_else(|| "Invalid pointer coordinates".to_string())
        }
    }
}

pub mod commands {
    use super::*;
    #[tauri::command]
    pub async fn list_hosts(state: State<'_, AppState>) -> Result<Vec<HostItem>, String> {
        list_hosts_internal(&state).await
    }

    pub async fn list_hosts_default() -> Result<Vec<HostItem>, String> {
        list_hosts_internal(&AppState::default()).await
    }

    pub async fn list_hosts_internal(state: &AppState) -> Result<Vec<HostItem>, String> {
        let store =
            PairingStore::open_default().map_err(|e| format!("Pairing store error: {e}"))?;
        let records = store
            .load_all()
            .map_err(|e| format!("Load pairings error: {e}"))?;

        let lan_res = {
            let mut browser_guard = state.discovery.lan_browser.lock().unwrap();
            if browser_guard.is_none() {
                match maho_net::discovery::LanDiscovery::new() {
                    Ok(browser) => {
                        *browser_guard = Some(browser);
                    }
                    Err(e) => {
                        tracing::warn!("LAN discovery browser initialization failed: {e}");
                    }
                }
            }
            match browser_guard.as_ref() {
                Some(browser) => browser.snapshot(),
                None => Err(maho_net::discovery::DiscoveryError::Unavailable),
            }
        };

        let cached_ts = {
            let guard = state.discovery.tailscale_cache.lock().unwrap();
            guard.clone()
        };

        if !state
            .discovery
            .tailscale_refreshing
            .swap(true, Ordering::SeqCst)
        {
            let discovery_clone = Arc::clone(&state.discovery);
            let records_clone = records.clone();
            tokio::spawn(async move {
                #[cfg(target_os = "macos")]
                let program = {
                    const CANDIDATES: &[&str] = &[
                        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
                        "/usr/local/bin/tailscale",
                        "/opt/homebrew/bin/tailscale",
                    ];
                    CANDIDATES
                        .iter()
                        .find(|p| std::path::Path::new(p).exists())
                        .copied()
                        .unwrap_or("tailscale")
                };
                #[cfg(not(target_os = "macos"))]
                let program = "tailscale";
                let output = tailscale_status(std::ffi::OsStr::new(program)).await;
                let ts_res = hosts_from_tailscale_output(output, &records_clone);
                if let Ok(mut cache_guard) = discovery_clone.tailscale_cache.lock() {
                    *cache_guard = Some(TailscaleCache {
                        result: ts_res,
                        updated_at: Instant::now(),
                    });
                }
                discovery_clone
                    .tailscale_refreshing
                    .store(false, Ordering::SeqCst);
            });
        }

        let ts_result = match cached_ts {
            Some(cache) => cache.result,
            None => {
                if lan_res.is_ok() {
                    Err("Tailscale background query in progress".into())
                } else {
                    tokio::time::timeout(Duration::from_millis(100), async {
                        while state.discovery.tailscale_refreshing.load(Ordering::Relaxed) {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .ok();
                    state
                        .discovery
                        .tailscale_cache
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(|c| c.result.clone())
                        .unwrap_or_else(|| Err("Tailscale status unavailable".into()))
                }
            }
        };

        merge_discovery_results(lan_res, ts_result, &records)
    }

    pub(super) async fn tailscale_status(
        program: &std::ffi::OsStr,
    ) -> std::io::Result<std::process::Output> {
        // Dropping the timed-out/cancelled output future also kills its child.
        // On macOS, the Tailscale app CLI checks $SHLVL and errors if missing (e.g. launched via GUI/LaunchServices).
        tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new(program)
                .args(["status", "--json"])
                .env("SHLVL", "1")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "Tailscale status timed out")
        })?
    }

    pub(super) fn hosts_from_tailscale_output(
        output: std::io::Result<std::process::Output>,
        records: &[maho_app::PairingRecord],
    ) -> Result<Vec<HostItem>, String> {
        let output = output.map_err(|e| format!("Tailscale status execution failed: {e}"))?;
        if !output.status.success() {
            // Do not expose raw stdout/stderr: daemon diagnostics can contain account data.
            return Err(format!("Tailscale status failed: {}", output.status));
        }
        let status: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("Invalid Tailscale status JSON: {e}"))?;
        let peers = match status.get("Peer") {
            Some(serde_json::Value::Object(peers)) => peers,
            // Tailscale may serialize a nil peer map as null.
            Some(serde_json::Value::Null) => return Ok(Vec::new()),
            _ => return Err("Invalid Tailscale status: Peer must be an object or null".into()),
        };

        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Peer {
            host_name: String,
            #[serde(rename = "OS")]
            os: Option<String>,
            online: bool,
            #[serde(rename = "TailscaleIPs")]
            ips: Vec<String>,
        }

        let mut hosts = Vec::new();
        for value in peers.values() {
            let peer: Peer = serde_json::from_value(value.clone())
                .map_err(|_| "Invalid Tailscale peer fields".to_string())?;
            // Mobile and non-host platforms cannot run maho-host.
            if let Some(ref os) = peer.os {
                let os_lower = os.to_ascii_lowercase();
                if matches!(
                    os_lower.as_str(),
                    "ios" | "android" | "tvos" | "watchos" | "visionos"
                ) {
                    continue;
                }
            }

            if let Some(ip) = peer.ips.first().filter(|ip| !ip.is_empty()) {
                // Legacy display hint only, not stable identity or authentication proof.
                let record = records.iter().rev().find(|r| r.name == peer.host_name);
                hosts.push(HostItem {
                    id: record.map(|r| r.id.clone()).unwrap_or_else(|| ip.clone()),
                    name: peer.host_name,
                    ip: ip.clone(),
                    os: peer.os.unwrap_or_else(|| "unknown".into()),
                    online: peer.online,
                    paired: record.is_some(),
                    last_seen: None,
                    tcp_port: None,
                    udp_port: None,
                });
            }
        }
        Ok(hosts)
    }

    pub fn merge_discovery_results(
        lan: Result<Vec<maho_net::discovery::DiscoveredHost>, maho_net::discovery::DiscoveryError>,
        tailscale: Result<Vec<HostItem>, String>,
        _records: &[maho_app::PairingRecord],
    ) -> Result<Vec<HostItem>, String> {
        match (lan, tailscale) {
            (Err(lan_err), Err(ts_err)) => {
                Err(format!(
                    "All discovery sources failed: LAN discovery error ({lan_err}); Tailscale error ({ts_err})"
                ))
            }
            (Ok(lan_hosts), Err(_)) => {
                let items = lan_hosts
                    .into_iter()
                    .map(|h| HostItem {
                        id: h.id,
                        name: h.name,
                        ip: h.ip,
                        os: h.os,
                        online: true,
                        paired: false,
                        last_seen: None,
                        tcp_port: Some(h.tcp_port),
                        udp_port: Some(h.udp_port),
                    })
                    .collect();
                Ok(items)
            }
            (Err(_), Ok(ts_hosts)) => Ok(ts_hosts),
            (Ok(lan_hosts), Ok(ts_hosts)) => {
                let mut merged: Vec<HostItem> = Vec::new();
                let mut seen_ips = std::collections::HashSet::new();

                for h in lan_hosts {
                    seen_ips.insert(h.ip.clone());
                    merged.push(HostItem {
                        id: h.id,
                        name: h.name,
                        ip: h.ip,
                        os: h.os,
                        online: true,
                        paired: false,
                        last_seen: None,
                        tcp_port: Some(h.tcp_port),
                        udp_port: Some(h.udp_port),
                    });
                }

                for th in ts_hosts {
                    if !seen_ips.contains(&th.ip) {
                        seen_ips.insert(th.ip.clone());
                        merged.push(th);
                    }
                }

                Ok(merged)
            }
        }
    }

    pub fn classify_session_error(err: &maho_app::SessionError) -> IpcError {
        match err {
            maho_app::SessionError::PairingRejected(reason) => match reason {
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
            maho_app::SessionError::PairingNotFound(id) => {
                IpcError::pairing_required(format!("No saved pairing credential found for '{id}'"))
            }
            maho_app::SessionError::HandshakeAckTimeout => IpcError::new(
                IpcErrorCode::HandshakeTimeout,
                IpcErrorStage::Handshake,
                "Host did not acknowledge handshake within deadline",
            ),
            maho_app::SessionError::MissingAuthenticatedRegistration => {
                IpcError::incompatible_peer("Host lacks authenticated UDP registration capability")
            }
            maho_app::SessionError::Tls(maho_net::tls_psk::TlsPskError::Io(io_err)) => {
                classify_io_error(io_err, IpcErrorStage::TlsPsk)
            }
            maho_app::SessionError::Tls(tls_err) => IpcError::new(
                IpcErrorCode::ConnectionFailed,
                IpcErrorStage::TlsPsk,
                format!("TLS connection failed: {tls_err}"),
            ),
            maho_app::SessionError::Io(io_err) => classify_io_error(io_err, IpcErrorStage::Connect),
            maho_app::SessionError::NoAddress => IpcError::new(
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

    pub fn classify_io_error(err: &std::io::Error, default_stage: IpcErrorStage) -> IpcError {
        match err.kind() {
            std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::HostUnreachable
            | std::io::ErrorKind::NetworkUnreachable => IpcError::new(
                IpcErrorCode::NetworkUnreachable,
                IpcErrorStage::Connect,
                err.to_string(),
            ),
            std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::BrokenPipe => {
                IpcError::new(IpcErrorCode::RemoteClosed, default_stage, err.to_string())
            }
            std::io::ErrorKind::TimedOut => IpcError::new(
                IpcErrorCode::HandshakeTimeout,
                default_stage,
                err.to_string(),
            ),
            _ => IpcError::new(
                IpcErrorCode::ConnectionFailed,
                default_stage,
                err.to_string(),
            ),
        }
    }

    #[tauri::command]
    pub async fn connect(
        state: State<'_, AppState>,
        host: String,
        tcp_port: Option<u16>,
        udp_port: Option<u16>,
        pin: Option<String>,
        pairing_id: Option<String>,
    ) -> Result<ConnectResponse, IpcError> {
        let _lifecycle = state.lifecycle.lock().await;
        if let Err(cleanup_err) = disconnect_internal(&state).await {
            return Err(IpcError::new(
                IpcErrorCode::CleanupFailed,
                IpcErrorStage::Cleanup,
                cleanup_err,
            ));
        }
        state.clear_metrics();

        if let Some(ref p) = pin {
            let trimmed_pin = p.trim();
            if !trimmed_pin.is_empty()
                && (trimmed_pin.len() != 8 || !trimmed_pin.chars().all(|c| c.is_ascii_digit()))
            {
                return Err(IpcError::invalid_pin("PIN must be exactly 8 ASCII digits"));
            }
        }
        let trimmed_pin = pin.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let trimmed_id = pairing_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        if trimmed_pin.is_none() && trimmed_id.is_none() {
            return Err(IpcError::pairing_required(
                "PIN required for initial authorization",
            ));
        }

        if trimmed_pin.is_none() {
            if let Some(id) = trimmed_id {
                let store = PairingStore::open_default().map_err(|e| {
                    IpcError::connection_failed(
                        IpcErrorStage::Client,
                        format!("Pairing store error: {e}"),
                    )
                })?;
                match store.load(id) {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        return Err(IpcError::pairing_required(format!(
                            "Unknown pairing ID '{id}'; PIN required"
                        )));
                    }
                    Err(e) => {
                        return Err(IpcError::connection_failed(
                            IpcErrorStage::Client,
                            format!("Pairing store error: {e}"),
                        ));
                    }
                }
            }
        }

        match connect_session(&state, host, tcp_port, udp_port, pin, pairing_id).await {
            Ok(response) => Ok(response),
            Err(error) => {
                let cleanup_result = disconnect_internal(&state).await;
                if let Err(cleanup_err) = cleanup_result {
                    Err(IpcError::new(
                        error.code,
                        IpcErrorStage::Cleanup,
                        format!("{}\nCleanup failed: {}", error.message, cleanup_err),
                    ))
                } else {
                    Err(error)
                }
            }
        }
    }

    pub(crate) fn authenticate_client_session(
        session: &ClientSession,
        pin: Option<&str>,
        store: &PairingStore,
        pairing_id: Option<&str>,
    ) -> Result<ReadySession, IpcError> {
        if let Some(pin_str) = pin.map(str::trim).filter(|s| !s.is_empty()) {
            if pin_str.len() != 8 || !pin_str.chars().all(|c| c.is_ascii_digit()) {
                return Err(IpcError::invalid_pin("PIN must be exactly 8 ASCII digits"));
            }
            return session
                .pair_with_pin(pin_str)
                .map_err(|e| classify_session_error(&e));
        }

        let id = match pairing_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => id,
            None => {
                return Err(IpcError::pairing_required(
                    "No PIN provided and no previous pairing found in store",
                ));
            }
        };

        let record = store
            .load(id)
            .map_err(|e| {
                IpcError::connection_failed(
                    IpcErrorStage::Client,
                    format!("Pairing store error: {e}"),
                )
            })?
            .ok_or_else(|| {
                IpcError::pairing_required(format!("Unknown pairing ID '{id}'; PIN required"))
            })?;

        session
            .connect_with_pairing(record)
            .map_err(|e| classify_session_error(&e))
    }

    async fn connect_session(
        state: &AppState,
        host: String,
        tcp_port: Option<u16>,
        udp_port: Option<u16>,
        pin: Option<String>,
        pairing_id: Option<String>,
    ) -> Result<ConnectResponse, IpcError> {
        let tcp = tcp_port.unwrap_or(DEFAULT_TCP_PORT);
        let udp = udp_port.unwrap_or(DEFAULT_UDP_PORT);

        let mut config = SessionConfig::direct(host.clone(), "MahoRD-Tauri");
        config.tcp_port = tcp;
        config.udp_port = udp;

        let session = ClientSession::new(config.clone()).map_err(|e| classify_session_error(&e))?;
        // Publish cleanup ownership before any fallible connect/start work.
        *state
            .session
            .lock()
            .map_err(|e| IpcError::connection_failed(IpcErrorStage::Client, e.to_string()))? =
            Some(session.clone());

        let session_for_connect = session.clone();
        let pin_clone = pin.clone();
        let pairing_id_clone = pairing_id.clone();

        let ready_session: ReadySession =
            tokio::task::spawn_blocking(move || -> Result<ReadySession, IpcError> {
                let store = PairingStore::open_default().map_err(|e| {
                    IpcError::connection_failed(
                        IpcErrorStage::Client,
                        format!("Pairing store error: {e}"),
                    )
                })?;
                authenticate_client_session(
                    &session_for_connect,
                    pin_clone.as_deref(),
                    &store,
                    pairing_id_clone.as_deref(),
                )
            })
            .await
            .map_err(|e| {
                IpcError::connection_failed(IpcErrorStage::Client, format!("Tokio join error: {e}"))
            })??;

        // Persist verified endpoint metadata through shared API:
        let endpoint = PairingEndpoint::new(host.clone(), tcp, udp);
        let store = match PairingStore::open_default() {
            Ok(s) => s,
            Err(e) => {
                let _ = session.disconnect();
                return Err(IpcError::connection_failed(
                    IpcErrorStage::Client,
                    format!("Failed to open pairing store for endpoint persistence: {e}"),
                ));
            }
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
                return Err(IpcError::connection_failed(
                    IpcErrorStage::Client,
                    format!("Failed to persist endpoint metadata: {e}"),
                ));
            }
        }

        let tcp_runtime = session
            .spawn_tcp_runtime()
            .map_err(|e| classify_session_error(&e))?;
        *state
            .tcp_runtime
            .lock()
            .map_err(|e| IpcError::connection_failed(IpcErrorStage::Client, e.to_string()))? =
            Some(tcp_runtime);
        state.start_session_audio().await;

        // Ask for an immediate keyframe so the canvas paints as soon as the
        // host's first (possibly keepalive) frame arrives.
        let _ = session.send_control(maho_proto::ControlMessage::RequestKeyFrame);

        let stop_flag = state.worker_stop_flag();
        let stop_flag_thread = stop_flag.clone();
        let session_udp = session.clone();
        let frames_rx = state.frames_received.clone();
        let frames_dec = state.frames_decoded.clone();
        let audio_rx = state.audio_packets_received.clone();
        let latency_tracker = state.latency.clone();
        let latest_frame_target = state.latest_raw_frame.clone();
        let latest_cursor_target = state.latest_cursor.clone();
        let audio_playback = state.audio_playback.clone();

        let media_thread = thread::Builder::new()
            .name("maho-media-pipeline".to_string())
            .spawn(move || {
                let mut decoder: Option<HevcDecoder> = None;
                let request_session = session_udp.clone();
                let decode_cursor_target = latest_cursor_target.clone();
                run_media_pipeline(
                    stop_flag_thread,
                    move || {
                        let event = session_udp.receive_udp_event();
                        if matches!(event, Ok(SessionEvent::Frame(_))) {
                            frames_rx.fetch_add(1, Ordering::Relaxed);
                        }
                        event
                    },
                    move |message| request_session.send_control(message),
                    move |frame, frame_recv_instant| {
                        for nv12 in decode_media_frame(&mut decoder, &frame)? {
                            frames_dec.fetch_add(1, Ordering::Relaxed);

                            let latency_ms = frame_recv_instant.elapsed().as_secs_f64() * 1000.0;
                            if let Ok(mut lat) = latency_tracker.lock() {
                                lat.record(latency_ms);
                            }

                            let width = nv12.width as usize;
                            let height = nv12.height as usize;
                            let y_len = width * height;
                            // NV12 carries one chroma row per two luma rows, rounded UP: an odd
                            // height has a final half-height row. Truncating here drops that row
                            // and desynchronizes the frame from both decoder adapters (which
                            // produce div_ceil rows) and the webview parser, which then rejects
                            // the buffer as mis-framed.
                            let uv_len = width * height.div_ceil(2);
                            let total_bytes = 16 + y_len + uv_len + 9;

                            let mut buffer = Vec::with_capacity(total_bytes);
                            buffer.extend_from_slice(&nv12.width.to_le_bytes());
                            buffer.extend_from_slice(&nv12.height.to_le_bytes());
                            buffer.extend_from_slice(&nv12.timestamp_ms.to_le_bytes());

                            if nv12.y_plane.len() >= y_len {
                                buffer.extend_from_slice(&nv12.y_plane[..y_len]);
                            } else {
                                buffer.extend_from_slice(&nv12.y_plane);
                                buffer.resize(16 + y_len, 0);
                            }

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

                            let payload = RawNv12Payload {
                                width: nv12.width,
                                height: nv12.height,
                                timestamp_ms: nv12.timestamp_ms,
                                buffer,
                            };

                            if let Ok(mut frame_target) = latest_frame_target.lock() {
                                frame_target.publish(payload);
                            }
                        }
                        Ok(())
                    },
                    move |event| {
                        dispatch_media_event(
                            event,
                            &audio_rx,
                            &latest_cursor_target,
                            &audio_playback,
                        )
                    },
                );
            })
            .map_err(|e| {
                IpcError::connection_failed(
                    IpcErrorStage::Runtime,
                    format!("Failed to spawn media thread: {e}"),
                )
            })?;

        if let Ok(mut media_lock) = state.media_handle.lock() {
            *media_lock = Some(media_thread);
        }

        start_clipboard_monitor(state, &session);

        if let Ok(mut id) = state.active_pairing_id.lock() {
            *id = Some(ready_session.pairing.id.clone());
        }

        Ok(ConnectResponse {
            pairing_id: ready_session.pairing.id,
            host_name: ready_session.pairing.name,
            server_name: ready_session.server.name,
        })
    }

    #[tauri::command]
    pub fn get_cursor_position(state: State<'_, AppState>) -> Result<CursorState, String> {
        state
            .latest_cursor
            .lock()
            .map(|c| *c)
            .map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub fn set_bitrate(state: State<'_, AppState>, bitrate_mbps: u32) -> Result<(), String> {
        let target = bitrate_mbps.clamp(1, 300) as i32 * 1_000_000;
        let session = state.session.lock().map_err(|e| e.to_string())?;
        let session = session.as_ref().ok_or("Not connected")?;
        session
            .send_control(maho_proto::ControlMessage::BitrateAdjust(
                maho_proto::BitrateAdjust {
                    target_bitrate: target,
                },
            ))
            .map_err(|e| e.to_string())
    }

    /// Watches the local pasteboard while connected and pushes changes to the
    /// host. Best effort: clipboard sync failure never blocks a session.
    #[cfg(target_os = "macos")]
    fn start_clipboard_monitor(state: &AppState, session: &ClientSession) {
        if let Ok(mut slot) = state.clipboard_monitor.lock() {
            if let Some(mut stale) = slot.take() {
                stale.stop();
            }
        }
        let mut monitor = match ClipboardMonitor::new(SystemClipboard) {
            Ok(monitor) => monitor,
            Err(error) => {
                tracing::warn!(%error, "clipboard monitor unavailable");
                return;
            }
        };
        let session = session.clone();
        let start = monitor.start(move |text| {
            let update = ClipboardSyncUpdate {
                request_id: 0,
                direction: ClipboardSyncDirection::ClientToHost,
                origin: ClipboardSyncOrigin::LocalPasteboard,
                text,
            };
            if let Err(error) =
                session.send_control(maho_proto::ControlMessage::ClipboardSyncUpdate(update))
            {
                tracing::debug!(%error, "clipboard push failed");
            }
        });
        if let Err(error) = start {
            tracing::warn!(%error, "clipboard monitor failed to start");
            return;
        }
        if let Ok(mut slot) = state.clipboard_monitor.lock() {
            *slot = Some(monitor);
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn start_clipboard_monitor(_state: &AppState, _session: &ClientSession) {}

    pub(super) fn stop_clipboard_monitor(state: &AppState) {
        #[cfg(target_os = "macos")]
        if let Ok(mut slot) = state.clipboard_monitor.lock() {
            if let Some(mut monitor) = slot.take() {
                monitor.stop();
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = state;
    }

    #[tauri::command]
    pub async fn poll_frame_raw(
        state: State<'_, AppState>,
    ) -> Result<tauri::ipc::Response, String> {
        poll_frame_with_clipboard(&state, |text| {
            #[cfg(target_os = "macos")]
            {
                use maho_app::PlatformClipboard;
                maho_app::platform::SystemClipboard
                    .set_text(text)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = text;
                Err("System clipboard is unavailable on this platform".to_string())
            }
        })
        .await
        .map(tauri::ipc::Response::new)
    }

    pub(super) async fn poll_frame_with_clipboard(
        state: &AppState,
        mut apply_clipboard: impl FnMut(&str) -> Result<(), String> + Send + 'static,
    ) -> Result<Vec<u8>, String> {
        // Keep this session's clipboard work ahead of teardown/reconnect. No
        // old worker may apply clipboard content after a new session starts.
        let _lifecycle = state.lifecycle.lock().await;
        let events = state
            .tcp_runtime
            .lock()
            .map_err(|e| e.to_string())?
            .as_ref()
            .map(|runtime| runtime.events().clone());
        let frames = state.latest_raw_frame.clone();
        #[cfg(target_os = "macos")]
        let clipboard_monitor = state.clipboard_monitor.clone();
        tokio::task::spawn_blocking(move || {
            let mut clipboard = None;
            let mut errors = Vec::new();
            if let Some(events) = events {
                // Three bounded slots plus the empty/closed observation. Bound
                // work even if the producer refills while this command runs.
                for _ in 0..4 {
                    match events.try_recv() {
                        Ok(Ok(SessionEvent::Clipboard(text))) => clipboard = Some(text),
                        Ok(Ok(
                            SessionEvent::Ping
                            | SessionEvent::Ignored
                            | SessionEvent::Frame(_)
                            | SessionEvent::Audio(_)
                            | SessionEvent::Cursor(_)
                            | SessionEvent::StreamConfig(_)
                            | SessionEvent::InputAck { .. },
                        )) => {}
                        Ok(Err(error)) => {
                            errors.push(error.to_string());
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            errors.push("TCP runtime disconnected".to_string());
                            break;
                        }
                    }
                }
            }
            if let Some(text) = clipboard {
                // The session monitor owns echo suppression: when present it
                // applies remote text and records the write so the local
                // pasteboard poller never bounces the text back to the host.
                #[cfg(target_os = "macos")]
                let applied = (|| {
                    let slot = clipboard_monitor.lock().ok()?;
                    let monitor = slot.as_ref()?;
                    monitor.apply_remote(&text).ok()?;
                    Some(())
                })()
                .is_some();
                #[cfg(not(target_os = "macos"))]
                let applied = false;
                if !applied {
                    if let Err(error) = apply_clipboard(&text) {
                        errors.push(error);
                    }
                }
            }
            if !errors.is_empty() {
                return Err(errors.join("\n"));
            }
            let frame = frames.lock().map_err(|e| e.to_string())?.take_snapshot();
            Ok(frame.map(|p| p.buffer.clone()).unwrap_or_default())
        })
        .await
        .map_err(|e| format!("Frame event worker failed: {e}"))?
    }

    #[tauri::command]
    pub async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
        let _lifecycle = state.lifecycle.lock().await;
        disconnect_internal(&state).await
    }

    #[tauri::command]
    pub async fn audio_status(state: State<'_, AppState>) -> Result<DesktopAudioStatus, String> {
        let _lifecycle = state.lifecycle.lock().await;
        state.audio_request(AudioAction::Status).await
    }

    #[tauri::command]
    pub async fn list_audio_devices(
        state: State<'_, AppState>,
    ) -> Result<DesktopAudioStatus, String> {
        let _lifecycle = state.lifecycle.lock().await;
        state.audio_request(AudioAction::List).await
    }

    #[tauri::command]
    pub async fn set_audio_volume(
        state: State<'_, AppState>,
        volume: f32,
    ) -> Result<DesktopAudioStatus, String> {
        let _lifecycle = state.lifecycle.lock().await;
        state.audio_request(AudioAction::Volume(volume)).await
    }

    #[tauri::command]
    pub async fn set_audio_muted(
        state: State<'_, AppState>,
        muted: bool,
    ) -> Result<DesktopAudioStatus, String> {
        let _lifecycle = state.lifecycle.lock().await;
        state.audio_request(AudioAction::Muted(muted)).await
    }

    #[tauri::command]
    pub async fn set_audio_device(
        state: State<'_, AppState>,
        device_id: Option<String>,
    ) -> Result<DesktopAudioStatus, String> {
        let _lifecycle = state.lifecycle.lock().await;
        let sessions = state.session.clone();
        let tracker = state.agent_tracker.clone();
        let position = state.agent_pos.clone();
        let released = tokio::task::spawn_blocking(move || {
            let session = sessions.lock().map_err(|e| e.to_string())?;
            reset_session_inputs(session.as_ref(), &tracker, &position)
        })
        .await
        .map_err(|e| format!("Input reset worker failed: {e}"))
        .and_then(|result| result);
        // A failed input release must not prevent detaching the old audio output.
        let switched = state.audio_request(AudioAction::Device(device_id)).await;
        match (released, switched) {
            (Ok(()), result) => result,
            (Err(error), Ok(_)) => Err(error),
            (Err(error), Err(audio)) => Err(format!("{error}\n{audio}")),
        }
    }

    #[tauri::command]
    pub fn list_pairings() -> Result<Vec<PairingSummary>, IpcError> {
        let store = PairingStore::open_default().map_err(|e| {
            IpcError::connection_failed(IpcErrorStage::Client, format!("Pairing store error: {e}"))
        })?;
        list_pairings_internal(&store)
    }

    pub fn list_pairings_internal(store: &PairingStore) -> Result<Vec<PairingSummary>, IpcError> {
        let records = store.load_all().map_err(|e| {
            IpcError::connection_failed(IpcErrorStage::Client, format!("Load pairings error: {e}"))
        })?;
        Ok(records.into_iter().map(PairingSummary::from).collect())
    }

    #[tauri::command]
    pub fn forget_pairing(id: String) -> Result<(), IpcError> {
        let store = PairingStore::open_default().map_err(|e| {
            IpcError::connection_failed(IpcErrorStage::Client, format!("Pairing store error: {e}"))
        })?;
        store.delete(&id).map_err(|e| {
            IpcError::connection_failed(IpcErrorStage::Client, format!("Delete pairing error: {e}"))
        })
    }

    #[tauri::command]
    pub fn stats(state: State<'_, AppState>) -> Result<SessionStats, String> {
        let (connected, state_str) = state
            .session
            .lock()
            .map_err(|e| e.to_string())?
            .as_ref()
            .map(|s| {
                let st = s.state().unwrap_or(SessionState::Disconnected);
                (st == SessionState::Ready, format!("{st:?}"))
            })
            .unwrap_or((false, "Disconnected".to_string()));

        let (latency_p50_ms, latency_p99_ms) = state
            .latency
            .lock()
            .map(|lat| lat.percentiles())
            .unwrap_or((None, None));

        Ok(SessionStats {
            connected,
            state: state_str,
            frames_received: state.frames_received.load(Ordering::Relaxed),
            frames_decoded: state.frames_decoded.load(Ordering::Relaxed),
            audio_packets_received: state.audio_packets_received.load(Ordering::Relaxed),
            latency_p50_ms,
            latency_p99_ms,
        })
    }

    /// Resolves the session to send on, rejecting the two states where input
    /// must not reach the host: teardown already began, or no session exists.
    /// Serializing on the session lock keeps a cloned session from landing a
    /// late key-down after cleanup released the host's keys.
    pub fn send_input_guarded(
        state: &AppState,
        event: &InputPayload,
    ) -> Result<(ClientSession, maho_proto::InputEvent), String> {
        let sess_opt = state.session.lock().map_err(|e| e.to_string())?;
        if state.stop_media_flag.load(Ordering::SeqCst) {
            return Err("Session disconnecting or inactive".to_string());
        }
        let session = sess_opt
            .as_ref()
            .ok_or_else(|| "Session not initialized".to_string())?;
        let input_event = convert_input_payload(event)?;
        Ok((session.clone(), input_event))
    }

    #[tauri::command]
    pub fn send_input(state: State<'_, AppState>, event: InputPayload) -> Result<(), String> {
        let (session, input_event) = send_input_guarded(&state, &event)?;
        let session = &session;

        match session.send_input(input_event) {
            Ok(()) => Ok(()),
            Err(error) if maho_app::should_reconnect(&error) => {
                // The host service replaces its session worker whenever the
                // console switches desktop, which tears down this control
                // channel. Re-handshake with the stored pairing and retry once
                // so a secure-desktop switch does not freeze the viewer.
                let pairing_id = state
                    .active_pairing_id
                    .lock()
                    .map_err(|e| e.to_string())?
                    .clone()
                    .ok_or_else(|| format!("Failed to send input: {error}"))?;
                if let Ok(mut tcp_guard) = state.tcp_runtime.lock() {
                    if let Some(mut old_runtime) = tcp_guard.take() {
                        let _ = old_runtime.stop();
                    }
                }
                session.reconnect(&pairing_id).map_err(|retry| {
                    format!("Failed to send input: {error}; reconnect: {retry}")
                })?;
                if let Ok(new_runtime) = session.spawn_tcp_runtime() {
                    if let Ok(mut tcp_guard) = state.tcp_runtime.lock() {
                        *tcp_guard = Some(new_runtime);
                    }
                }
                session
                    .send_input(input_event)
                    .map_err(|retry| format!("Failed to send input after reconnect: {retry}"))
            }
            Err(error) => Err(format!("Failed to send input: {error}")),
        }
    }

    #[tauri::command]
    pub fn agent_execute_action(
        state: State<'_, AppState>,
        action: AgentAction,
    ) -> Result<usize, String> {
        let sess_opt = state.session.lock().map_err(|e| e.to_string())?;
        if state.stop_media_flag.load(Ordering::SeqCst) {
            return Err("Session disconnecting or inactive".to_string());
        }
        let session = match sess_opt.as_ref() {
            Some(s) => s,
            None => return Err("Session not active".to_string()),
        };

        let info = screen_info(&state)?;
        let (width, height) = (info.width as f32, info.height as f32);

        let mut tracker = state.agent_tracker.lock().map_err(|e| e.to_string())?;
        let mut pos = state.agent_pos.lock().map_err(|e| e.to_string())?;

        let events = convert_agent_action_to_events(&action, &mut tracker, &mut pos, width, height)
            .map_err(|e| e.to_string())?;

        let count = events.len();
        for event in events {
            session.send_input(event).map_err(|e| e.to_string())?;
        }
        Ok(count)
    }

    #[tauri::command]
    pub fn agent_get_screen_info(state: State<'_, AppState>) -> Result<ScreenInfo, String> {
        screen_info(&state)
    }

    pub(super) fn screen_info(state: &AppState) -> Result<ScreenInfo, String> {
        let lock = state.latest_raw_frame.lock().map_err(|e| e.to_string())?;
        if let Some(frame) = lock.latest.as_ref() {
            Ok(ScreenInfo {
                width: frame.width,
                height: frame.height,
                scale: 1.0,
                logical_width: Some(frame.width),
                logical_height: Some(frame.height),
                monitors: vec![maho_app::agent_input::MonitorInfo {
                    id: 0,
                    name: "Primary Display".to_string(),
                    x: 0,
                    y: 0,
                    width: frame.width,
                    height: frame.height,
                    scale: 1.0,
                    is_primary: true,
                }],
                connected_host: "remote-host".to_string(),
            })
        } else {
            Ok(ScreenInfo {
                width: 1920,
                height: 1080,
                scale: 1.0,
                logical_width: Some(1920),
                logical_height: Some(1080),
                monitors: vec![maho_app::agent_input::MonitorInfo {
                    id: 0,
                    name: "Primary Display".to_string(),
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                    scale: 1.0,
                    is_primary: true,
                }],
                connected_host: "unconnected".to_string(),
            })
        }
    }

    #[tauri::command]
    pub async fn agent_capture_screen(
        state: State<'_, AppState>,
        format: Option<String>,
    ) -> Result<String, String> {
        capture_with_job(&state, move |frame| encode_capture(frame, format)).await
    }

    pub(super) async fn capture_with_job(
        state: &AppState,
        encode: impl FnOnce(Arc<RawNv12Payload>) -> Result<String, String> + Send + 'static,
    ) -> Result<String, String> {
        let snapshot = state
            .latest_raw_frame
            .lock()
            .map_err(|e| e.to_string())?
            .latest
            .clone()
            .ok_or_else(|| "No active frame received yet".to_string())?;
        tokio::task::spawn_blocking(move || encode(snapshot))
            .await
            .map_err(|e| format!("Screenshot worker failed: {e}"))?
    }

    #[cfg(test)]
    pub(super) fn capture_screen(
        state: &AppState,
        format: Option<String>,
    ) -> Result<String, String> {
        let snapshot = state
            .latest_raw_frame
            .lock()
            .map_err(|e| e.to_string())?
            .latest
            .clone();
        if let Some(frame) = snapshot {
            encode_capture(frame, format)
        } else {
            Err("No active frame received yet".to_string())
        }
    }

    fn encode_capture(
        frame: Arc<RawNv12Payload>,
        format: Option<String>,
    ) -> Result<String, String> {
        let fmt = if format.as_deref() == Some("jpeg") {
            ScreenshotFormat::Jpeg
        } else {
            ScreenshotFormat::Png
        };
        encode_nv12_screenshot(frame.width, frame.height, &frame.buffer[16..], fmt)
    }

    #[tauri::command]
    pub fn agent_release_all(state: State<'_, AppState>) -> Result<usize, String> {
        let sess_opt = state.session.lock().map_err(|e| e.to_string())?;
        let session = match sess_opt.as_ref() {
            Some(s) => s,
            None => return Err("Session not active".to_string()),
        };

        let mut tracker = state.agent_tracker.lock().map_err(|e| e.to_string())?;
        let pos = state.agent_pos.lock().map_err(|e| e.to_string())?;
        let events = tracker.release_all(pos.0, pos.1);
        let count = events.len();
        for event in events {
            session.send_input(event).map_err(|e| e.to_string())?;
        }
        Ok(count)
    }

    #[tauri::command]
    pub fn get_host_status(state: State<'_, AppState>) -> Result<HostStatus, String> {
        state.get_host_status()
    }

    #[tauri::command]
    pub fn start_host(state: State<'_, AppState>) -> Result<HostStatus, String> {
        state.start_host()
    }

    #[tauri::command]
    pub fn set_auto_approve(
        state: State<'_, AppState>,
        enabled: bool,
    ) -> Result<HostStatus, String> {
        state.set_auto_approve(enabled)
    }

    #[tauri::command]
    pub fn stop_host(state: State<'_, AppState>) -> Result<HostStatus, String> {
        state.stop_host()
    }
}

pub use commands::{get_host_status, set_auto_approve, start_host, stop_host};

// Shared by the actual connect worker and media integration tests.
fn dispatch_media_event(
    event: SessionEvent,
    audio_rx: &AtomicU64,
    cursor_target: &Mutex<CursorState>,
    audio: &Mutex<AudioPlayback>,
) {
    match event {
        SessionEvent::Audio(bytes) => {
            audio_rx.fetch_add(1, Ordering::Relaxed);
            let mut playback = audio.lock().expect("private audio endpoint lock poisoned");
            if let Some(queue) = &playback.queue {
                // v3 transports little-endian f32 stereo PCM, not a compressed codec.
                if let Err(error) = queue.push_pcm_bytes(&bytes) {
                    tracing::error!(%error, "Desktop PCM packet rejected");
                    playback.error = Some(error.to_string());
                }
            }
        }
        SessionEvent::Cursor(cursor) => {
            if let Ok(mut c) = cursor_target.lock() {
                *c = cursor;
            }
        }
        _ => {}
    }
}

// Reset a failed codec before the shared queue admits another reference chain.
fn decode_media_frame(
    decoder: &mut Option<HevcDecoder>,
    frame: &maho_app::AssembledFrame,
) -> Result<Vec<maho_decode::Nv12Frame>, String> {
    let result = (|| {
        let dec = match decoder {
            Some(dec) => dec,
            None => {
                let (_, dec) = HevcDecoder::from_keyframe_auto(&frame.data)?;
                decoder.insert(dec)
            }
        };
        dec.decode(&frame.data, frame.timestamp_ms as i64)
    })();
    result.map_err(|error: maho_decode::DecodeError| {
        *decoder = None;
        error.to_string()
    })
}

// The connect worker and recovery regressions share this media dispatch seam.
// Decoder/publisher ownership stays in the caller; events are real session events.
fn run_media_pipeline(
    stop: Arc<AtomicBool>,
    mut receive: impl FnMut() -> Result<SessionEvent, SessionError>,
    request: impl Fn(maho_proto::ControlMessage) -> Result<(), SessionError> + Send + Sync + 'static,
    mut decode: impl FnMut(maho_app::AssembledFrame, Instant) -> Result<(), String> + Send + 'static,
    mut other: impl FnMut(SessionEvent),
) {
    use maho_app::frame_queue::FrameQueue;

    // Scope owns exactly one decoder. Closing the queue also happens on unwind,
    // before scope joins, so an idle decoder can never strand its UDP owner.
    struct CloseQueue<'a>(&'a FrameQueue);
    impl Drop for CloseQueue<'_> {
        fn drop(&mut self) {
            if let Err(error) = self.0.stop() {
                tracing::error!(%error, "Failed to close media queue");
            }
        }
    }

    let queue = FrameQueue::new();
    let request_keyframe = || {
        if let Err(error) = request(maho_proto::ControlMessage::RequestKeyFrame) {
            tracing::warn!(%error, "Media recovery keyframe request failed");
        }
    };
    thread::scope(|scope| {
        let close = CloseQueue(&queue);
        let decoder = match thread::Builder::new()
            .name("maho-media-decode".to_string())
            .spawn_scoped(scope, || {
                while let Ok((frame, received)) = queue.recv() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Err(error) = decode(frame, received) {
                        tracing::warn!(%error, "Frame decode failed");
                        match queue.decode_failed() {
                            Ok(true) => request_keyframe(),
                            Ok(false) => {}
                            Err(error) => {
                                tracing::error!(%error, "Decode recovery queue failed");
                                stop.store(true, Ordering::SeqCst);
                                break;
                            }
                        }
                    }
                }
            }) {
            Ok(worker) => worker,
            Err(error) => {
                tracing::error!(%error, "Failed to spawn media decoder");
                stop.store(true, Ordering::SeqCst);
                return;
            }
        };
        while !stop.load(Ordering::Relaxed) {
            match receive() {
                Ok(SessionEvent::Frame(frame)) => {
                    let received = Instant::now();
                    match queue.push((frame, received)) {
                        Ok(true) => request_keyframe(),
                        Ok(false) => {}
                        Err(error) => {
                            tracing::error!(%error, "Media queue closed");
                            break;
                        }
                    }
                }
                Ok(event) => other(event),
                Err(SessionError::Io(e))
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(SessionError::NotReady) => break,
                Err(e) => {
                    tracing::debug!("UDP media pipeline event error: {e}");
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
        drop(close);
        if let Err(panic) = decoder.join() {
            stop.store(true, Ordering::SeqCst);
            // Preserve disconnect's existing media-worker panic reporting.
            std::panic::resume_unwind(panic);
        }
    });
}

async fn disconnect_internal(state: &AppState) -> Result<(), String> {
    disconnect_with_stop(state, SessionRuntime::stop).await
}

fn reset_session_inputs(
    session: Option<&ClientSession>,
    tracker: &Mutex<InputStateTracker>,
    position: &Mutex<(f32, f32)>,
) -> Result<(), String> {
    let mut tracker = tracker.lock().map_err(|e| e.to_string())?;
    tracker.clear();
    *position.lock().map_err(|e| e.to_string())? = (0.5, 0.5);
    if let Some(session) = session {
        if session.state().map_err(|e| e.to_string())? == SessionState::Ready {
            session
                .send_input(InputEvent {
                    event_type: InputEventType::Reset,
                    x: 0.5,
                    y: 0.5,
                    key_code: 0,
                    modifiers: Modifiers::empty(),
                    scroll_dx: 0.0,
                    scroll_dy: 0.0,
                })
                .map_err(|e| format!("Failed to reset remote input: {e}"))?;
        }
    }
    Ok(())
}

async fn disconnect_with_stop(
    state: &AppState,
    stop_tcp: impl FnOnce(&mut SessionRuntime) -> Result<(), SessionError> + Send + 'static,
) -> Result<(), String> {
    state.stop_media_flag.store(true, Ordering::SeqCst);
    commands::stop_clipboard_monitor(state);
    let sessions = state.session.clone();
    let tracker = state.agent_tracker.clone();
    let position = state.agent_pos.clone();
    let tcp = state.tcp_runtime.lock().map_err(|e| e.to_string())?.take();
    let media = state.media_handle.lock().map_err(|e| e.to_string())?.take();
    // Do not create an audio worker for an unused/failed pre-connect teardown.
    let has_audio = state
        .audio_runtime
        .lock()
        .map_err(|e| e.to_string())?
        .is_some();
    let audio_stopped = if has_audio {
        state.audio_request(AudioAction::Stop).await.map(|_| ())
    } else {
        Ok(())
    };
    let result = tokio::task::spawn_blocking(move || {
        let (session, released) = match sessions.lock() {
            Ok(mut session) => {
                let released = reset_session_inputs(session.as_ref(), &tracker, &position);
                (session.take(), released)
            }
            Err(error) => (None, Err(error.to_string())),
        };
        // Disconnect wakes transport reads before either worker is joined.
        let disconnected = session
            .map(|s| s.disconnect())
            .transpose()
            .map_err(|e| e.to_string());
        let stopped = tcp
            .map(|mut tcp| stop_tcp(&mut tcp))
            .transpose()
            .map_err(|e| e.to_string());
        let joined = media
            .map(|handle| handle.join())
            .transpose()
            .map_err(|_| "Media worker panicked".to_string());
        let errors: Vec<_> = [
            released,
            audio_stopped,
            disconnected.map(|_| ()),
            stopped.map(|_| ()),
            joined.map(|_| ()),
        ]
        .into_iter()
        .filter_map(Result::err)
        .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    })
    .await
    .map_err(|e| format!("Disconnect worker failed: {e}"));
    state.clear_metrics();
    result?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AppState::default();
    if let Err(err) = state.start_host() {
        tracing::warn!("Failed to auto-start host daemon: {err}");
    }
    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::list_hosts,
            commands::list_pairings,
            commands::forget_pairing,
            commands::connect,
            commands::disconnect,
            commands::audio_status,
            commands::list_audio_devices,
            commands::set_audio_volume,
            commands::set_audio_muted,
            commands::set_audio_device,
            commands::stats,
            commands::send_input,
            commands::agent_execute_action,
            commands::agent_get_screen_info,
            commands::agent_capture_screen,
            commands::agent_release_all,
            commands::get_cursor_position,
            commands::poll_frame_raw,
            commands::set_bitrate,
            commands::get_host_status,
            commands::start_host,
            commands::stop_host,
            commands::set_auto_approve
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
#[path = "mailbox_tests.rs"]
mod mailbox_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn distinctive_frame() -> RawNv12Payload {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&2u32.to_le_bytes());
        buffer.extend_from_slice(&2u32.to_le_bytes());
        buffer.extend_from_slice(&123i64.to_le_bytes());
        buffer.extend_from_slice(&[40, 80, 120, 160, 90, 200]);
        RawNv12Payload {
            width: 2,
            height: 2,
            timestamp_ms: 123,
            buffer,
        }
    }

    fn decoded_capture(state: &AppState) -> image::RgbImage {
        let encoded = commands::capture_screen(state, None).unwrap();
        // Decode the command's base64 wire value without adding a dependency.
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut bits = 0u32;
        let mut count = 0;
        let mut bytes = Vec::new();
        for c in encoded.bytes().take_while(|c| *c != b'=') {
            bits = (bits << 6) | alphabet.iter().position(|v| *v == c).unwrap() as u32;
            count += 6;
            if count >= 8 {
                count -= 8;
                bytes.push((bits >> count) as u8);
            }
        }
        image::load_from_memory(&bytes).unwrap().to_rgb8()
    }

    #[test]
    fn poll_retains_agent_dimensions_and_capture() {
        let state = AppState::default();
        state.publish_frame(distinctive_frame());
        assert_eq!(state.take_display_frame(), distinctive_frame().buffer);
        assert!(state.take_display_frame().is_empty());
        let info = commands::screen_info(&state).unwrap();
        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(decoded_capture(&state).dimensions(), (2, 2));
    }

    #[test]
    fn screenshot_excludes_ipc_header_exact_pixels() {
        let state = AppState::default();
        state.publish_frame(distinctive_frame());
        assert_eq!(
            decoded_capture(&state).as_raw(),
            &[153, 13, 0, 193, 53, 9, 233, 93, 49, 255, 133, 89,]
        );
    }

    #[test]
    fn media_worker_uses_shared_stop_token() {
        let state = AppState::default();
        let worker_flag = state.worker_stop_flag();
        state.stop_media_flag.store(true, Ordering::SeqCst);
        assert!(
            worker_flag.load(Ordering::SeqCst),
            "worker must observe teardown cancellation"
        );
        assert!(Arc::ptr_eq(&worker_flag, &state.stop_media_flag));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn screenshot_encoding_leaves_executor_and_mailbox_responsive() {
        let state = Arc::new(AppState::default());
        state.publish_frame(distinctive_frame());
        let executor_thread = thread::current().id();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let capture_state = state.clone();
        let capture = tokio::spawn(async move {
            commands::capture_with_job(&capture_state, move |frame| {
                assert_ne!(
                    thread::current().id(),
                    executor_thread,
                    "encoding must leave executor"
                );
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                encode_nv12_screenshot(
                    frame.width,
                    frame.height,
                    &frame.buffer[16..],
                    ScreenshotFormat::Png,
                )
            })
            .await
        });
        let progress = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(5), entered_rx)
                .await
                .unwrap()
                .unwrap();
            state.publish_frame(distinctive_frame());
            release_tx.send(()).unwrap();
        });
        let (capture, progress) = tokio::join!(capture, progress);
        assert!(!capture.unwrap().unwrap().is_empty());
        progress.unwrap();
    }

    #[test]
    fn display_is_latest_wins_and_snapshot_ownership_is_shared() {
        let state = AppState::default();
        state.publish_frame(distinctive_frame());
        let snapshot = state
            .latest_raw_frame
            .lock()
            .unwrap()
            .latest
            .clone()
            .unwrap();
        let retained = state
            .latest_raw_frame
            .lock()
            .unwrap()
            .latest
            .clone()
            .unwrap();
        assert!(Arc::ptr_eq(&snapshot, &retained));
        let mut next = distinctive_frame();
        next.buffer[16] = 99;
        state.publish_frame(next.clone());
        assert_eq!(state.take_display_frame(), next.buffer);
        assert!(state.take_display_frame().is_empty());
        assert_eq!(snapshot.buffer[16], 40);
        state.clear_metrics();
        assert!(commands::capture_screen(&state, None).is_err());
        assert!(state.take_display_frame().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn disconnect_join_leaves_executor_responsive_and_clears_frames() {
        let state = AppState::default();
        state.publish_frame(distinctive_frame());
        // A previous host's cursor must not survive disconnect: seed a stale value.
        *state.latest_cursor.lock().unwrap() = CursorState {
            x: 0.25,
            y: 0.75,
            cursor_type: 2,
        };
        let stop = state.worker_stop_flag();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *state.media_handle.lock().unwrap() = Some(thread::spawn(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(stop.load(Ordering::SeqCst));
        }));
        entered_rx.await.unwrap();
        let disconnect = disconnect_internal(&state);
        tokio::pin!(disconnect);
        // Poll teardown once: it must yield while the worker is still gated.
        std::future::poll_fn(|cx| {
            use std::future::Future;
            assert!(disconnect.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert!(state.stop_media_flag.load(Ordering::SeqCst));
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), disconnect)
            .await
            .unwrap()
            .unwrap();
        assert!(state.media_handle.lock().unwrap().is_none());
        assert!(commands::capture_screen(&state, None).is_err());
        assert_eq!(
            *state.latest_cursor.lock().unwrap(),
            CursorState::default(),
            "stale cursor must reset to the hidden sentinel after teardown"
        );
        let new_stop = state.worker_stop_flag();
        assert!(!new_stop.load(Ordering::SeqCst));
    }

    #[test]
    fn latency_tracker_calculates_percentiles() {
        let mut tracker = LatencyTracker::default();
        assert_eq!(tracker.percentiles(), (None, None));

        for i in 1..=100 {
            tracker.record(i as f64);
        }

        let (p50, p99) = tracker.percentiles();
        assert_eq!(p50, Some(51.0));
        assert_eq!(p99, Some(100.0));

        tracker.clear();
        assert_eq!(tracker.percentiles(), (None, None));
    }

    #[test]
    fn convert_pointer_input_payload() {
        let payload = InputPayload {
            event_type: "MouseMove".to_string(),
            x: 640.0,
            y: 400.0,
            view_width: 1280.0,
            view_height: 800.0,
            key_code: None,
            modifiers: 0,
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        let event = convert_input_payload(&payload).unwrap();
        assert_eq!(event.event_type, InputEventType::MouseMove);
        assert!((event.x - 0.5).abs() < 1e-4);
        assert!((event.y - 0.5).abs() < 1e-4);
    }

    #[test]
    fn convert_keyboard_input_payload() {
        let payload = InputPayload {
            event_type: "KeyDown".to_string(),
            x: 0.0,
            y: 0.0,
            view_width: 0.0,
            view_height: 0.0,
            key_code: Some(0x41),
            modifiers: Modifiers::COMMAND.bits(),
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        let event = convert_input_payload(&payload).unwrap();
        assert_eq!(event.event_type, InputEventType::KeyDown);
        assert_eq!(event.key_code, 0x00);
        assert!(event.modifiers.contains(Modifiers::COMMAND));
    }

    #[tokio::test]
    async fn app_state_lifecycle() {
        let state = AppState::default();
        state.frames_received.store(42, Ordering::Relaxed);
        state.frames_decoded.store(42, Ordering::Relaxed);
        state.clear_metrics();
        assert_eq!(state.frames_received.load(Ordering::Relaxed), 0);
        assert_eq!(state.frames_decoded.load(Ordering::Relaxed), 0);
        assert!(disconnect_internal(&state).await.is_ok());
    }

    #[test]
    fn test_host_status_defaults() {
        let state = AppState::default();
        let status = state.get_host_status().unwrap();
        assert!(!status.running);
        assert_eq!(status.pin.len(), 8);
        assert!(status.pin.bytes().all(|b| b.is_ascii_digit()));
        assert_ne!(status.pin, "12345678");
        assert_eq!(status.port, maho_host::session::DEFAULT_TCP_PORT);
        assert!(!status.auto_approve);
        assert!(!status.ip.is_empty());
    }

    #[test]
    fn test_host_runtime_lifecycle() {
        let state = AppState::default();
        assert!(!state.host_runtime.running.load(Ordering::SeqCst));
        let initial_status = state.get_host_status().unwrap();
        assert!(!initial_status.running);

        let stopped_status = state.stop_host().unwrap();
        assert!(!stopped_status.running);
        assert!(!state.host_runtime.running.load(Ordering::SeqCst));
    }

    #[test]
    fn test_host_start_and_stop() {
        let state = AppState::default();
        match state.start_host() {
            Ok(status) => {
                assert!(status.running);
                assert!(state.host_runtime.running.load(Ordering::SeqCst));

                // Calling start_host again when already running is idempotent
                let status2 = state.start_host().unwrap();
                assert!(status2.running);

                // Stop host
                let stopped = state.stop_host().unwrap();
                assert!(!stopped.running);
                assert!(!state.host_runtime.running.load(Ordering::SeqCst));
            }
            Err(e) => {
                eprintln!("start_host skipped in this runner: {e}");
            }
        }
    }
}

#[cfg(test)]
mod recovery_tests;

#[cfg(test)]
mod desktop_integration_tests;
