use std::fs::{self, OpenOptions};
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use maho_net::{
    BootstrapLockout, DatagramCipher, Direction, PskIdentity, TlsPskError, TlsPskServer,
    BOOTSTRAP_IDENTITY, PAIRING_IDENTITY_PREFIX,
};
use maho_proto::{
    AudioFragmentHeader, BitrateAdjust, Capabilities, ControlMessage, CursorUpdate, FrameHeader,
    Handshake, InputAckMessage, InputEvent, PacketHeader, PacketType, PairingGrant, PairingReject,
    PairingRejectReason, PairingRequest, StreamConfigurationErrorCode, StreamConfigurationReject,
    StreamConfigurationResponse, WireCodec, MAX_AUDIO_FRAGMENT_BYTES, MAX_VIDEO_CHUNK_BYTES,
    PROTOCOL_VERSION,
};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use maho_proto::{ClipboardSyncDirection, ClipboardSyncOrigin, ClipboardSyncUpdate};
use openssl::base64;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};
use uuid::Uuid;

#[cfg(target_os = "linux")]
use crate::capture_linux::{CaptureConfig as LinuxCaptureConfig, LinuxCapture};
#[cfg(target_os = "macos")]
use crate::capture_macos::{CaptureConfig, CaptureEvent, CaptureFrame, ScreenCapture};
#[cfg(target_os = "windows")]
use crate::capture_windows::{CaptureError, WindowsCapture};
#[cfg(target_os = "linux")]
use crate::encode_linux::{
    EncoderConfig as LinuxEncoderConfig, LinuxVideoEncoder, VideoCodec as LinuxVideoCodec,
};
#[cfg(target_os = "macos")]
use crate::encode_vt::{EncoderConfig, VideoToolboxEncoder, DEFAULT_BITRATE};
#[cfg(target_os = "windows")]
use crate::encode_windows::{EncoderConfig, MediaFoundationEncoder, VideoCodec};
#[cfg(target_os = "linux")]
use crate::inject_linux::LinuxInputInjector;
#[cfg(target_os = "macos")]
use crate::inject_macos::InputInjector;
#[cfg(target_os = "windows")]
use crate::inject_windows::WindowsInputInjector;
#[cfg(target_os = "windows")]
use crate::windows_logic::TargetDisplay;

pub const DEFAULT_TCP_PORT: u16 = 19_730;
pub const DEFAULT_UDP_PORT: u16 = 19_731;
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);
pub const PAIRING_WINDOW: Duration = Duration::from_secs(300);
/// The shared codec remains available through both legacy host paths.
///
/// ```
/// let stats = maho_host::TimestampStats {
///     frame_id: 7,
///     capture_us: 10,
///     encode_start_us: 20,
///     encode_end_us: 30,
///     send_us: 40,
/// };
/// let session_stats: maho_host::session::TimestampStats = stats;
/// let shared_stats: maho_proto::TimestampStats = session_stats;
/// let encode: fn(maho_host::TimestampStats) -> Vec<u8> =
///     maho_host::session::TimestampStats::encode;
/// let decode: fn(&[u8]) -> Option<maho_host::TimestampStats> =
///     maho_host::session::TimestampStats::decode;
/// let bytes = encode(shared_stats);
/// assert_eq!(bytes.len(), 42);
/// assert_eq!(maho_host::TimestampStats::SIZE, 42);
/// assert_eq!(&bytes[..6], maho_host::session::TIMESTAMP_STATS_MAGIC);
/// assert_eq!(maho_host::session::TIMESTAMP_STATS_MAGIC, b"ERDTS1");
/// assert_eq!(decode(&bytes), Some(stats));
/// ```
pub use maho_proto::{TimestampStats, TIMESTAMP_STATS_MAGIC};
const SWIFT_REFERENCE_DATE_OFFSET: f64 = 978_307_200.0;
#[path = "host_trace.rs"]
pub(crate) mod host_trace;

#[cfg(any(target_os = "windows", test))]
pub(crate) fn capture_failure_from_label(label: &str) -> crate::windows_session::CaptureFailure {
    use crate::windows_session::CaptureFailure;

    match label {
        "access_lost" => CaptureFailure::AccessLost,
        "access_denied" => CaptureFailure::AccessDenied,
        "refresh_failure" => CaptureFailure::RefreshFailure,
        _ => CaptureFailure::Other,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingRecord {
    pub id: String,
    pub name: String,
    pub key: [u8; 32],
    pub added_at_unix_ms: u64,
}

#[derive(Debug, Clone)]
pub struct PairingStore {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiskPairingRecord {
    id: String,
    name: String,
    key: String,
    #[serde(
        default,
        alias = "addedAt",
        alias = "added_at",
        alias = "added_at_unix_ms",
        alias = "addedAtUnixMs"
    )]
    added_at: f64,
}

impl PairingStore {
    pub fn default_path() -> Result<PathBuf, SessionError> {
        let directory = dirs::data_dir()
            .ok_or_else(|| SessionError::Store("Application Support is unavailable".into()))?
            .join("MahoRD");
        Ok(directory.join("host-authorizations.json"))
    }

    pub fn host_default() -> Result<Self, SessionError> {
        Ok(Self::new(Self::default_path()?))
    }

    pub fn service_default_path(program_data: &str) -> PathBuf {
        crate::windows_session::service_store_path(program_data)
    }

    #[cfg(target_os = "windows")]
    pub fn service_default() -> Self {
        let program_data =
            std::env::var("PROGRAMDATA").unwrap_or_else(|_| r"C:\ProgramData".into());
        Self::new(Self::service_default_path(&program_data))
    }

    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_all(&self) -> Result<Vec<PairingRecord>, SessionError> {
        let _guard = self.lock.lock().expect("pairing store lock poisoned");
        self.load_all_unlocked()
    }

    pub fn load(&self, id: &str) -> Result<Option<PairingRecord>, SessionError> {
        Ok(self.load_all()?.into_iter().find(|record| record.id == id))
    }

    pub fn save(&self, record: PairingRecord) -> Result<(), SessionError> {
        let _guard = self.lock.lock().expect("pairing store lock poisoned");
        let mut records = self.load_all_unlocked()?;
        records.retain(|existing| existing.id != record.id);
        records.push(record);
        self.write_unlocked(&records)
    }

    pub fn revoke(&self, id: &str) -> Result<bool, SessionError> {
        let _guard = self.lock.lock().expect("pairing store lock poisoned");
        let mut records = self.load_all_unlocked()?;
        let original_len = records.len();
        records.retain(|record| record.id != id);
        if records.len() == original_len {
            return Ok(false);
        }
        self.write_unlocked(&records)?;
        Ok(true)
    }

    fn load_all_unlocked(&self) -> Result<Vec<PairingRecord>, SessionError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(SessionError::Io(error)),
        };
        let disk: Vec<DiskPairingRecord> = serde_json::from_slice(&bytes)
            .map_err(|error| SessionError::Store(error.to_string()))?;
        disk.into_iter()
            .map(|record| {
                let key = base64::decode_block(&record.key)
                    .map_err(|error| SessionError::Store(error.to_string()))?;
                let key: [u8; 32] = key
                    .try_into()
                    .map_err(|_| SessionError::Store("pairing key is not 32 bytes".into()))?;
                let unix_seconds = record.added_at + SWIFT_REFERENCE_DATE_OFFSET;
                Ok(PairingRecord {
                    id: record.id,
                    name: record.name,
                    key,
                    added_at_unix_ms: (unix_seconds.max(0.0) * 1_000.0).round() as u64,
                })
            })
            .collect()
    }

    fn write_unlocked(&self, records: &[PairingRecord]) -> Result<(), SessionError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let disk = records
            .iter()
            .map(|record| DiskPairingRecord {
                id: record.id.clone(),
                name: record.name.clone(),
                key: base64::encode_block(&record.key),
                added_at: record.added_at_unix_ms as f64 / 1_000.0 - SWIFT_REFERENCE_DATE_OFFSET,
            })
            .collect::<Vec<_>>();
        let bytes =
            serde_json::to_vec(&disk).map_err(|error| SessionError::Store(error.to_string()))?;
        let temporary = self.path.with_extension("json.tmp");
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        use std::io::Write;
        file.write_all(&bytes)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
        }
        fs::rename(&temporary, &self.path)?;
        #[cfg(unix)]
        {
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

pub fn random_pin() -> String {
    let mut rng = rand::rng();
    let mut bytes = [0_u8; 8];
    rng.fill_bytes(&mut bytes);
    let value = u64::from_le_bytes(bytes) % 100_000_000;
    format!("{value:08}")
}

pub struct ConsentPrompt {
    pub client_name: String,
    response: SyncSender<bool>,
}

impl ConsentPrompt {
    pub fn approve(self) {
        let _ = self.response.send(true);
    }

    pub fn reject(self) {
        let _ = self.response.send(false);
    }

    pub fn respond(self, approved: bool) {
        let _ = self.response.send(approved);
    }
}

#[derive(Clone)]
pub struct HostConfig {
    pub tcp_addr: SocketAddr,
    pub udp_addr: SocketAddr,
    pub bootstrap_pin: Option<String>,
    pub pairing_window: Duration,
    pub pairing_store: PairingStore,
    pub host_name: String,
    pub display: DisplayInfo,
    pub frames_per_second: u32,
    pub bitrate: u32,
    pub capture_audio: bool,
    pub consent_sender: Option<mpsc::Sender<ConsentPrompt>>,
    /// Linux only: which output to capture. `None` picks the first output,
    /// which on Hyprland is frequently a headless output that never commits.
    pub output_name: Option<String>,
}

impl HostConfig {
    #[cfg(target_os = "macos")]
    pub fn macos_default(
        bootstrap_pin: Option<String>,
        pairing_store: PairingStore,
    ) -> Result<Self, SessionError> {
        let display = ScreenCapture::display_info()?;
        Ok(Self {
            tcp_addr: SocketAddr::from(([0, 0, 0, 0], DEFAULT_TCP_PORT)),
            udp_addr: SocketAddr::from(([0, 0, 0, 0], DEFAULT_UDP_PORT)),
            bootstrap_pin,
            pairing_window: PAIRING_WINDOW,
            pairing_store,
            host_name: hostname::get()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            display,
            frames_per_second: 60,
            bitrate: DEFAULT_BITRATE,
            capture_audio: true,
            output_name: None,
            consent_sender: None,
        })
    }

    #[cfg(target_os = "windows")]
    pub fn windows_default(
        bootstrap_pin: Option<String>,
        pairing_store: PairingStore,
    ) -> Result<Self, SessionError> {
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::UI::HiDpi::{
                SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            };
            unsafe {
                let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            }
        }
        let meta = crate::capture_windows::WindowsCapture::primary_output_metadata()
            .map_err(|error| SessionError::Io(io::Error::other(error.to_string())))?;
        Ok(Self {
            tcp_addr: SocketAddr::from(([0, 0, 0, 0], DEFAULT_TCP_PORT)),
            udp_addr: SocketAddr::from(([0, 0, 0, 0], DEFAULT_UDP_PORT)),
            bootstrap_pin,
            pairing_window: PAIRING_WINDOW,
            pairing_store,
            host_name: hostname::get()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            display: DisplayInfo {
                desktop_x: meta.desktop_x,
                desktop_y: meta.desktop_y,
                logical_width: meta.logical_width,
                logical_height: meta.logical_height,
                pixel_width: meta.pixel_width,
                pixel_height: meta.pixel_height,
                scale_factor_milli: meta.scale_factor_milli,
            },
            frames_per_second: 60,
            bitrate: WINDOWS_DEFAULT_BITRATE,
            capture_audio: false,
            output_name: None,
            consent_sender: None,
        })
    }

    #[cfg(target_os = "linux")]
    pub fn linux_default(
        bootstrap_pin: Option<String>,
        pairing_store: PairingStore,
        output_name: Option<String>,
    ) -> Result<Self, SessionError> {
        let config = LinuxCaptureConfig {
            output_name: output_name.clone(),
            ..LinuxCaptureConfig::default()
        };
        let capture = LinuxCapture::connect(config)
            .map_err(|error| SessionError::Io(io::Error::other(error.to_string())))?;
        let output = capture.output_info();
        let display = DisplayInfo {
            desktop_x: 0,
            desktop_y: 0,
            logical_width: output.pixel_width,
            logical_height: output.pixel_height,
            pixel_width: output.pixel_width,
            pixel_height: output.pixel_height,
            scale_factor_milli: (output.scale.max(1) as u32) * 1_000,
        };
        drop(capture);
        Ok(Self {
            tcp_addr: SocketAddr::from(([0, 0, 0, 0], DEFAULT_TCP_PORT)),
            udp_addr: SocketAddr::from(([0, 0, 0, 0], DEFAULT_UDP_PORT)),
            bootstrap_pin,
            pairing_window: PAIRING_WINDOW,
            pairing_store,
            host_name: hostname::get()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            display,
            frames_per_second: 60,
            bitrate: LINUX_DEFAULT_BITRATE,
            capture_audio: false,
            output_name,
            consent_sender: None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    PreAuth,
    PairingGranted,
    Authenticated,
    Closed,
}

impl SessionState {
    pub fn allows(self, packet_type: PacketType) -> bool {
        match self {
            Self::PreAuth => matches!(
                packet_type,
                PacketType::PairingRequest | PacketType::Handshake
            ),
            Self::PairingGranted => matches!(packet_type, PacketType::Handshake),
            Self::Authenticated => {
                matches!(packet_type, PacketType::InputEvent | PacketType::Control)
            }
            Self::Closed => false,
        }
    }
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("TLS-PSK failed: {0}")]
    Tls(#[from] TlsPskError),
    #[error("protocol codec failed: {0}")]
    Codec(#[from] maho_proto::CodecError),
    #[error("UDP cipher failed: {0}")]
    Cipher(#[from] maho_net::DatagramError),
    #[cfg(target_os = "macos")]
    #[error("capture failed: {0}")]
    Capture(#[from] crate::capture_macos::CaptureError),
    #[cfg(target_os = "macos")]
    #[error("encoder failed: {0}")]
    Encode(#[from] crate::encode_vt::EncodeError),
    #[error("pairing store failed: {0}")]
    Store(String),
    #[error("invalid bootstrap PIN")]
    InvalidPin,
    #[error("pairing consent is unavailable")]
    ConsentUnavailable,
    #[error("pairing consent timed out")]
    ConsentTimeout,
    #[error("peer is not authenticated")]
    PreAuth,
    #[error("session is already authenticated")]
    AlreadyAuthenticated,
    #[error("handshake pairing ID does not match the TLS identity")]
    IdentityMismatch,
    #[error("unknown pairing ID")]
    UnknownPairing,
    #[error("session salt is missing")]
    MissingSessionSalt,
    #[error("UDP peer is unavailable")]
    UdpPeerUnavailable,
    #[error("discovery advertisement failed: {0}")]
    Discovery(#[from] maho_net::discovery::DiscoveryError),
    #[error("media pipeline stopped")]
    MediaStopped,
    #[error("peer lacks authenticated UDP registration capability")]
    MissingAuthenticatedRegistration,
}

#[derive(Debug)]
enum MediaEvent {
    Video(VideoFrame),
    /// Windows pipeline has no audio source yet; the variant is kept so the
    /// wire shape stays identical across platforms.
    #[allow(dead_code)]
    Audio(Vec<u8>),
    #[cfg(target_os = "linux")]
    NativeAudio(Arc<native_pipeline::LatestAudio>),
    #[allow(dead_code)]
    Cursor(CursorUpdate),
    Error(String),
}

/// Platform-neutral encoded frame handed from a [`MediaSource`] to the wire.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub data: Vec<u8>,
    pub is_key_frame: bool,
    pub capture_at: Instant,
    pub encode_started_at: Instant,
    pub encode_completed_at: Instant,
}

/// Platform-neutral display geometry shared by every media backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayInfo {
    pub desktop_x: i32,
    pub desktop_y: i32,
    pub logical_width: u32,
    pub logical_height: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub scale_factor_milli: u32,
}

impl DisplayInfo {
    pub fn scale_factor(self) -> f32 {
        self.scale_factor_milli as f32 / 1_000.0
    }
}

#[cfg(target_os = "windows")]
const WINDOWS_DEFAULT_BITRATE: u32 = 8_000_000;

#[cfg(target_os = "linux")]
const LINUX_DEFAULT_BITRATE: u32 = 8_000_000;

/// Selects the focused output name from compositor monitor information,
/// falling back to the first available named monitor if none is marked focused.
pub fn select_focused_output(monitors: &serde_json::Value) -> Option<String> {
    let array = monitors.as_array()?;
    if let Some(name) = array
        .iter()
        .find(|monitor| monitor.get("focused").and_then(serde_json::Value::as_bool) == Some(true))
        .and_then(|monitor| monitor.get("name"))
        .and_then(serde_json::Value::as_str)
    {
        return Some(name.to_owned());
    }
    array
        .iter()
        .find_map(|monitor| monitor.get("name").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
}

/// Resolves the output target based on strict precedence:
/// 1. Explicit CLI argument (`--output <NAME>`) — strict, preserved as-is.
/// 2. Explicit environment variable (`MAHO_OUTPUT=<NAME>`) — strict, preserved as-is.
/// 3. Auto-detection via compositor monitor information:
///    a. If a monitor has `"focused": true`, select it.
///    b. If no monitor is focused, fall back to the first available named monitor.
///    c. If no monitors are available, return `None`.
pub fn resolve_output_target(
    cli_output: Option<String>,
    env_output: Option<String>,
    monitors: Option<&serde_json::Value>,
) -> Option<String> {
    if let Some(name) = cli_output.filter(|s| !s.trim().is_empty()) {
        return Some(name);
    }
    if let Some(name) = env_output.filter(|s| !s.trim().is_empty()) {
        return Some(name);
    }
    let monitors = monitors?;
    select_focused_output(monitors)
}

/// Probes Hyprland monitors using `hyprctl monitors -j`.
#[cfg(target_os = "linux")]
pub fn probe_hyprland_monitors() -> Option<serde_json::Value> {
    let output = std::process::Command::new("hyprctl")
        .arg("monitors")
        .arg("-j")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Name of the compositor's focused output, for hosts that expose several
/// outputs (e.g. Hyprland headless outputs). Best effort: None when the
/// compositor is unknown or the probe fails, which makes the capture fall
/// back to the first output.
#[cfg(target_os = "linux")]
pub fn focused_output_name() -> Option<String> {
    let monitors = probe_hyprland_monitors();
    resolve_output_target(None, std::env::var("MAHO_OUTPUT").ok(), monitors.as_ref())
}

trait MediaHandle {
    fn force_key_frame(&self) -> Result<(), SessionError>;
    fn update_bitrate(&self, bitrate: u32) -> Result<(), SessionError>;
    fn stop(&mut self);
}

trait MediaSource: Send + Sync {
    fn start(&self, sender: SyncSender<MediaEvent>) -> Result<Box<dyn MediaHandle>, SessionError>;
    #[cfg(test)]
    fn sender_exited(&self) {}
}

#[cfg(target_os = "macos")]
struct MacMediaSource {
    config: CaptureConfig,
    encoder: EncoderConfig,
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct MacMediaControls {
    force_key_frame: bool,
    bitrate: Option<u32>,
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct MacMediaState {
    cancelled: std::sync::atomic::AtomicBool,
    controls: Mutex<MacMediaControls>,
}

#[cfg(target_os = "macos")]
impl MacMediaState {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Acquire)
    }

    // Keep the pending compressed packet until sent. Only cancellation or an
    // output disconnect may abandon it; raw capture has its own bounded queue.
    fn publish(&self, sender: &SyncSender<MediaEvent>, mut event: MediaEvent) -> bool {
        while !self.is_cancelled() {
            match sender.try_send(event) {
                Ok(()) => return true,
                Err(mpsc::TrySendError::Full(pending)) => event = pending,
                Err(mpsc::TrySendError::Disconnected(_)) => return false,
            }
            // std mpsc has no cancellable send. Stop/control unpark the owner;
            // the bounded retry also observes receiver progress/disconnection.
            thread::park_timeout(Duration::from_millis(2));
        }
        false
    }

    fn flush_controls(
        &self,
        sender: &SyncSender<crate::encode_vt::Command>,
    ) -> Result<(), SessionError> {
        use crate::encode_vt::Command;
        let mut controls = self
            .controls
            .lock()
            .map_err(|_| SessionError::MediaStopped)?;
        if controls.force_key_frame {
            match sender.try_send(Command::ForceKeyFrame) {
                Ok(()) => controls.force_key_frame = false,
                Err(mpsc::TrySendError::Full(_)) => return Ok(()),
                Err(mpsc::TrySendError::Disconnected(_)) => return Err(SessionError::MediaStopped),
            }
        }
        if let Some(bitrate) = controls.bitrate {
            match sender.try_send(Command::UpdateBitrate(bitrate)) {
                Ok(()) => controls.bitrate = None,
                Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => return Err(SessionError::MediaStopped),
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
struct MacMediaHandle {
    state: Arc<MacMediaState>,
    worker: Option<thread::JoinHandle<()>>,
}

// Created and destroyed on the owner thread, including encoder init failures.
#[cfg(target_os = "macos")]
struct MacCaptureOwner(Option<ScreenCapture>);

#[cfg(target_os = "macos")]
impl Drop for MacCaptureOwner {
    fn drop(&mut self) {
        if let Some(capture) = self.0.take() {
            if let Err(error) = capture.stop() {
                warn!(%error, "macOS capture stop failed");
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl MacMediaHandle {
    fn spawn(
        sender: SyncSender<MediaEvent>,
        run: impl FnOnce(&MacMediaState, &SyncSender<MediaEvent>) -> Result<(), SessionError>
            + Send
            + 'static,
    ) -> Result<Self, SessionError> {
        let state = Arc::new(MacMediaState::default());
        let worker_state = Arc::clone(&state);
        let worker = thread::Builder::new()
            .name("maho-host-macos-media".into())
            .spawn(move || {
                if let Err(error) = run(&worker_state, &sender) {
                    if !worker_state.is_cancelled() {
                        worker_state.publish(&sender, MediaEvent::Error(error.to_string()));
                    }
                }
                worker_state
                    .cancelled
                    .store(true, std::sync::atomic::Ordering::Release);
            })?;
        Ok(Self {
            state,
            worker: Some(worker),
        })
    }

    fn wake(&self) {
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
}

#[cfg(target_os = "macos")]
impl MediaSource for MacMediaSource {
    fn start(&self, sender: SyncSender<MediaEvent>) -> Result<Box<dyn MediaHandle>, SessionError> {
        let config = self.config;
        let encoder_config = self.encoder;
        Ok(Box::new(MacMediaHandle::spawn(
            sender,
            move |state, sender| {
                // SCK objects never cross this thread boundary. The caller already
                // owns the cancellation handle while native callbacks are pending.
                let (capture, capture_rx) = ScreenCapture::start_cancellable(
                    config,
                    &state.cancelled,
                    Instant::now() + Duration::from_secs(15),
                )?;
                let _capture = MacCaptureOwner(Some(capture));
                if state.is_cancelled() {
                    return Ok(());
                }
                let (encoder, encoded_rx) = VideoToolboxEncoder::start(encoder_config)?;
                let result = (|| {
                    while !state.is_cancelled() {
                        state.flush_controls(&encoder.sender)?;
                        let mut progressed = false;
                        // One packet at a time preserves compressed output order and
                        // bounds pending memory even if the session receiver stalls.
                        match encoded_rx.try_recv() {
                            Ok(Ok(frame)) => {
                                progressed = true;
                                if !state.publish(
                                    sender,
                                    MediaEvent::Video(VideoFrame {
                                        data: frame.data,
                                        is_key_frame: frame.is_key_frame,
                                        capture_at: frame.capture_at,
                                        encode_started_at: frame.encode_started_at,
                                        encode_completed_at: frame.encode_completed_at,
                                    }),
                                ) {
                                    return Ok(());
                                }
                            }
                            Ok(Err(error)) => return Err(SessionError::from(error)),
                            Err(mpsc::TryRecvError::Empty) => {}
                            Err(mpsc::TryRecvError::Disconnected) => {
                                return Err(SessionError::MediaStopped)
                            }
                        }
                        match capture_rx.try_recv() {
                            Ok(CaptureEvent::Video(frame)) => {
                                progressed = true;
                                if !submit_capture_frame(&encoder.sender, frame) {
                                    return Err(SessionError::MediaStopped);
                                }
                            }
                            Ok(CaptureEvent::Audio { pcm_f32_le, .. }) => {
                                progressed = true;
                                if !state.publish(sender, MediaEvent::Audio(pcm_f32_le)) {
                                    return Ok(());
                                }
                            }
                            Ok(CaptureEvent::Stopped(error)) => {
                                return Err(SessionError::Io(io::Error::other(error)));
                            }
                            Err(mpsc::TryRecvError::Empty) => {}
                            Err(mpsc::TryRecvError::Disconnected) => {
                                return Err(SessionError::MediaStopped)
                            }
                        }
                        if !progressed {
                            thread::park_timeout(Duration::from_millis(2));
                        }
                    }
                    Ok(())
                })();
                // Release native output backpressure before stopping its producer.
                // No bridge spawn can fail after capture/encoder initialization.
                drop(encoded_rx);
                encoder.stop();
                result
            },
        )?))
    }
}

#[cfg(target_os = "macos")]
fn submit_capture_frame(
    sender: &SyncSender<crate::encode_vt::Command>,
    frame: CaptureFrame,
) -> bool {
    match sender.try_send(crate::encode_vt::Command::Frame(frame)) {
        Ok(()) | Err(mpsc::TrySendError::Full(_)) => true,
        Err(mpsc::TrySendError::Disconnected(_)) => false,
    }
}

#[cfg(target_os = "macos")]
impl MediaHandle for MacMediaHandle {
    fn force_key_frame(&self) -> Result<(), SessionError> {
        if self.state.is_cancelled() {
            return Err(SessionError::MediaStopped);
        }
        self.state
            .controls
            .lock()
            .map_err(|_| SessionError::MediaStopped)?
            .force_key_frame = true;
        self.wake();
        Ok(())
    }

    fn update_bitrate(&self, bitrate: u32) -> Result<(), SessionError> {
        if self.state.is_cancelled() {
            return Err(SessionError::MediaStopped);
        }
        self.state
            .controls
            .lock()
            .map_err(|_| SessionError::MediaStopped)?
            .bitrate = Some(bitrate);
        self.wake();
        Ok(())
    }

    fn stop(&mut self) {
        self.state
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        self.wake();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                warn!("macOS media worker panicked");
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for MacMediaHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(all(test, target_os = "macos"))]
mod mac_media_tests {
    use super::*;
    use crate::encode_vt::Command;

    const BOUND: Duration = Duration::from_secs(2);

    #[test]
    fn pending_startup_accepts_coalesced_controls_and_stop_joins() {
        // Given: the production owner-worker protocol with a native callback
        // that never arrives. The non-Send resource stays inside that owner.
        let (output, _receiver) = mpsc::sync_channel(1);
        let (entered_tx, entered) = mpsc::channel();
        let (exited_tx, exited) = mpsc::channel();
        let handle = MacMediaHandle::spawn(output, move |state, _| {
            let resource = std::rc::Rc::new(());
            entered_tx.send(()).unwrap();
            while !state.is_cancelled() {
                thread::park();
            }
            drop(resource);
            exited_tx.send(()).unwrap();
            Ok(())
        })
        .unwrap();
        entered.recv_timeout(BOUND).unwrap();
        // When: controls arrive during startup, followed by disconnect.
        for bitrate in 1..=10_000 {
            handle.force_key_frame().unwrap();
            handle.update_bitrate(bitrate).unwrap();
        }
        {
            let controls = handle.state.controls.lock().unwrap();
            assert!(controls.force_key_frame);
            assert_eq!(controls.bitrate, Some(10_000));
        }
        let (done_tx, done) = mpsc::channel();
        let stopper = thread::spawn(move || {
            let mut handle = handle;
            handle.stop();
            assert!(matches!(
                handle.force_key_frame(),
                Err(SessionError::MediaStopped)
            ));
            handle.stop(); // Idempotent, including subsequent Drop.
            done_tx.send(()).unwrap();
        });
        // Then: stop owns the join and does not require a native callback.
        let stopped = done.recv_timeout(BOUND);
        assert_eq!(exited.recv_timeout(BOUND), Ok(()));
        stopper.join().unwrap();
        assert_eq!(stopped, Ok(()));
    }

    #[test]
    fn full_output_stop_joins_before_receiver_is_released() {
        // Given: full output with its receiver intentionally retained.
        let (output, receiver) = mpsc::sync_channel(1);
        output.send(MediaEvent::Audio(vec![1])).unwrap();
        let (entered_tx, entered) = mpsc::channel();
        let handle = MacMediaHandle::spawn(output, move |state, sender| {
            entered_tx.send(()).unwrap();
            state.publish(sender, MediaEvent::Audio(vec![2]));
            Ok(())
        })
        .unwrap();
        entered.recv_timeout(BOUND).unwrap();
        let (done_tx, done) = mpsc::channel();
        // When: disconnect while the worker forwards to the full queue.
        let stopper = thread::spawn(move || {
            let mut handle = handle;
            handle.stop();
            done_tx.send(()).unwrap();
        });
        let stopped = done.recv_timeout(BOUND);
        drop(receiver); // Rescue a blocking-send regression before asserting.
        stopper.join().unwrap();
        // Then: output backpressure did not prevent owned shutdown.
        assert_eq!(stopped, Ok(()), "full output blocked media stop");
    }

    #[test]
    fn startup_controls_survive_full_encoder_queue_and_deliver_latest_bitrate() {
        // Given: startup pending and the actual encoder command queue full.
        let (output, _receiver) = mpsc::sync_channel(1);
        let (commands, receiver) = mpsc::sync_channel(2);
        commands.send(Command::UpdateBitrate(1)).unwrap();
        commands.send(Command::UpdateBitrate(2)).unwrap();
        let (resume_tx, resume) = mpsc::channel();
        let (full_tx, full) = mpsc::channel();
        let (drained_tx, drained) = mpsc::channel();
        let (delivered_tx, delivered) = mpsc::channel();
        let mut handle = MacMediaHandle::spawn(output, move |state, _| {
            resume.recv_timeout(BOUND).unwrap();
            state.flush_controls(&commands)?;
            full_tx.send(()).unwrap();
            drained.recv_timeout(BOUND).unwrap();
            state.flush_controls(&commands)?;
            delivered_tx.send(()).unwrap();
            Ok(())
        })
        .unwrap();
        // When: repeated controls are queued before startup completes.
        for bitrate in 3..=10_000 {
            handle.force_key_frame().unwrap();
            handle.update_bitrate(bitrate).unwrap();
        }
        resume_tx.send(()).unwrap();
        full.recv_timeout(BOUND).unwrap();
        assert!(matches!(
            receiver.recv_timeout(BOUND),
            Ok(Command::UpdateBitrate(1))
        ));
        assert!(matches!(
            receiver.recv_timeout(BOUND),
            Ok(Command::UpdateBitrate(2))
        ));
        drained_tx.send(()).unwrap();
        delivered.recv_timeout(BOUND).unwrap();
        // Then: overload retained exactly one keyframe and the latest bitrate.
        assert!(matches!(
            receiver.recv_timeout(BOUND),
            Ok(Command::ForceKeyFrame)
        ));
        assert!(matches!(
            receiver.recv_timeout(BOUND),
            Ok(Command::UpdateBitrate(10_000))
        ));
        handle.stop();
    }

    #[test]
    fn startup_permission_error_is_forwarded_as_media_error() {
        // Given: the native boundary rejects Screen Recording permission.
        let (output, receiver) = mpsc::sync_channel(1);
        let mut handle = MacMediaHandle::spawn(output, |_, _| {
            Err(crate::capture_macos::CaptureError::PermissionDenied.into())
        })
        .unwrap();
        // When: observe startup through the same session output protocol.
        let event = receiver.recv_timeout(BOUND).unwrap();
        handle.stop();
        // Then: a failure is emitted, not a synthetic desktop frame.
        assert!(matches!(event, MediaEvent::Error(_)));
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
}

#[cfg(test)]
#[derive(Clone)]
struct SyntheticMediaSource {
    frame_count: u32,
}

#[cfg(test)]
struct SyntheticMediaHandle(Option<thread::JoinHandle<()>>);

#[cfg(test)]
impl MediaSource for SyntheticMediaSource {
    fn start(&self, sender: SyncSender<MediaEvent>) -> Result<Box<dyn MediaHandle>, SessionError> {
        let count = self.frame_count;
        let producer = thread::spawn(move || {
            for index in 0..count {
                let capture_at = Instant::now();
                let encode_started_at = Instant::now();
                let mut data = Vec::new();
                let nalu = [0x26, 0x01, index as u8];
                data.extend_from_slice(&(nalu.len() as u32).to_be_bytes());
                data.extend_from_slice(&nalu);
                let frame = VideoFrame {
                    data,
                    is_key_frame: index == 0,
                    capture_at,
                    encode_started_at,
                    encode_completed_at: Instant::now(),
                };
                if sender.send(MediaEvent::Video(frame)).is_err() {
                    break;
                }
            }
        });
        Ok(Box::new(SyntheticMediaHandle(Some(producer))))
    }
}

#[cfg(test)]
impl MediaHandle for SyntheticMediaHandle {
    fn force_key_frame(&self) -> Result<(), SessionError> {
        Ok(())
    }

    fn update_bitrate(&self, _bitrate: u32) -> Result<(), SessionError> {
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(producer) = self.0.take() {
            producer.join().unwrap();
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
#[path = "native_pipeline.rs"]
mod native_pipeline;

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn native_media_error<T>(
    handoff: &native_pipeline::Handoff<T>,
    sender: &SyncSender<MediaEvent>,
    error: String,
) {
    // Never block a failing worker behind compressed output. Cancellation wakes
    // its sibling before reporting; a full queue is logged, not waited upon.
    handoff.stop();
    warn!(%error, "Native media pipeline stopped");
    match sender.try_send(MediaEvent::Error(error)) {
        Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => {}
        Err(mpsc::TrySendError::Full(_)) => {
            warn!("Native media error notification could not enter the full media queue");
        }
    }
}

#[cfg(target_os = "windows")]
struct WindowsMediaSource {
    display_index: usize,
    fps: u32,
    width: u32,
    height: u32,
    bitrate: u32,
    codec: VideoCodec,
    capture_audio: bool,
    /// Captured display origin in virtual-desktop physical pixels. `GetCursorInfo`
    /// reports virtual-desktop coordinates, so this offset must be subtracted
    /// before normalizing against the captured display.
    desktop_x: i32,
    desktop_y: i32,
}

#[cfg(target_os = "windows")]
use crate::windows_logic::freshness::RawFrame as WindowsRawFrame;

#[cfg(target_os = "windows")]
mod display_power {
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetThreadExecutionState(es_flags: u32) -> u32;
    }

    /// Keeps the console display (and system) awake while a streaming session
    /// is active — DXGI Desktop Duplication produces no frames while the panel
    /// is in DPMS sleep, which would leave clients with a black canvas.
    pub fn set_keep_awake(enabled: bool) {
        unsafe {
            let flags = if enabled {
                ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED
            } else {
                ES_CONTINUOUS
            };
            SetThreadExecutionState(flags);
        }
    }
}

#[cfg(target_os = "windows")]
mod cursor_jiggle {
    #[repr(C)]
    #[derive(Default)]
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        pub fn GetCursorPos(point: *mut Point) -> i32;
        fn SetCursorPos(x: i32, y: i32) -> i32;
    }

    /// Nudges the cursor by one pixel (alternating direction). Pointer updates
    /// force DWM to compose a frame even on an otherwise static desktop, which
    /// unblocks Desktop Duplication before the first frame ever arrives. Only
    /// called while no frame has been captured yet, so active sessions are
    /// never disturbed.
    pub fn nudge(odd: bool) {
        unsafe {
            let mut point = Point::default();
            if GetCursorPos(&mut point) != 0 {
                let dx = if odd { 1 } else { -1 };
                SetCursorPos(point.x + dx, point.y);
            }
        }
    }
}

#[cfg(target_os = "windows")]
struct WindowsSessionEncoder(MediaFoundationEncoder);

#[cfg(target_os = "windows")]
impl native_pipeline::Encoder<WindowsRawFrame> for WindowsSessionEncoder {
    type Output = VideoFrame;
    type Error = String;

    fn selected(&mut self, frame: &WindowsRawFrame) {
        if let Some(trace) = host_trace::enabled() {
            trace.record(host_trace::Record {
                event: 19,
                frame: frame.capture_id,
                value: trace.time(frame.published_at),
                repeat: frame.repeat,
                ..Default::default()
            });
        }
    }

    fn force_keyframe(&mut self) {
        self.0.force_key_frame();
    }

    fn bitrate(&mut self, bitrate: u32) -> Result<Vec<VideoFrame>, String> {
        self.0
            .update_bitrate(bitrate)
            .map_err(|error| format!("mf bitrate: {error}"))?;
        Ok(Vec::new())
    }

    fn encode(&mut self, frame: WindowsRawFrame) -> Result<Vec<VideoFrame>, String> {
        let trace = host_trace::enabled();
        if let Some(trace) = trace {
            let (content, residence) = frame.ages(Instant::now());
            for (event, value) in [
                (14, trace.time(frame.captured_at)),
                (15, u64::try_from(content.as_micros()).unwrap_or(u64::MAX)),
                (16, u64::try_from(residence.as_micros()).unwrap_or(u64::MAX)),
                (17, trace.time(frame.published_at)),
                (
                    18,
                    u64::try_from(frame.conversion.as_micros()).unwrap_or(u64::MAX),
                ),
            ] {
                trace.record(host_trace::Record {
                    event,
                    frame: frame.capture_id,
                    value,
                    repeat: frame.repeat,
                    ..Default::default()
                });
            }
        }
        let started = trace.map(|_| Instant::now());
        let result = self.0.encode_nv12(&frame.nv12, frame.captured_at);
        if let (Some(trace), Some(started)) = (trace, started) {
            trace.record(host_trace::Record {
                event: 40,
                frame: frame.capture_id,
                value: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                repeat: frame.repeat,
                ..Default::default()
            });
        }
        result
            .map(|frames| {
                frames
                    .into_iter()
                    .map(|encoded| VideoFrame {
                        data: encoded.data,
                        is_key_frame: encoded.is_key_frame,
                        capture_at: encoded.capture_at,
                        encode_started_at: encoded.encode_started_at,
                        encode_completed_at: encoded.encode_completed_at,
                    })
                    .collect()
            })
            .map_err(|error| format!("mf encode: {error}"))
    }
}

#[cfg(target_os = "windows")]
impl MediaSource for WindowsMediaSource {
    fn start(&self, sender: SyncSender<MediaEvent>) -> Result<Box<dyn MediaHandle>, SessionError> {
        use std::sync::atomic::{AtomicBool, Ordering};

        let mut workers = native_pipeline::Workers::<WindowsRawFrame>::new();
        let first_keyframe_emitted = Arc::new(AtomicBool::new(false));
        {
            let sender = sender.clone();
            let first_keyframe_emitted = Arc::clone(&first_keyframe_emitted);
            let display_index = self.display_index;
            let fps = self.fps.max(1);
            let origin_x = self.desktop_x;
            let origin_y = self.desktop_y;
            let mon_w = self.width;
            let mon_h = self.height;
            workers.spawn("maho-win-capture", true, move |handoff| {
                // SetThreadExecutionState is per-thread. Reset it on this same
                // owning thread on every exit, including initialization failure.
                struct KeepAwake;
                impl Drop for KeepAwake {
                    fn drop(&mut self) {
                        display_power::set_keep_awake(false);
                    }
                }
                display_power::set_keep_awake(true);
                let _awake = KeepAwake;
                let interval = Duration::from_micros(1_000_000 / u64::from(fps));
                let mut capture =
                    match WindowsCapture::new(display_index, Duration::from_millis(33)) {
                        Ok(capture) => capture,
                        Err(error) => {
                            native_media_error(&handoff, &sender, format!("dxgi init: {error}"));
                            return;
                        }
                    };
                // Share the immutable NV12 allocation with the keepalive cache.
                // Repeats retain the real original capture instant.
                let mut last_frame: Option<WindowsRawFrame> = None;
                let mut capture_id = 0_u64;
                let mut consecutive_failures = 0_u32;
                let mut last_emit = Instant::now();
                let mut jiggle_flip = false;
                let mut last_jiggle = Instant::now() - Duration::from_millis(700);
                let send_cursor = |sender: &SyncSender<MediaEvent>,
                                   event: MediaEvent,
                                   frame_id: u64| {
                    let trace = host_trace::enabled();
                    let started = trace.map(|_| Instant::now());
                    if let Some(trace) = trace {
                        trace.record(host_trace::Record {
                            event: 21,
                            frame: frame_id,
                            ..Default::default()
                        });
                    }
                    let res = sender.send(event);
                    if let (Some(trace), Some(started)) = (trace, started) {
                        trace.record(host_trace::Record {
                            event: 22,
                            frame: frame_id,
                            value: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                            repeat: res.is_err(),
                            ..Default::default()
                        });
                    }
                };
                let query_cursor = || -> (f32, f32, u8) {
                    use windows::Win32::UI::WindowsAndMessaging::{
                        GetCursorInfo, CURSORINFO, CURSOR_SHOWING,
                    };
                    let mut ci = CURSORINFO {
                        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
                        flags: windows::Win32::UI::WindowsAndMessaging::CURSORINFO_FLAGS(0),
                        hCursor: Default::default(),
                        ptScreenPos: Default::default(),
                    };
                    let is_showing = unsafe {
                        GetCursorInfo(&mut ci).is_ok()
                            && (ci.flags.0 & CURSOR_SHOWING.0) == CURSOR_SHOWING.0
                    };
                    if !is_showing {
                        return (-1.0, -1.0, 0);
                    }
                    let px = ci.ptScreenPos.x - origin_x;
                    let py = ci.ptScreenPos.y - origin_y;
                    if px >= 0
                        && px < mon_w as i32
                        && py >= 0
                        && py < mon_h as i32
                        && mon_w > 0
                        && mon_h > 0
                    {
                        let cx = (px as f32) / (mon_w as f32);
                        let cy = (py as f32) / (mon_h as f32);
                        (cx.clamp(0.0, 1.0), cy.clamp(0.0, 1.0), 1)
                    } else {
                        (-1.0, -1.0, 0)
                    }
                };
                while !handoff.is_stopped() {
                    let started = Instant::now();
                    if let Some(trace) = host_trace::enabled() {
                        trace.record(host_trace::Record {
                            event: 9,
                            frame: capture_id + 1,
                            ..Default::default()
                        });
                    }
                    match capture.acquire_next_frame(Duration::from_millis(33)) {
                        Ok(frame) => {
                            consecutive_failures = 0;
                            if let Some(trace) = host_trace::enabled() {
                                trace.record(host_trace::Record {
                                    event: 10,
                                    frame: capture_id + 1,
                                    value: trace.time(started),
                                    ..Default::default()
                                });
                            }
                            if frame.pointer_visible
                                && frame.pointer_position.is_some()
                                && frame.width > 0
                                && frame.height > 0
                            {
                                let (px, py) = frame.pointer_position.unwrap();
                                let cx = (px as f32) / (frame.width as f32);
                                let cy = (py as f32) / (frame.height as f32);
                                send_cursor(
                                    &sender,
                                    MediaEvent::Cursor(CursorUpdate {
                                        x: cx.clamp(0.0, 1.0),
                                        y: cy.clamp(0.0, 1.0),
                                        cursor_type: 1,
                                    }),
                                    capture_id + 1,
                                );
                            } else {
                                let (cx, cy, ctype) = query_cursor();
                                send_cursor(
                                    &sender,
                                    MediaEvent::Cursor(CursorUpdate {
                                        x: cx,
                                        y: cy,
                                        cursor_type: ctype,
                                    }),
                                    capture_id + 1,
                                );
                            }
                            let captured_at = Instant::now();
                            let nv12 = match bgra_to_nv12(
                                frame.width,
                                frame.height,
                                &frame.bgra,
                                frame.stride as usize,
                            ) {
                                Ok(nv12) => nv12,
                                Err(error) => {
                                    native_media_error(
                                        &handoff,
                                        &sender,
                                        format!("nv12 conversion: {error}"),
                                    );
                                    return;
                                }
                            };
                            capture_id += 1;
                            let published_at = Instant::now();
                            let raw = WindowsRawFrame {
                                nv12: Arc::new(nv12),
                                captured_at,
                                published_at,
                                capture_id,
                                repeat: false,
                                conversion: published_at.saturating_duration_since(captured_at),
                            };
                            last_frame = Some(raw.clone());
                            if let Some(trace) = host_trace::enabled() {
                                trace.record(host_trace::Record {
                                    event: 11,
                                    frame: capture_id,
                                    value: trace.time(captured_at),
                                    ..Default::default()
                                });
                                trace.record(host_trace::Record {
                                    event: 12,
                                    frame: capture_id,
                                    value: trace.time(published_at),
                                    ..Default::default()
                                });
                            }
                            last_emit = started;
                            if !handoff.publish(raw) {
                                break;
                            }
                        }
                        Err(CaptureError::Timeout) => {
                            let (cx, cy, ctype) = query_cursor();
                            send_cursor(
                                &sender,
                                MediaEvent::Cursor(CursorUpdate {
                                    x: cx,
                                    y: cy,
                                    cursor_type: ctype,
                                }),
                                capture_id,
                            );
                            if (!first_keyframe_emitted.load(Ordering::Relaxed)
                                || last_frame.is_none())
                                && last_jiggle.elapsed() >= Duration::from_millis(300)
                            {
                                last_jiggle = started;
                                jiggle_flip = !jiggle_flip;
                                cursor_jiggle::nudge(jiggle_flip);
                            }
                            let keepalive_interval =
                                if first_keyframe_emitted.load(Ordering::Relaxed) {
                                    Duration::from_millis(500)
                                } else {
                                    Duration::from_millis(33)
                                };
                            if last_emit.elapsed() >= keepalive_interval {
                                if let Some(frame) = &last_frame {
                                    last_emit = started;
                                    if let Some(trace) = host_trace::enabled() {
                                        trace.record(host_trace::Record {
                                            event: 13,
                                            frame: frame.capture_id,
                                            value: trace.time(frame.captured_at),
                                            ..Default::default()
                                        });
                                    }
                                    if !handoff.publish(frame.repeated(Instant::now())) {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            use crate::windows_session::{capture_recovery, CaptureRecovery};

                            let failure = capture_failure_from_label(match &error {
                                CaptureError::AccessLost => "access_lost",
                                CaptureError::AccessDenied => "access_denied",
                                CaptureError::RefreshFailure => "refresh_failure",
                                _ => "capture",
                            });
                            warn!(%error, consecutive_failures, "DXGI capture interrupted");
                            while !handoff.is_stopped() {
                                match capture_recovery(failure, consecutive_failures) {
                                    CaptureRecovery::Reacquire(delay) => {
                                        thread::sleep(delay);
                                        if handoff.is_stopped() {
                                            break;
                                        }
                                        let rebuilt = WindowsCapture::new(
                                            display_index,
                                            Duration::from_millis(33),
                                        );
                                        consecutive_failures =
                                            consecutive_failures.saturating_add(1);
                                        match rebuilt {
                                            Ok(rebuilt) => {
                                                capture = rebuilt;
                                                break;
                                            }
                                            Err(error) => {
                                                warn!(%error, consecutive_failures, "DXGI reacquire failed");
                                            }
                                        }
                                    }
                                    CaptureRecovery::Abort => {
                                        native_media_error(
                                            &handoff,
                                            &sender,
                                            format!("dxgi: {error}"),
                                        );
                                        return;
                                    }
                                }
                            }
                            continue;
                        }
                    }
                    handoff.pace_until(started + interval);
                }
            })?;
        }
        if self.capture_audio {
            let sender = sender.clone();
            workers.spawn("maho-win-audio", false, move |handoff| {
                let mut capture =
                    match crate::audio_windows::WindowsAudioCapture::new() {
                        Ok(capture) => capture,
                        Err(error) => {
                            tracing::warn!(%error, "Windows audio loopback unavailable; continuing without audio");
                            return;
                        }
                    };
                info!("Windows audio loopback started");
                while !handoff.is_stopped() {
                    let pcm = capture.poll();
                    if pcm.is_empty() {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    let _ = sender.send(MediaEvent::Audio(pcm));
                }
            })?;
        }
        let config = EncoderConfig {
            width: self.width,
            height: self.height,
            bitrate: self.bitrate,
            fps: self.fps.max(1),
            keyframe_interval: self.fps.max(1),
            preferred_codec: self.codec,
        };
        workers.spawn("maho-win-encode", true, move |handoff| {
            let result = native_pipeline::run_encoder(
                &handoff,
                || {
                    MediaFoundationEncoder::new(config)
                        .map(WindowsSessionEncoder)
                        .map_err(|error| format!("mf init: {error}"))
                },
                |frame| {
                    let is_key = frame.is_key_frame;
                    let trace = host_trace::enabled();
                    let started = trace.map(|_| Instant::now());
                    if let Some(trace) = trace {
                        trace.record(host_trace::Record {
                            event: 23,
                            keyframe: is_key,
                            size: frame.data.len(),
                            ..Default::default()
                        });
                    }
                    let sent = sender.send(MediaEvent::Video(frame)).is_ok();
                    if let (Some(trace), Some(started)) = (trace, started) {
                        trace.record(host_trace::Record {
                            event: 24,
                            keyframe: is_key,
                            value: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                            repeat: !sent,
                            ..Default::default()
                        });
                    }
                    if sent && is_key {
                        first_keyframe_emitted.store(true, Ordering::Relaxed);
                    }
                    sent
                },
            );
            if let Err(error) = result {
                native_media_error(&handoff, &sender, error);
            }
        })?;
        workers.handoff.activate();
        Ok(Box::new(WindowsMediaHandle { workers }))
    }
}

#[cfg(target_os = "windows")]
struct WindowsMediaHandle {
    workers: native_pipeline::Workers<WindowsRawFrame>,
}

#[cfg(target_os = "windows")]
impl MediaHandle for WindowsMediaHandle {
    fn force_key_frame(&self) -> Result<(), SessionError> {
        self.workers
            .handoff
            .control(true, None)
            .then_some(())
            .ok_or(SessionError::MediaStopped)
    }

    fn update_bitrate(&self, bitrate: u32) -> Result<(), SessionError> {
        self.workers
            .handoff
            .control(false, Some(bitrate))
            .then_some(())
            .ok_or(SessionError::MediaStopped)
    }

    fn stop(&mut self) {
        self.workers.stop();
    }
}

#[cfg(target_os = "linux")]
struct LinuxMediaSource {
    fps: u32,
    width: u32,
    height: u32,
    bitrate: u32,
    capture_audio: bool,
    output_name: Option<String>,
}

#[cfg(target_os = "linux")]
struct LinuxRawFrame {
    bgra: Vec<u8>,
    stride: usize,
    captured_at: Instant,
}

#[cfg(target_os = "linux")]
struct LinuxSessionEncoder {
    encoder: LinuxVideoEncoder,
    times: native_pipeline::FrameTimes,
}

#[cfg(target_os = "linux")]
impl LinuxSessionEncoder {
    fn output(
        &mut self,
        frames: Vec<crate::encode_linux::EncodedFrame>,
    ) -> Result<Vec<VideoFrame>, String> {
        let encode_completed_at = Instant::now();
        frames
            .into_iter()
            .map(|encoded| {
                let (capture_at, encode_started_at) =
                    self.times.take(encoded.pts).ok_or_else(|| {
                        format!("encoder output has unknown input PTS {}", encoded.pts)
                    })?;
                Ok(VideoFrame {
                    data: encoded.data,
                    is_key_frame: encoded.is_key_frame,
                    capture_at,
                    encode_started_at,
                    encode_completed_at,
                })
            })
            .collect()
    }
}

#[cfg(target_os = "linux")]
impl native_pipeline::Encoder<LinuxRawFrame> for LinuxSessionEncoder {
    type Output = VideoFrame;
    type Error = String;

    fn force_keyframe(&mut self) {
        self.encoder.force_key_frame();
    }

    fn bitrate(&mut self, bitrate: u32) -> Result<Vec<VideoFrame>, String> {
        let frames = self
            .encoder
            .update_bitrate(bitrate as usize)
            .map_err(|error| format!("encoder bitrate: {error}"))?;
        self.output(frames)
    }

    fn encode(&mut self, frame: LinuxRawFrame) -> Result<Vec<VideoFrame>, String> {
        self.times.submitted(frame.captured_at, Instant::now());
        let frames = self
            .encoder
            .encode_bgra(&frame.bgra, frame.stride)
            .map_err(|error| format!("encode: {error}"))?;
        self.output(frames)
    }
}

#[cfg(target_os = "linux")]
impl MediaSource for LinuxMediaSource {
    fn start(&self, sender: SyncSender<MediaEvent>) -> Result<Box<dyn MediaHandle>, SessionError> {
        let mut workers = native_pipeline::Workers::<LinuxRawFrame>::new();
        {
            let sender = sender.clone();
            let fps = self.fps.max(1);
            let output_name = self.output_name.clone();
            workers.spawn("maho-linux-capture", true, move |handoff| {
                let interval = Duration::from_micros(1_000_000 / u64::from(fps));
                let capture_config = LinuxCaptureConfig {
                    output_name,
                    ..LinuxCaptureConfig::default()
                };
                let mut capture = match LinuxCapture::connect_cancellable(
                    capture_config,
                    handoff.cancellation(),
                    Instant::now() + Duration::from_secs(5),
                ) {
                    Ok(capture) => capture,
                    Err(error) => {
                        if !handoff.is_stopped() {
                            native_media_error(&handoff, &sender, format!("capture init: {error}"));
                        }
                        return;
                    }
                };
                while !handoff.is_stopped() {
                    let started = Instant::now();
                    match capture.capture_frame_cancellable(
                        handoff.cancellation(),
                        Duration::from_millis(50),
                    ) {
                        Ok(Some(frame)) => {
                            if !handoff.publish(LinuxRawFrame {
                                bgra: frame.bgra,
                                stride: frame.stride as usize,
                                captured_at: Instant::now(),
                            }) {
                                break;
                            }
                            handoff.pace_until(started + interval);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            if !handoff.is_stopped() {
                                native_media_error(&handoff, &sender, format!("capture: {error}"));
                            }
                            return;
                        }
                    }
                }
            })?;
        }
        {
            let sender = sender.clone();
            let config = LinuxEncoderConfig {
                width: self.width,
                height: self.height,
                bitrate: self.bitrate as usize,
                fps: self.fps.max(1),
                keyframe_interval: self.fps.max(1),
                preferred_codec: LinuxVideoCodec::Hevc,
            };
            workers.spawn("maho-linux-encode", true, move |handoff| {
                let result = native_pipeline::run_encoder(
                    &handoff,
                    || {
                        LinuxVideoEncoder::new(config)
                            .map(|encoder| LinuxSessionEncoder {
                                encoder,
                                times: native_pipeline::FrameTimes::new(),
                            })
                            .map_err(|error| format!("encoder init: {error}"))
                    },
                    |frame| sender.send(MediaEvent::Video(frame)).is_ok(),
                );
                if let Err(error) = result {
                    native_media_error(&handoff, &sender, error);
                }
            })?;
        }
        if self.capture_audio {
            workers.spawn("maho-linux-audio", false, move |handoff| {
                use crate::audio_linux::LinuxAudioCapture;
                let mut capture = match LinuxAudioCapture::open_cancellable(handoff.cancellation())
                {
                    Ok(capture) => capture,
                    Err(error) => {
                        warn!(%error, "Audio capture unavailable");
                        return;
                    }
                };
                let slot = Arc::new(native_pipeline::LatestAudio::default());
                let mut samples = [0.0_f32; 1920];
                while !handoff.is_stopped() {
                    match capture.read_interleaved_f32_cancellable(
                        &mut samples,
                        handoff.cancellation(),
                        Duration::from_millis(50),
                    ) {
                        Ok(0) => {}
                        Ok(count) => {
                            let captured_at = Instant::now();
                            let mut pcm = Vec::with_capacity(count * 4);
                            for sample in &samples[..count] {
                                pcm.extend_from_slice(&sample.to_le_bytes());
                            }
                            if slot.publish(native_pipeline::AudioBlock { pcm, captured_at }) {
                                match sender.try_send(MediaEvent::NativeAudio(Arc::clone(&slot))) {
                                    Ok(()) => {}
                                    Err(mpsc::TrySendError::Full(_)) => {
                                        slot.notification_rejected()
                                    }
                                    Err(mpsc::TrySendError::Disconnected(_)) => {
                                        handoff.stop();
                                        break;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            if !handoff.is_stopped() {
                                warn!(%error, "Audio capture ended");
                            }
                            break;
                        }
                    }
                }
                // Drop owns recorder shutdown/reap on this thread.
            })?;
        }
        workers.handoff.activate();
        Ok(Box::new(LinuxMediaHandle { workers }))
    }
}

#[cfg(target_os = "linux")]
struct LinuxMediaHandle {
    workers: native_pipeline::Workers<LinuxRawFrame>,
}

#[cfg(target_os = "linux")]
impl MediaHandle for LinuxMediaHandle {
    fn force_key_frame(&self) -> Result<(), SessionError> {
        self.workers
            .handoff
            .control(true, None)
            .then_some(())
            .ok_or(SessionError::MediaStopped)
    }

    fn update_bitrate(&self, bitrate: u32) -> Result<(), SessionError> {
        self.workers
            .handoff
            .control(false, Some(bitrate))
            .then_some(())
            .ok_or(SessionError::MediaStopped)
    }

    fn stop(&mut self) {
        self.workers.stop();
    }
}

/// BT.601 limited-range BGRA8 -> NV12 conversion for tightly packed input.
#[cfg(target_os = "windows")]
fn bgra_to_nv12(
    width: u32,
    height: u32,
    bgra: &[u8],
    stride: usize,
) -> Result<Vec<u8>, &'static str> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
        return Err("display dimensions must be non-zero and even");
    }
    if bgra.len() < stride * h || stride < w * 4 {
        return Err("bgra buffer smaller than stride * height");
    }
    let y_plane = w * h;
    let mut nv12 = vec![128u8; y_plane + y_plane / 2];
    let (y_plane, uv_plane) = nv12.split_at_mut(y_plane);
    for row in 0..h {
        let src = &bgra[row * stride..row * stride + w * 4];
        let y_row = &mut y_plane[row * w..(row + 1) * w];
        for col in (0..w).step_by(2) {
            let (b0, g0, r0) = (
                src[col * 4] as u32,
                src[col * 4 + 1] as u32,
                src[col * 4 + 2] as u32,
            );
            let (b1, g1, r1) = (
                src[col * 4 + 4] as u32,
                src[col * 4 + 5] as u32,
                src[col * 4 + 6] as u32,
            );
            y_row[col] = ((77 * r0 + 150 * g0 + 29 * b0) >> 8) as u8;
            y_row[col + 1] = ((77 * r1 + 150 * g1 + 29 * b1) >> 8) as u8;
            let (b_avg, g_avg, r_avg) = ((b0 + b1) / 2, (g0 + g1) / 2, (r0 + r1) / 2);
            let uv_index = (row / 2) * w + col;
            uv_plane[uv_index] =
                (128 + ((-43 * r_avg as i32 - 85 * g_avg as i32 + 128 * b_avg as i32) >> 8)) as u8;
            uv_plane[uv_index + 1] =
                (128 + ((128 * r_avg as i32 - 107 * g_avg as i32 - 21 * b_avg as i32) >> 8)) as u8;
        }
    }
    Ok(nv12)
}

pub struct HostServer {
    config: HostConfig,
    tcp_listener: TcpListener,
    udp_socket: UdpSocket,
    media_source: Arc<dyn MediaSource>,
    lockout: Arc<Mutex<BootstrapLockout>>,
    pairing_deadline: Option<Instant>,
    bootstrap_identity: Option<PskIdentity>,
    preauth_timeout: Duration,
    _advertisement: Option<maho_net::discovery::ServiceAdvertiser>,
    #[cfg(all(test, target_os = "linux"))]
    prepared_admission_input: Mutex<Option<LinuxInputInjector>>,
}

#[cfg(test)]
thread_local! {
    static BOOTSTRAP_DERIVATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn derive_bootstrap_identity(pin: &str) -> Result<PskIdentity, TlsPskError> {
    #[cfg(test)]
    BOOTSTRAP_DERIVATIONS.with(|count| count.set(count.get() + 1));
    PskIdentity::bootstrap(pin)
}

impl HostServer {
    pub fn bind(config: HostConfig) -> Result<Self, SessionError> {
        let fps = if config.frames_per_second == 0 {
            60
        } else {
            config.frames_per_second
        };
        let media_source = Self::default_media_source(&config, fps)?;
        Self::bind_with_media_advertising(config, media_source, true)
    }

    #[cfg(target_os = "macos")]
    fn default_media_source(
        config: &HostConfig,
        fps: u32,
    ) -> Result<Arc<dyn MediaSource>, SessionError> {
        Ok(Arc::new(MacMediaSource {
            config: CaptureConfig {
                width: config.display.pixel_width,
                height: config.display.pixel_height,
                frames_per_second: fps,
                capture_audio: config.capture_audio,
            },
            encoder: EncoderConfig {
                width: config.display.pixel_width,
                height: config.display.pixel_height,
                frames_per_second: fps,
                bitrate: config.bitrate,
                key_frame_interval: fps,
            },
        }))
    }

    #[cfg(target_os = "linux")]
    fn default_media_source(
        config: &HostConfig,
        fps: u32,
    ) -> Result<Arc<dyn MediaSource>, SessionError> {
        Ok(Arc::new(LinuxMediaSource {
            fps,
            width: config.display.pixel_width,
            height: config.display.pixel_height,
            bitrate: config.bitrate,
            capture_audio: config.capture_audio,
            output_name: config.output_name.clone(),
        }))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    fn default_media_source(
        _config: &HostConfig,
        _fps: u32,
    ) -> Result<Arc<dyn MediaSource>, SessionError> {
        Err(SessionError::Store(
            "no media source is wired for this platform yet".into(),
        ))
    }

    #[cfg(target_os = "windows")]
    fn default_media_source(
        config: &HostConfig,
        fps: u32,
    ) -> Result<Arc<dyn MediaSource>, SessionError> {
        Ok(Arc::new(WindowsMediaSource {
            display_index: 0,
            fps,
            width: config.display.pixel_width,
            height: config.display.pixel_height,
            bitrate: config.bitrate,
            codec: VideoCodec::H264,
            capture_audio: config.capture_audio,
            desktop_x: config.display.desktop_x,
            desktop_y: config.display.desktop_y,
        }))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    fn default_media_source(
        _config: &HostConfig,
        _fps: u32,
    ) -> Result<Arc<dyn MediaSource>, SessionError> {
        Err(SessionError::Store(
            "no media source is wired for this platform yet".into(),
        ))
    }

    #[cfg(test)]
    fn bind_synthetic(config: HostConfig, frame_count: u32) -> Result<Self, SessionError> {
        Self::bind_with_media_advertising(
            config,
            Arc::new(SyntheticMediaSource { frame_count }),
            false,
        )
    }

    #[cfg(test)]
    fn bind_with_media(
        config: HostConfig,
        media_source: Arc<dyn MediaSource>,
    ) -> Result<Self, SessionError> {
        Self::bind_with_media_advertising(config, media_source, false)
    }

    fn bind_with_media_advertising(
        config: HostConfig,
        media_source: Arc<dyn MediaSource>,
        advertise: bool,
    ) -> Result<Self, SessionError> {
        if config
            .bootstrap_pin
            .as_ref()
            .is_some_and(|pin| pin.len() != 8 || !pin.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err(SessionError::InvalidPin);
        }
        let tcp_listener = TcpListener::bind(config.tcp_addr)?;
        let udp_socket = UdpSocket::bind(config.udp_addr)?;
        let _ = rustix::net::sockopt::set_socket_recv_buffer_size(&udp_socket, 4 * 1024 * 1024);
        let _ = rustix::net::sockopt::set_socket_send_buffer_size(&udp_socket, 4 * 1024 * 1024);
        udp_socket.set_nonblocking(true)?;
        let lockout = Arc::new(Mutex::new(BootstrapLockout::default()));
        let bootstrap_identity = config
            .bootstrap_pin
            .as_deref()
            .map(derive_bootstrap_identity)
            .transpose()?;
        let pairing_deadline = config
            .bootstrap_pin
            .as_ref()
            .map(|_| Instant::now() + config.pairing_window);
        if pairing_deadline.is_some() {
            lockout.lock().expect("lockout poisoned").begin_pairing();
        }
        let bound_tcp = tcp_listener.local_addr()?;
        let bound_udp = udp_socket.local_addr()?;
        let _advertisement = if advertise {
            match maho_net::discovery::ServiceAdvertiser::start(
                &config.host_name,
                bound_tcp.port(),
                bound_udp.port(),
                bound_tcp,
            ) {
                Ok(adv) => {
                    tracing::info!(
                        host = %config.host_name,
                        tcp = %bound_tcp,
                        udp = %bound_udp,
                        "Started LAN discovery advertisement"
                    );
                    Some(adv)
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to start LAN discovery advertisement: {e}. Direct host connectivity remains usable."
                    );
                    None
                }
            }
        } else {
            None
        };
        Ok(Self {
            config,
            tcp_listener,
            udp_socket,
            media_source,
            lockout,
            pairing_deadline,
            bootstrap_identity,
            preauth_timeout: Duration::from_secs(10),
            _advertisement,
            #[cfg(all(test, target_os = "linux"))]
            prepared_admission_input: Mutex::new(None),
        })
    }

    pub fn is_advertising(&self) -> bool {
        self._advertisement.is_some()
    }

    pub fn tcp_addr(&self) -> Result<SocketAddr, SessionError> {
        Ok(self.tcp_listener.local_addr()?)
    }

    pub fn udp_addr(&self) -> Result<SocketAddr, SessionError> {
        Ok(self.udp_socket.local_addr()?)
    }

    pub fn serve(self) -> Result<(), SessionError> {
        self.serve_with_stop(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            false,
        )))
    }

    pub fn serve_with_stop(
        self,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), SessionError> {
        self.tcp_listener.set_nonblocking(true)?;
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
                            {
                                tracing::warn!(%error, "connection ended with an error");
                            }
                        }
                        Err(error) => {
                            let _ = error;
                            let locked = self
                                .lockout
                                .lock()
                                .expect("lockout poisoned")
                                .record_failure(std::time::Instant::now());
                            if locked {
                                tracing::warn!("bootstrap TLS path locked after repeated failures");
                            }
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
                Err(e) => return Err(SessionError::Io(e)),
            }
        }
        Ok(())
    }

    pub fn serve_n(&self, connection_count: usize) -> Result<(), SessionError> {
        for _ in 0..connection_count {
            self.serve_next()?;
        }
        Ok(())
    }

    fn serve_next(&self) -> Result<(), SessionError> {
        let (tcp, peer) = self.tcp_listener.accept()?;
        let admission_deadline = Instant::now() + self.preauth_timeout;
        tcp.set_nodelay(true)?;
        let tls_server = TlsPskServer::new(self.current_psks()?)?;
        match tls_server.accept_stream_until(tcp, admission_deadline) {
            Ok(stream) => self.handle_connection(stream, peer, admission_deadline),
            Err(error) => {
                let locked = self
                    .lockout
                    .lock()
                    .expect("lockout poisoned")
                    .record_failure(Instant::now());
                if locked {
                    warn!("bootstrap TLS path locked after repeated failures");
                }
                Err(SessionError::Tls(error))
            }
        }
    }

    fn current_psks(&self) -> Result<Vec<PskIdentity>, SessionError> {
        let mut psks = self
            .config
            .pairing_store
            .load_all()?
            .into_iter()
            .map(|record| PskIdentity::pairing(&record.id, &record.key))
            .collect::<Result<Vec<_>, _>>()?;
        let pairing_active = self
            .pairing_deadline
            .is_some_and(|deadline| Instant::now() < deadline)
            && self
                .lockout
                .lock()
                .expect("lockout poisoned")
                .is_allowed(Instant::now());
        if pairing_active {
            if let Some(identity) = &self.bootstrap_identity {
                psks.push(identity.clone());
            }
        }
        if psks.is_empty() {
            let mut disabled_key = [0_u8; 32];
            rand::rng().fill_bytes(&mut disabled_key);
            psks.push(PskIdentity::new("maho-disabled", disabled_key.to_vec())?);
        }
        Ok(psks)
    }

    fn is_pairing_active(&self) -> bool {
        self.pairing_deadline
            .is_some_and(|deadline| Instant::now() < deadline)
            && self
                .lockout
                .lock()
                .expect("lockout poisoned")
                .is_allowed(Instant::now())
    }

    fn handle_connection(
        &self,
        mut stream: maho_net::TlsPskStream<TcpStream>,
        tcp_peer: SocketAddr,
        admission_deadline: Instant,
    ) -> Result<(), SessionError> {
        stream
            .ssl_stream_mut()
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(5)))?;
        let negotiated_identity = stream
            .negotiated_identity()
            .and_then(|identity| std::str::from_utf8(identity).ok())
            .unwrap_or_default()
            .to_owned();
        let mut state = SessionState::PreAuth;
        let mut granted_pairing_id: Option<String> = None;
        let mut c2h_cipher = None;
        let mut h2c_cipher = None;
        let mut udp_peer = None;
        let mut media_receiver: Option<Receiver<MediaEvent>> = None;
        let mut media_handle: Option<Box<dyn MediaHandle>> = None;
        #[cfg(target_os = "macos")]
        let input = InputInjector::new(
            self.config.display.logical_width as f32,
            self.config.display.logical_height as f32,
        );
        #[cfg(target_os = "windows")]
        let mut input = {
            let target = TargetDisplay {
                x: self.config.display.desktop_x,
                y: self.config.display.desktop_y,
                width: self.config.display.pixel_width,
                height: self.config.display.pixel_height,
            };
            WindowsInputInjector::new(Some(target)).map_err(SessionError::Io)?
        };
        #[cfg(all(target_os = "linux", not(test)))]
        let mut input =
            LinuxInputInjector::new(crate::inject_linux::OutputGeometry::single_output(
                self.config.display.pixel_width,
                self.config.display.pixel_height,
            ))
            .map_err(SessionError::Io)?;
        #[cfg(all(target_os = "linux", test))]
        let mut input = tests::admission_input(self)?;
        // Establish the diagnostic clock before any session-local timestamps.
        let _trace = host_trace::enabled();
        let session_origin = Instant::now();
        info!(identity = negotiated_identity, peer = %tcp_peer, "TLS-PSK session established");
        let mut last_pong = Instant::now();
        let mut next_ping = Instant::now() + HEARTBEAT_INTERVAL;
        let mut stop_sender_tx = None;
        let mut sender_thread = None;
        let mut clipboard_sync = false;
        #[cfg(target_os = "windows")]
        let mut clipboard = Some(crate::WindowsClipboard::new());
        #[cfg(target_os = "linux")]
        let mut clipboard = crate::clipboard_linux::LinuxClipboard::new().ok();
        #[cfg(target_os = "linux")]
        let mut next_clipboard_poll = Instant::now();

        let result = (|| -> Result<(), SessionError> {
            // If authenticated and media_receiver is set, we run UDP sending in a dedicated thread to avoid TCP blocking it.
            loop {
                if state != SessionState::Authenticated {
                    let remaining = admission_deadline
                        .checked_duration_since(Instant::now())
                        .filter(|remaining| !remaining.is_zero())
                        .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))?;
                    stream
                        .ssl_stream()
                        .get_ref()
                        .set_write_timeout(Some(remaining))?;
                }
                if state == SessionState::Authenticated {
                    self.discover_udp_peer(tcp_peer, &mut udp_peer, c2h_cipher.as_mut())?;
                    if sender_thread.is_none() {
                        if let (Some(_), Some(peer)) = (&media_receiver, udp_peer) {
                            let receiver = media_receiver.take().unwrap();
                            let udp_socket = self.udp_socket.try_clone()?;
                            let mut cipher = h2c_cipher.take().ok_or(SessionError::PreAuth)?;
                            let pixel_width = self.config.display.pixel_width;
                            let pixel_height = self.config.display.pixel_height;
                            let (stx, srx) = mpsc::channel();
                            stop_sender_tx = Some(stx);
                            #[cfg(test)]
                            let media_source = self.media_source.clone();
                            let handle = thread::spawn(move || {
                                let mut sender = UdpSender::default();
                                sender.trace = host_trace::enabled().cloned();
                                sender.trace_session =
                                    sender.trace.as_ref().map_or(0, |t| t.time(session_origin));
                                info!(%peer, "Starting UDP sender thread");
                                loop {
                                    if !matches!(srx.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                                        info!("UDP sender received stop signal");
                                        break;
                                    }
                                    match receiver.recv_timeout(Duration::from_millis(5)) {
                                        Ok(MediaEvent::Video(frame)) => {
                                            let size = frame.data.len();
                                            let is_key = frame.is_key_frame;
                                            if let Err(err) = sender.send_frame(
                                                &udp_socket,
                                                peer,
                                                &mut cipher,
                                                pixel_width,
                                                pixel_height,
                                                frame,
                                                session_origin,
                                            ) {
                                                warn!(%err, "Failed to send video frame over UDP");
                                            } else {
                                                debug!(size, is_key, %peer, "Successfully sent video frame over UDP");
                                            }
                                        }
                                        Ok(MediaEvent::Audio(bytes)) => {
                                            let _ = sender.send_audio(
                                                &udp_socket,
                                                peer,
                                                &mut cipher,
                                                &bytes,
                                            );
                                        }
                                        #[cfg(target_os = "linux")]
                                        Ok(MediaEvent::NativeAudio(slot)) => {
                                            if let Some(block) = slot.take_fresh(Instant::now()) {
                                                if let Err(error) = sender.send_audio(
                                                    &udp_socket,
                                                    peer,
                                                    &mut cipher,
                                                    &block.pcm,
                                                ) {
                                                    warn!(%error, "Failed to send native audio over UDP");
                                                }
                                            }
                                        }
                                        Ok(MediaEvent::Cursor(cursor)) => {
                                            let _ = sender.send_cursor(
                                                &udp_socket,
                                                peer,
                                                &mut cipher,
                                                &cursor,
                                            );
                                        }
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
                        }
                    }
                    let now = Instant::now();
                    if now >= next_ping {
                        send_tcp_control(&mut stream, ControlMessage::Ping)?;
                        next_ping = now + HEARTBEAT_INTERVAL;
                    }
                    if now.duration_since(last_pong) >= HEARTBEAT_TIMEOUT {
                        warn!(peer = %tcp_peer, "heartbeat timeout");
                        break;
                    }
                    #[cfg(any(target_os = "windows", target_os = "linux"))]
                    if clipboard_sync {
                        // Linux reads the pasteboard through a subprocess, so it
                        // is gated to the protocol's 500 ms cadence; the Win32
                        // poll is a single syscall and runs every iteration.
                        #[cfg(target_os = "linux")]
                        let clipboard_due = now >= next_clipboard_poll;
                        #[cfg(target_os = "linux")]
                        if clipboard_due {
                            next_clipboard_poll =
                                now + crate::clipboard_linux::DEFAULT_POLL_INTERVAL;
                        }
                        #[cfg(not(target_os = "linux"))]
                        let clipboard_due = true;
                        if clipboard_due {
                            if let Some(clipboard) = clipboard.as_mut() {
                                let polled = {
                                    #[cfg(target_os = "windows")]
                                    {
                                        clipboard.poll()
                                    }
                                    #[cfg(target_os = "linux")]
                                    {
                                        clipboard.poll(now)
                                    }
                                };
                                match polled {
                                    Ok(Some(text)) => {
                                        debug!(bytes = text.len(), "clipboard change detected");
                                        send_tcp_control(
                                            &mut stream,
                                            ControlMessage::ClipboardSyncUpdate(
                                                ClipboardSyncUpdate {
                                                    request_id: 0,
                                                    direction: ClipboardSyncDirection::HostToClient,
                                                    origin: ClipboardSyncOrigin::LocalPasteboard,
                                                    text,
                                                },
                                            ),
                                        )?;
                                    }
                                    Ok(None) => {}
                                    Err(error) => {
                                        debug!(%error, "clipboard poll failed");
                                    }
                                }
                            }
                        }
                    }
                }

                let read = if state == SessionState::Authenticated {
                    stream.read_frame_step()
                } else {
                    stream.read_frame_until(admission_deadline)
                };
                let packet = match read {
                    Ok(Some(packet)) => packet,
                    Ok(None) => continue,
                    Err(TlsPskError::Io(error))
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue
                    }
                    Err(TlsPskError::Io(error))
                        if matches!(
                            error.kind(),
                            io::ErrorKind::UnexpectedEof
                                | io::ErrorKind::ConnectionReset
                                | io::ErrorKind::BrokenPipe
                        ) || error
                            .get_ref()
                            .and_then(|cause| cause.downcast_ref::<openssl::ssl::Error>())
                            .is_some_and(|ssl_err| {
                                ssl_err.code() == openssl::ssl::ErrorCode::SYSCALL
                            }) =>
                    {
                        warn!(%error, "read_frame saw EOF/reset/broken pipe, closing connection");
                        break;
                    }
                    Err(error) => {
                        warn!(%error, "read_frame failed with error, terminating connection");
                        return Err(SessionError::Tls(error));
                    }
                };
                let (header, payload) = split_packet(&packet)?;
                if !state.allows(header.packet_type) {
                    debug!(?state, ?header.packet_type, "refusing packet for current state");
                    if state == SessionState::Authenticated
                        && header.packet_type == PacketType::Handshake
                    {
                        return Err(SessionError::AlreadyAuthenticated);
                    }
                    continue;
                }

                match header.packet_type {
                    PacketType::PairingRequest => {
                        if negotiated_identity != BOOTSTRAP_IDENTITY || !self.is_pairing_active() {
                            send_pairing_reject(&mut stream, PairingRejectReason::PairingDisabled)?;
                            continue;
                        }
                        let request = PairingRequest::decode(payload)?;
                        let Some(consent_sender) = &self.config.consent_sender else {
                            send_pairing_reject(&mut stream, PairingRejectReason::PairingDisabled)?;
                            continue;
                        };
                        let (response_tx, response_rx) = mpsc::sync_channel(1);
                        consent_sender
                            .send(ConsentPrompt {
                                client_name: request.name.clone(),
                                response: response_tx,
                            })
                            .map_err(|_| SessionError::ConsentUnavailable)?;
                        let approved = response_rx
                            .recv_timeout(
                                admission_deadline
                                    .min(
                                        self.pairing_deadline
                                            .ok_or(SessionError::ConsentTimeout)?,
                                    )
                                    .saturating_duration_since(Instant::now()),
                            )
                            .map_err(|_| SessionError::ConsentTimeout)?;
                        if Instant::now() >= admission_deadline || !self.is_pairing_active() {
                            return Err(SessionError::ConsentTimeout);
                        }
                        if !approved {
                            send_pairing_reject(&mut stream, PairingRejectReason::DeniedByHost)?;
                            break;
                        }
                        let record = PairingRecord {
                            id: Uuid::new_v4().to_string().to_uppercase(),
                            name: request.name,
                            key: random_key(),
                            added_at_unix_ms: unix_ms_u64(),
                        };
                        self.config.pairing_store.save(record.clone())?;
                        let grant = PairingGrant {
                            pairing_id: record.id.clone(),
                            host_name: self.config.host_name.clone(),
                            key: record.key,
                        };
                        send_tcp_packet(&mut stream, PacketType::PairingGrant, &grant.encode()?)?;
                        granted_pairing_id = Some(record.id);
                        state = SessionState::PairingGranted;
                    }
                    PacketType::Handshake => {
                        if state == SessionState::Authenticated {
                            return Err(SessionError::AlreadyAuthenticated);
                        }
                        let handshake = Handshake::decode(payload)?;
                        let record = self
                            .config
                            .pairing_store
                            .load(&handshake.pairing_id)?
                            .ok_or(SessionError::UnknownPairing)?;
                        if let Some(identity_pairing_id) =
                            negotiated_identity.strip_prefix(PAIRING_IDENTITY_PREFIX)
                        {
                            if identity_pairing_id != handshake.pairing_id {
                                return Err(SessionError::IdentityMismatch);
                            }
                        } else if negotiated_identity == BOOTSTRAP_IDENTITY {
                            let Some(expected_id) = &granted_pairing_id else {
                                return Err(SessionError::PreAuth);
                            };
                            if *expected_id != handshake.pairing_id {
                                return Err(SessionError::IdentityMismatch);
                            }
                        } else {
                            return Err(SessionError::IdentityMismatch);
                        }
                        if handshake.version != PROTOCOL_VERSION {
                            return Err(SessionError::Codec(
                                maho_proto::CodecError::UnsupportedVersion(handshake.version),
                            ));
                        }
                        clipboard_sync = handshake
                            .capabilities
                            .contains(Capabilities::TEXT_CLIPBOARD_SYNC);
                        if !handshake
                            .capabilities
                            .contains(Capabilities::AUTHENTICATED_UDP_REGISTRATION)
                        {
                            return Err(SessionError::MissingAuthenticatedRegistration);
                        }
                        debug!(
                            clipboard_sync,
                            bits = handshake.capabilities.bits(),
                            "capabilities negotiated"
                        );
                        c2h_cipher = Some(DatagramCipher::derive(
                            &record.key,
                            &handshake.session_salt,
                            Direction::ClientToHost,
                        )?);
                        h2c_cipher = Some(DatagramCipher::derive(
                            &record.key,
                            &handshake.session_salt,
                            Direction::HostToClient,
                        )?);
                        state = SessionState::Authenticated;
                        stream
                            .ssl_stream()
                            .get_ref()
                            .set_write_timeout(Some(Duration::from_secs(5)))?;
                        let ack = Handshake {
                            name: self.config.host_name.clone(),
                            width: self.config.display.logical_width.min(u16::MAX as u32) as u16,
                            height: self.config.display.logical_height.min(u16::MAX as u32) as u16,
                            scale: self.config.display.scale_factor(),
                            version: PROTOCOL_VERSION,
                            capabilities: Capabilities::STREAM_CONFIGURATION
                                | Capabilities::AUTHENTICATED_UDP_REGISTRATION,
                            pairing_id: String::new(),
                            session_salt: [0_u8; 16],
                        };
                        let (media_tx, media_rx) = mpsc::sync_channel(16);
                        media_handle = Some(self.media_source.start(media_tx)?);
                        media_receiver = Some(media_rx);
                        send_tcp_packet(&mut stream, PacketType::HandshakeAck, &ack.encode()?)?;
                        last_pong = Instant::now();
                        next_ping = Instant::now() + HEARTBEAT_INTERVAL;
                        info!(
                            client = handshake.name,
                            "v3 handshake authenticated; UDP ciphers armed"
                        );
                    }
                    PacketType::InputEvent => {
                        if state != SessionState::Authenticated {
                            continue;
                        }
                        let event = InputEvent::decode(payload)?;
                        let success = inject_input(&event, |event| input.inject(event));
                        let ack = InputAckMessage {
                            sequence: header.sequence,
                            success,
                            error_code: if success { 0 } else { 1 },
                        };
                        if let Err(error) =
                            send_tcp_control(&mut stream, ControlMessage::InputAck(ack))
                        {
                            warn!(%error, "failed to send input ACK");
                        }
                    }
                    PacketType::Control => {
                        if state != SessionState::Authenticated {
                            continue;
                        }
                        match ControlMessage::decode(payload)? {
                            ControlMessage::RequestKeyFrame => {
                                if let Some(media) = &media_handle {
                                    media.force_key_frame()?;
                                }
                            }
                            ControlMessage::BitrateAdjust(BitrateAdjust { target_bitrate })
                                if target_bitrate > 0 =>
                            {
                                if let Some(media) = &media_handle {
                                    if let Err(error) = media.update_bitrate(target_bitrate as u32)
                                    {
                                        // A rejected quality change must not end the
                                        // session; the stream stays at its old bitrate.
                                        warn!(%error, target_bitrate, "bitrate adjust failed");
                                    }
                                }
                            }
                            ControlMessage::StreamConfigRequest(req) => {
                                let reject_reason = if req.desired.width == 0
                                    || req.desired.height == 0
                                    || req.desired.width > 7680
                                    || req.desired.height > 4320
                                {
                                    Some((
                                        StreamConfigurationErrorCode::UnsupportedDimensions,
                                        "unsupported dimensions",
                                    ))
                                } else if req.desired.frames_per_second == 0
                                    || req.desired.frames_per_second > 240
                                {
                                    Some((
                                        StreamConfigurationErrorCode::UnsupportedFps,
                                        "unsupported fps",
                                    ))
                                } else if req.desired.bitrate < 100_000
                                    || req.desired.bitrate > 300_000_000
                                {
                                    Some((
                                        StreamConfigurationErrorCode::UnsupportedBitrate,
                                        "unsupported bitrate",
                                    ))
                                } else {
                                    None
                                };
                                match reject_reason {
                                    Some((reason, message)) => {
                                        send_tcp_control(
                                            &mut stream,
                                            ControlMessage::StreamConfigReject(
                                                StreamConfigurationReject {
                                                    request_id: req.request_id,
                                                    reason,
                                                    message: message.to_owned(),
                                                },
                                            ),
                                        )?;
                                    }
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
                                }
                            }
                            ControlMessage::Ping => {
                                send_tcp_control(&mut stream, ControlMessage::Pong)?;
                            }
                            ControlMessage::Pong => last_pong = Instant::now(),
                            ControlMessage::ClipboardSyncUpdate(update) => {
                                #[cfg(any(target_os = "windows", target_os = "linux"))]
                                if clipboard_sync
                                    && matches!(
                                        update.direction,
                                        ClipboardSyncDirection::ClientToHost
                                            | ClipboardSyncDirection::Bidirectional
                                    )
                                {
                                    if let Some(clipboard) = clipboard.as_mut() {
                                        let applied = {
                                            #[cfg(target_os = "windows")]
                                            {
                                                clipboard.apply_remote_text(&update.text)
                                            }
                                            #[cfg(target_os = "linux")]
                                            {
                                                clipboard
                                                    .apply_remote_text(&update.text, Instant::now())
                                            }
                                        };
                                        if let Err(error) = applied {
                                            warn!(%error, "failed to apply remote clipboard text");
                                        }
                                    }
                                }
                                #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                                let _ = update;
                            }
                            ControlMessage::Disconnect | ControlMessage::StopStream => break,
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }

            Ok(())
        })();

        // Release blocked output sends before stopping or joining their producers.
        drop(media_receiver);
        if let Some(tx) = stop_sender_tx {
            let _ = tx.send(());
        }
        if let Some(handle) = sender_thread {
            if handle.join().is_err() {
                warn!("UDP sender thread panicked");
            }
        }
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
    }

    fn discover_udp_peer(
        &self,
        tcp_peer: SocketAddr,
        udp_peer: &mut Option<SocketAddr>,
        receive_cipher: Option<&mut DatagramCipher>,
    ) -> Result<(), SessionError> {
        let Some(cipher) = receive_cipher else {
            return Ok(());
        };
        let mut buffer = [0_u8; 2_048];
        for _ in 0..MAX_UDP_DISCOVERY_BURST {
            match self.udp_socket.recv_from(&mut buffer) {
                Ok((length, peer)) if peer.ip() == tcp_peer.ip() => {
                    if udp_peer.is_some() {
                        continue;
                    }
                    if length < 40 {
                        continue;
                    }
                    match cipher.open_datagram(&buffer[..length]) {
                        Ok((header, payload)) => {
                            if header.packet_type == PacketType::Ping && payload.is_empty() {
                                *udp_peer = Some(peer);
                                return Ok(());
                            }
                        }
                        Err(_) => {
                            continue;
                        }
                    }
                }
                Ok(_) => continue,
                Err(error)
                    if error.kind() == io::ErrorKind::ConnectionReset
                        || error.raw_os_error() == Some(10054) =>
                {
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(SessionError::Io(error)),
            }
        }
        Ok(())
    }
}

/// Maximum number of UDP registration datagrams processed per discovery step
/// before yielding back to the outer session loop to prevent TCP control starvation.
pub const MAX_UDP_DISCOVERY_BURST: usize = 32;

/// Explicit conservative UDP payload budget ensuring datagrams never exceed
/// the 1280-byte MTU standard for IPv6 and Tailscale / WireGuard virtual interfaces.
///
/// Budget calculation:
/// ```text
///   1280 bytes MTU
/// -   40 bytes IPv6 header (or 20 bytes IPv4)
/// -    8 bytes UDP header
/// = 1232 bytes maximum UDP payload under IPv6
/// ```
///
/// To provide safe headroom for IP options and encapsulation headers,
/// MahoRD targets a conservative 1200-byte UDP payload budget.
pub const UDP_PAYLOAD_BUDGET: usize = 1200;

/// Encrypted envelope overhead on the wire:
/// PacketHeader::SIZE (12 B) + udp_gcm::NONCE_SIZE (12 B) + udp_gcm::TAG_SIZE (16 B) = 40 bytes.
pub const ENCRYPTED_DATAGRAM_OVERHEAD: usize =
    PacketHeader::SIZE + maho_net::udp_gcm::NONCE_SIZE + maho_net::udp_gcm::TAG_SIZE;

/// Maximum plaintext payload per datagram within the conservative UDP payload budget:
/// 1200 - 40 = 1160 bytes.
pub const MAX_SENDER_PLAINTEXT_PAYLOAD: usize = UDP_PAYLOAD_BUDGET - ENCRYPTED_DATAGRAM_OVERHEAD;

/// MTU-safe sender video chunk size (1154 bytes).
/// Plaintext video chunk payload = FrameChunk::HEADER_SIZE (6 B) + data bytes.
/// Total datagram = 40 (envelope) + 6 (header) + 1154 (data) = 1200 bytes <= UDP_PAYLOAD_BUDGET.
/// Note: Wire protocol accepts up to MAX_VIDEO_CHUNK_BYTES (1382 B) on the receiver side.
pub const SENDER_MAX_VIDEO_CHUNK_BYTES: usize =
    MAX_SENDER_PLAINTEXT_PAYLOAD - maho_proto::FrameChunk::HEADER_SIZE;

/// MTU-safe sender audio fragment size (1152 bytes).
/// Plaintext audio fragment payload = AudioFragmentHeader::SIZE (8 B) + data bytes.
/// Total datagram = 40 (envelope) + 8 (header) + 1152 (data) = 1200 bytes <= UDP_PAYLOAD_BUDGET.
/// Note: Wire protocol accepts up to MAX_AUDIO_FRAGMENT_BYTES (1380 B) on the receiver side.
pub const SENDER_MAX_AUDIO_FRAGMENT_BYTES: usize =
    MAX_SENDER_PLAINTEXT_PAYLOAD - AudioFragmentHeader::SIZE;

const _: () = assert!(SENDER_MAX_VIDEO_CHUNK_BYTES <= MAX_VIDEO_CHUNK_BYTES);
const _: () = assert!(SENDER_MAX_AUDIO_FRAGMENT_BYTES <= MAX_AUDIO_FRAGMENT_BYTES);
const _: () = assert!(
    ENCRYPTED_DATAGRAM_OVERHEAD
        + maho_proto::FrameChunk::HEADER_SIZE
        + SENDER_MAX_VIDEO_CHUNK_BYTES
        <= UDP_PAYLOAD_BUDGET
);
const _: () = assert!(
    ENCRYPTED_DATAGRAM_OVERHEAD + AudioFragmentHeader::SIZE + SENDER_MAX_AUDIO_FRAGMENT_BYTES
        <= UDP_PAYLOAD_BUDGET
);

#[derive(Default)]
struct UdpSender {
    trace: Option<Arc<host_trace::Trace>>,
    trace_session: u64,
    trace_keyframe: bool,
    sequence: u32,
    frame_id: u32,
    audio_frame_id: u32,
    payload: Vec<u8>,
    datagram: Vec<u8>,
}

impl UdpSender {
    fn send_packet(
        &mut self,
        socket: &UdpSocket,
        peer: SocketAddr,
        cipher: &mut DatagramCipher,
        packet_type: PacketType,
        payload: &[u8],
    ) -> Result<(), SessionError> {
        self.send_selected_packet(cipher, packet_type, payload, |bytes| {
            socket.send_to(bytes, peer)
        })
    }

    fn send_selected_packet(
        &mut self,
        cipher: &mut DatagramCipher,
        packet_type: PacketType,
        payload: &[u8],
        send: impl FnOnce(&[u8]) -> io::Result<usize>,
    ) -> Result<(), SessionError> {
        self.sequence = self.sequence.wrapping_add(1);
        let header = PacketHeader::new(packet_type, self.sequence, unix_ms_u32(), 0);
        let header = header.to_bytes();
        self.datagram.clear();
        self.datagram.extend_from_slice(&header);
        cipher.seal_into(payload, &header, &mut self.datagram)?;
        let observation = self.trace.as_ref().map(|trace| {
            let frame = match packet_type {
                PacketType::FrameHeader | PacketType::FrameChunk | PacketType::AudioFrame => {
                    payload
                        .get(..4)
                        .map(|bytes| {
                            u64::from(u32::from_le_bytes(bytes.try_into().expect("four bytes")))
                        })
                        .unwrap_or(0)
                }
                PacketType::Ping => {
                    TimestampStats::decode(payload).map_or(0, |stats| u64::from(stats.frame_id))
                }
                _ => 0,
            };
            let record = host_trace::Record {
                session: self.trace_session,
                sequence: self.sequence,
                kind: packet_type as u8,
                size: self.datagram.len(),
                frame,
                event: 1,
                keyframe: self.trace_keyframe
                    && matches!(
                        packet_type,
                        PacketType::FrameHeader | PacketType::FrameChunk | PacketType::Ping
                    ),
                ..Default::default()
            };
            trace.record(record);
            record
        });
        let result = send(&self.datagram);
        if let (Some(trace), Some(mut record)) = (&self.trace, observation) {
            match &result {
                Ok(size) => {
                    record.event = 2;
                    record.value = u64::try_from(*size).unwrap_or(u64::MAX);
                }
                Err(error) => {
                    record.event = 3;
                    record.value = error
                        .raw_os_error()
                        .map_or(0, |code| u64::from(code.unsigned_abs()));
                }
            }
            trace.record(record);
        }
        result?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn send_frame(
        &mut self,
        socket: &UdpSocket,
        peer: SocketAddr,
        cipher: &mut DatagramCipher,
        width: u32,
        height: u32,
        frame: VideoFrame,
        session_origin: Instant,
    ) -> Result<(), SessionError> {
        let chunk_count = frame.data.len().div_ceil(SENDER_MAX_VIDEO_CHUNK_BYTES);
        let frame_id = self.frame_id.wrapping_add(1);
        let header = FrameHeader {
            frame_id,
            width: width.min(u16::MAX as u32) as u16,
            height: height.min(u16::MAX as u32) as u16,
            is_key_frame: frame.is_key_frame,
            total_chunks: u16::try_from(chunk_count)
                .map_err(|_| SessionError::Store("encoded frame has too many chunks".into()))?,
            total_size: u32::try_from(frame.data.len())
                .map_err(|_| SessionError::Store("encoded frame is too large".into()))?,
        };
        let encoded_header = header.encode()?;
        self.frame_id = frame_id;
        self.trace_keyframe = frame.is_key_frame;
        if let Some(trace) = &self.trace {
            trace.record(host_trace::Record {
                session: self.trace_session,
                event: 20,
                frame: u64::from(frame_id),
                value: trace.time(frame.capture_at),
                keyframe: frame.is_key_frame,
                ..Default::default()
            });
        }
        self.send_packet(
            socket,
            peer,
            cipher,
            PacketType::FrameHeader,
            &encoded_header,
        )?;
        for (index, bytes) in frame.data.chunks(SENDER_MAX_VIDEO_CHUNK_BYTES).enumerate() {
            // Spread keyframe-sized bursts (100+ datagrams at line rate lose
            // mid-frame chunks on WiFi/relay paths, stranding assembly until
            // the next keyframe). Only large frames are paced, so the 60 fps
            // small-P-frame budget stays untouched.
            if chunk_count > 16 && index > 0 && index % 8 == 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
            let mut payload = std::mem::take(&mut self.payload);
            payload.clear();
            payload.extend_from_slice(&frame_id.to_le_bytes());
            payload.extend_from_slice(&(index as u16).to_le_bytes());
            payload.extend_from_slice(bytes);
            let sent = self.send_packet(socket, peer, cipher, PacketType::FrameChunk, &payload);
            self.payload = payload;
            sent?;
        }
        let send_at = Instant::now();
        let stats = TimestampStats {
            frame_id,
            capture_us: monotonic_us(session_origin, frame.capture_at),
            encode_start_us: monotonic_us(session_origin, frame.encode_started_at),
            encode_end_us: monotonic_us(session_origin, frame.encode_completed_at),
            send_us: monotonic_us(session_origin, send_at),
        };
        self.send_packet(socket, peer, cipher, PacketType::Ping, &stats.encode())?;
        Ok(())
    }

    fn send_cursor(
        &mut self,
        socket: &UdpSocket,
        peer: SocketAddr,
        cipher: &mut DatagramCipher,
        cursor: &CursorUpdate,
    ) -> Result<(), SessionError> {
        let payload = cursor.encode()?;
        self.send_packet(socket, peer, cipher, PacketType::CursorUpdate, &payload)
    }

    fn send_audio(
        &mut self,
        socket: &UdpSocket,
        peer: SocketAddr,
        cipher: &mut DatagramCipher,
        bytes: &[u8],
    ) -> Result<(), SessionError> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.audio_frame_id = self.audio_frame_id.wrapping_add(1);
        let fragment_count = bytes.len().div_ceil(SENDER_MAX_AUDIO_FRAGMENT_BYTES);
        let fragment_count = u16::try_from(fragment_count)
            .map_err(|_| SessionError::Store("audio frame has too many fragments".into()))?;
        for (index, data) in bytes.chunks(SENDER_MAX_AUDIO_FRAGMENT_BYTES).enumerate() {
            let header = AudioFragmentHeader {
                frame_id: self.audio_frame_id,
                fragment_index: index as u16,
                fragment_count,
            }
            .to_bytes()?;
            let mut payload = std::mem::take(&mut self.payload);
            payload.clear();
            payload.extend_from_slice(&header);
            payload.extend_from_slice(data);
            let sent = self.send_packet(socket, peer, cipher, PacketType::AudioFrame, &payload);
            self.payload = payload;
            sent?;
        }
        Ok(())
    }
}

fn inject_input<E: std::fmt::Display>(
    event: &InputEvent,
    inject: impl FnOnce(&InputEvent) -> Result<(), E>,
) -> bool {
    tracing::trace!(?event, "Host received input event");
    match inject(event) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, "input event was not injected");
            false
        }
    }
}

fn split_packet(packet: &[u8]) -> Result<(PacketHeader, &[u8]), SessionError> {
    if packet.len() < PacketHeader::SIZE {
        return Err(SessionError::Codec(maho_proto::CodecError::Truncated {
            field: "packet header",
            needed: PacketHeader::SIZE,
            remaining: packet.len(),
        }));
    }
    let header = PacketHeader::decode(&packet[..PacketHeader::SIZE])?;
    Ok((header, &packet[PacketHeader::SIZE..]))
}

fn send_tcp_packet(
    stream: &mut maho_net::TlsPskStream<TcpStream>,
    packet_type: PacketType,
    payload: &[u8],
) -> Result<(), SessionError> {
    let mut packet = PacketHeader::new(packet_type, 0, unix_ms_u32(), 0).encode()?;
    packet.extend_from_slice(payload);
    stream.write_frame(&packet)?;
    Ok(())
}

fn send_tcp_control(
    stream: &mut maho_net::TlsPskStream<TcpStream>,
    message: ControlMessage,
) -> Result<(), SessionError> {
    send_tcp_packet(stream, PacketType::Control, &message.encode()?)
}

fn send_pairing_reject(
    stream: &mut maho_net::TlsPskStream<TcpStream>,
    reason: PairingRejectReason,
) -> Result<(), SessionError> {
    send_tcp_packet(
        stream,
        PacketType::PairingReject,
        &PairingReject { reason }.encode()?,
    )
}

fn random_key() -> [u8; 32] {
    let mut key = [0_u8; 32];
    rand::rng().fill_bytes(&mut key);
    key
}

fn unix_ms_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn unix_ms_u32() -> u32 {
    unix_ms_u64() as u32
}

fn monotonic_us(origin: Instant, value: Instant) -> u64 {
    value
        .checked_duration_since(origin)
        .unwrap_or_default()
        .as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use maho_proto::{AudioFragment, FrameChunk};
    mod sender_trace {
        include!("sender_trace_tests.rs");
    }
    mod sender_packetization {
        include!("sender_packetization_tests.rs");
    }

    fn peek_datagram(socket: &UdpSocket, buffer: &mut [u8]) -> (usize, SocketAddr) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match socket.peek_from(buffer) {
                Ok(result) => return result,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "datagram did not arrive within 2s");
                    thread::yield_now();
                }
                Err(error) => panic!("peek_from failed: {error}"),
            }
        }
    }

    #[test]
    fn capture_failure_labels_preserve_secure_desktop_recovery() {
        use crate::windows_session::{capture_recovery, CaptureFailure, CaptureRecovery};

        for (label, expected) in [
            ("access_lost", CaptureFailure::AccessLost),
            ("access_denied", CaptureFailure::AccessDenied),
            ("refresh_failure", CaptureFailure::RefreshFailure),
            ("capture", CaptureFailure::Other),
            ("", CaptureFailure::Other),
            ("ACCESS_DENIED", CaptureFailure::Other),
        ] {
            assert_eq!(capture_failure_from_label(label), expected);
        }
        for consecutive in [0, 10_000, u32::MAX] {
            assert!(matches!(
                capture_recovery(capture_failure_from_label("access_denied"), consecutive),
                CaptureRecovery::Reacquire(_)
            ));
        }
    }

    #[test]
    fn service_pairing_store_uses_machine_wide_path() {
        let program_data = "C:/ProgramData";
        let path = PairingStore::service_default_path(program_data);
        assert_eq!(
            path,
            crate::windows_session::service_store_path(program_data)
        );
        assert_eq!(
            path,
            PathBuf::from(program_data)
                .join("MahoRD")
                .join("host-authorizations.json")
        );
    }

    #[test]
    fn bind_synthetic_does_not_start_mdns_advertisement() {
        let directory = tempdir().unwrap();
        let (consent, _) = mpsc::channel();
        let mut config = test_config(
            PairingStore::new(directory.path().join("keys.json")),
            consent,
        );
        config.tcp_addr = "127.0.0.1:0".parse().unwrap();
        config.udp_addr = "127.0.0.1:0".parse().unwrap();
        let server = HostServer::bind_synthetic(config, 1).unwrap();
        assert!(!server.is_advertising());
    }

    #[test]
    fn bind_with_advertising_fallback_keeps_host_usable() {
        let directory = tempdir().unwrap();
        let (consent, _) = mpsc::channel();
        let mut config = test_config(
            PairingStore::new(directory.path().join("keys.json")),
            consent,
        );
        config.tcp_addr = "127.0.0.1:0".parse().unwrap();
        config.udp_addr = "127.0.0.1:0".parse().unwrap();
        let source = Arc::new(SyntheticMediaSource { frame_count: 1 });
        let server = HostServer::bind_with_media_advertising(config, source, true).unwrap();
        assert!(server.tcp_addr().is_ok());
        assert!(server.udp_addr().is_ok());
    }

    #[test]
    fn sender_reuses_video_packet_buffers() {
        let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let peer = rx.local_addr().unwrap();
        let mut cipher =
            DatagramCipher::derive(&[0x31; 32], &[0x72; 16], Direction::HostToClient).unwrap();
        let mut receiver =
            DatagramCipher::derive(&[0x31; 32], &[0x72; 16], Direction::HostToClient).unwrap();
        let mut sender = UdpSender::default();
        let expected: Vec<_> = (0..MAX_VIDEO_CHUNK_BYTES + 7)
            .map(|index| (index % 251) as u8)
            .collect();
        let now = Instant::now();
        let frame = || VideoFrame {
            data: expected.clone(),
            is_key_frame: true,
            capture_at: now,
            encode_started_at: now,
            encode_completed_at: now,
        };
        sender
            .send_frame(&tx, peer, &mut cipher, 320, 180, frame(), now)
            .unwrap();
        let mut packet = [0; 65536];
        for _ in 0..4 {
            rx.recv(&mut packet).unwrap();
        }
        let input = frame();
        let (result, count) = crate::test_alloc::allocations(|| {
            sender.send_frame(&tx, peer, &mut cipher, 320, 180, input, now)
        });
        result.unwrap();
        let mut chunks = std::collections::BTreeMap::new();
        let mut sequences = std::collections::BTreeSet::new();
        let mut saw_header = false;
        let mut saw_timing = false;
        for _ in 0..4 {
            let size = rx.recv(&mut packet).unwrap();
            let (header, payload) = receiver.open_datagram(&packet[..size]).unwrap();
            sequences.insert(header.sequence);
            match header.packet_type {
                PacketType::FrameHeader => {
                    let frame = FrameHeader::decode(&payload).unwrap();
                    assert_eq!((frame.frame_id, frame.width, frame.height), (2, 320, 180));
                    assert_eq!(frame.total_chunks, 2);
                    assert_eq!(frame.total_size as usize, expected.len());
                    saw_header = true;
                }
                PacketType::FrameChunk => {
                    let chunk = FrameChunk::decode(&payload).unwrap();
                    assert_eq!(chunk.frame_id, 2);
                    chunks.insert(chunk.chunk_index, chunk.data);
                }
                PacketType::Ping => {
                    assert!(!payload.is_empty());
                    saw_timing = true;
                }
                other => panic!("unexpected packet type {other:?}"),
            }
        }
        assert!(saw_header && saw_timing);
        assert_eq!(sequences.into_iter().collect::<Vec<_>>(), [5, 6, 7, 8]);
        assert_eq!(chunks.into_values().flatten().collect::<Vec<_>>(), expected);
        eprintln!("warm two-chunk video sender allocations: {count}");
        assert!(count <= 2, "fragment and datagram buffers must be reused");
    }

    #[test]
    fn sender_reuses_audio_packet_buffers() {
        let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let peer = rx.local_addr().unwrap();
        let mut cipher =
            DatagramCipher::derive(&[0x41; 32], &[0x82; 16], Direction::HostToClient).unwrap();
        let mut receiver =
            DatagramCipher::derive(&[0x41; 32], &[0x82; 16], Direction::HostToClient).unwrap();
        let mut sender = UdpSender::default();
        let expected = vec![0x29; MAX_AUDIO_FRAGMENT_BYTES + 9];
        sender
            .send_audio(&tx, peer, &mut cipher, &expected)
            .unwrap();
        let mut packet = [0; 65536];
        for _ in 0..2 {
            rx.recv(&mut packet).unwrap();
        }
        let (result, count) =
            crate::test_alloc::allocations(|| sender.send_audio(&tx, peer, &mut cipher, &expected));
        result.unwrap();
        let mut chunks = std::collections::BTreeMap::new();
        for _ in 0..2 {
            let size = rx.recv(&mut packet).unwrap();
            let (header, payload) = receiver.open_datagram(&packet[..size]).unwrap();
            assert_eq!(header.packet_type, PacketType::AudioFrame);
            let fragment = AudioFragment::decode(&payload).unwrap();
            assert_eq!(fragment.header.frame_id, 2);
            assert_eq!(fragment.header.fragment_count, 2);
            chunks.insert(fragment.header.fragment_index, fragment.data);
        }
        assert_eq!(chunks.into_values().flatten().collect::<Vec<_>>(), expected);
        let sequence = sender.sequence;
        sender.send_audio(&tx, peer, &mut cipher, &[]).unwrap();
        assert_eq!(sender.sequence, sequence);
        assert_eq!(sender.audio_frame_id, 2);
        eprintln!("warm two-fragment audio sender allocations: {count}");
        assert_eq!(
            count, 0,
            "audio fragment and datagram buffers must be reused"
        );
    }

    #[test]
    fn bootstrap_kdf_runs_once_per_unchanged_pairing_window() {
        let directory = tempdir().unwrap();
        let (consent, _) = mpsc::channel();
        let config = test_config(
            PairingStore::new(directory.path().join("keys.json")),
            consent,
        );
        BOOTSTRAP_DERIVATIONS.with(|count| count.set(0));
        let started = Instant::now();
        let server = HostServer::bind_synthetic(config, 0).unwrap();
        let first = server.current_psks().unwrap();
        for _ in 1..8 {
            assert_eq!(server.current_psks().unwrap(), first);
        }
        let derivations = BOOTSTRAP_DERIVATIONS.with(|count| count.get());
        eprintln!("unchanged pairing window: {derivations} PBKDF2 derivations for 8 accepts, elapsed {:?}", started.elapsed());
        assert_eq!(
            derivations, 1,
            "unchanged PIN repeats 600000-round PBKDF2 per accept"
        );
    }

    #[test]
    fn cached_bootstrap_obeys_lockout_expiry_and_pairing_revocation() {
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("keys.json"));
        store
            .save(PairingRecord {
                id: "stored".into(),
                name: "stored".into(),
                key: [3; 32],
                added_at_unix_ms: 0,
            })
            .unwrap();
        let (consent, _) = mpsc::channel();
        let mut server =
            HostServer::bind_synthetic(test_config(store.clone(), consent), 0).unwrap();
        assert_eq!(server.current_psks().unwrap().len(), 2);
        for _ in 0..5 {
            server
                .lockout
                .lock()
                .unwrap()
                .record_failure(Instant::now());
        }
        let paired = server.current_psks().unwrap();
        assert_eq!(paired.len(), 1);
        assert_eq!(paired[0].identity(), "maho-p1.stored");
        server.lockout.lock().unwrap().begin_pairing();
        server.pairing_deadline = Some(Instant::now());
        assert_eq!(server.current_psks().unwrap(), paired);
        store.revoke("stored").unwrap();
        let disabled = server.current_psks().unwrap();
        assert_eq!(disabled.len(), 1);
        assert_eq!(disabled[0].identity(), "maho-disabled");
    }

    #[test]
    fn host_default_store_path_and_current_psks_isolation() {
        let default_path = PairingStore::default_path().unwrap();
        assert_eq!(
            default_path.file_name().and_then(|n| n.to_str()),
            Some("host-authorizations.json"),
            "Host default path must be host-authorizations.json"
        );

        let directory = tempdir().unwrap();
        // Place a legacy pairing-keys.json in the directory
        let legacy_file = directory.path().join("pairing-keys.json");
        fs::write(
            &legacy_file,
            br#"[{"id":"legacy-peer-1","name":"Legacy","key":"AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=","addedAt":0.0}]"#,
        )
        .unwrap();

        // Host store pointed to host-authorizations.json in that directory must NOT import legacy-peer-1
        let host_file = directory.path().join("host-authorizations.json");
        let store = PairingStore::new(&host_file);
        assert!(store.load_all().unwrap().is_empty());

        let (consent, _) = mpsc::channel();
        let mut config = test_config(store.clone(), consent);
        config.bootstrap_pin = None;
        config.pairing_window = Duration::from_secs(0);
        let server = HostServer::bind_synthetic(config, 0).unwrap();
        let psks = server.current_psks().unwrap();
        // Since no bootstrap PIN and store is empty, psks only has the disabled fallback psk, never legacy-peer-1
        assert!(
            psks.iter().all(|p| p.identity() != "maho-p1.legacy-peer-1"),
            "Host current_psks must never contain legacy pairing records"
        );

        // Explicitly saved authorization in host store DOES appear in current_psks
        store
            .save(PairingRecord {
                id: "approved-inbound".into(),
                name: "Approved".into(),
                key: [7; 32],
                added_at_unix_ms: 1000,
            })
            .unwrap();
        let updated_psks = server.current_psks().unwrap();
        assert!(
            updated_psks
                .iter()
                .any(|p| p.identity() == "maho-p1.approved-inbound"),
            "Host current_psks must contain explicitly approved inbound authorization"
        );
    }

    #[cfg(target_os = "linux")]
    pub(super) fn admission_input(server: &HostServer) -> io::Result<LinuxInputInjector> {
        if let Some(input) = server.prepared_admission_input.lock().unwrap().take() {
            return Ok(input);
        }
        LinuxInputInjector::new(crate::inject_linux::OutputGeometry::single_output(
            server.config.display.pixel_width,
            server.config.display.pixel_height,
        ))
    }

    fn prepare_admission_input(_server: &mut HostServer) {
        // Real uinput device registration contends across parallel tests. It is
        // a fixture prerequisite, not the TLS/consent wait being measured here.
        // Keep production construction and the single admission deadline intact.
        #[cfg(target_os = "linux")]
        {
            let started = Instant::now();
            let input = admission_input(_server).unwrap();
            *_server.prepared_admission_input.get_mut().unwrap() = Some(input);
            eprintln!("admission input prerequisite: {:?}", started.elapsed());
        }
    }

    fn pairing_deadline_scenario(approve: bool) {
        // Given: costly prerequisites are ready before the admission budget.
        let setup_started = Instant::now();
        let directory = tempdir().unwrap();
        let (consent, prompts) = mpsc::channel();
        let config = test_config(
            PairingStore::new(directory.path().join("keys.json")),
            consent,
        );
        let mut server = HostServer::bind_synthetic(config, 0).unwrap();
        prepare_admission_input(&mut server);
        let client = TlsPskClient::new(PskIdentity::bootstrap("12345678").unwrap()).unwrap();
        let tls = TlsPskServer::new(server.current_psks().unwrap()).unwrap();
        eprintln!(
            "pairing prerequisites (including both KDFs): {:?}",
            setup_started.elapsed()
        );
        let addr = server.tcp_addr().unwrap();
        let (deadline_tx, deadline_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (socket, peer) = server.tcp_listener.accept().unwrap();
            let deadline = Instant::now() + Duration::from_millis(200);
            deadline_tx.send(deadline).unwrap();
            socket.set_nodelay(true).unwrap();
            let result = tls
                .accept_stream_until(socket, deadline)
                .map_err(SessionError::Tls)
                .and_then(|stream| server.handle_connection(stream, peer, deadline));
            done_tx.send((result, Instant::now())).unwrap();
        });
        let mut tcp = client.connect(addr).unwrap();
        let deadline = deadline_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        tcp.ssl_stream()
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        send_tcp_packet(
            &mut tcp,
            PacketType::PairingRequest,
            &PairingRequest {
                name: "deadline".into(),
            }
            .encode()
            .unwrap(),
        )
        .unwrap();
        let prompt = prompts
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| {
                panic!(
                    "consent channel failed: {error:?}; host result: {:?}",
                    done_rx.try_recv()
                )
            });
        // When: actual consent delivery (and, if approved, a wire grant) proves
        // entry to the state under test. An earlier timeout is never success.
        eprintln!(
            "consent entered with {:?} remaining",
            deadline.checked_duration_since(Instant::now())
        );
        let retained_prompt = if approve {
            prompt.approve();
            let packet = tcp.read_frame().unwrap();
            assert_eq!(
                split_packet(&packet).unwrap().0.packet_type,
                PacketType::PairingGrant
            );
            eprintln!(
                "grant received with {:?} remaining",
                deadline.checked_duration_since(Instant::now())
            );
            None
        } else {
            Some(prompt)
        };
        let completion = done_rx.recv_timeout(Duration::from_secs(2));
        let _ = tcp
            .ssl_stream()
            .get_ref()
            .shutdown(std::net::Shutdown::Both);
        drop(tcp);
        drop(retained_prompt);
        worker.join().unwrap();
        // Then: the original absolute deadline ends this specific state while
        // the client (and pending consent sender) are still open.
        let (result, ended_at) = completion.expect("preauth deadline must release the host");
        assert!(ended_at >= deadline, "admission ended before its deadline");
        eprintln!(
            "admission completed {:?} after deadline: {result:?}",
            ended_at.duration_since(deadline)
        );
        if approve {
            assert!(
                matches!(result, Err(SessionError::Io(error)) if error.kind() == io::ErrorKind::TimedOut)
            );
        } else {
            assert!(matches!(result, Err(SessionError::ConsentTimeout)));
        }
    }

    #[test]
    fn preauth_deadline_bounds_pending_consent() {
        pairing_deadline_scenario(false);
    }

    #[test]
    fn preauth_deadline_bounds_granted_client_without_handshake() {
        pairing_deadline_scenario(true);
    }

    #[test]
    fn preauth_deadline_releases_idle_client_before_peer_disconnect() {
        // Given: a paired TLS client that sends no application handshake.
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("keys.json"));
        store
            .save(PairingRecord {
                id: "deadline-client".into(),
                name: "deadline".into(),
                key: [7; 32],
                added_at_unix_ms: 0,
            })
            .unwrap();
        let (consent, _) = mpsc::channel();
        let mut config = test_config(store, consent);
        config.bootstrap_pin = None;
        let mut server = HostServer::bind_synthetic(config, 0).unwrap();
        server.preauth_timeout = Duration::from_millis(50);
        prepare_admission_input(&mut server);
        let addr = server.tcp_addr().unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            done_tx.send(server.serve_next()).unwrap();
        });
        let client =
            TlsPskClient::new(PskIdentity::pairing("deadline-client", &[7; 32]).unwrap()).unwrap();
        let tcp = client.connect(addr).unwrap();
        // When: the total application-handshake budget expires with the peer open.
        let completion = done_rx.recv_timeout(Duration::from_millis(500));
        let ended_before_peer = completion.is_ok();
        let shutdown = tcp
            .ssl_stream()
            .get_ref()
            .shutdown(std::net::Shutdown::Both);
        drop(tcp);
        worker.join().unwrap();
        assert!(shutdown.is_ok() || shutdown.unwrap_err().kind() == io::ErrorKind::NotConnected);
        // Then: the host, not peer disconnect, ends admission with an error.
        assert!(
            ended_before_peer,
            "idle preauth client monopolized host admission"
        );
        assert!(
            matches!(completion.unwrap(), Err(SessionError::Io(error)) if error.kind() == io::ErrorKind::TimedOut)
        );
    }

    #[test]
    fn admission_deadline_releases_silent_tls_then_accepts_paired_client() {
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("keys.json"));
        store
            .save(PairingRecord {
                id: "deadline-client".into(),
                name: "deadline".into(),
                key: [7; 32],
                added_at_unix_ms: 0,
            })
            .unwrap();
        let (consent, _) = mpsc::channel();
        let mut config = test_config(store, consent);
        config.bootstrap_pin = None;
        let mut server = HostServer::bind_synthetic(config, 0).unwrap();
        server.preauth_timeout = Duration::from_millis(100);
        prepare_admission_input(&mut server);
        let addr = server.tcp_addr().unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            done_tx.send(server.serve_next()).unwrap();
            done_tx.send(server.serve_next()).unwrap();
        });
        let silent = TcpStream::connect(addr).unwrap();
        let first = done_rx.recv_timeout(Duration::from_secs(1));
        let ended_before_peer = first.is_ok();
        let _ = silent.shutdown(std::net::Shutdown::Both);
        drop(silent);
        let first = match first {
            Ok(result) => result,
            Err(_) => done_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        };
        let client =
            TlsPskClient::new(PskIdentity::pairing("deadline-client", &[7; 32]).unwrap()).unwrap();
        let socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut tcp = client.connect_stream(socket).unwrap();
        let handshake = Handshake {
            name: "deadline".into(),
            width: 0,
            height: 0,
            scale: 1.0,
            version: PROTOCOL_VERSION,
            capabilities: Capabilities::AUTHENTICATED_UDP_REGISTRATION,
            pairing_id: "deadline-client".into(),
            session_salt: [9; 16],
        };
        send_tcp_packet(
            &mut tcp,
            PacketType::Handshake,
            &handshake.encode().unwrap(),
        )
        .unwrap();
        assert_eq!(
            split_packet(&tcp.read_frame().unwrap())
                .unwrap()
                .0
                .packet_type,
            PacketType::HandshakeAck
        );
        send_tcp_control(&mut tcp, ControlMessage::Disconnect).unwrap();
        let second = done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert!(
            ended_before_peer,
            "silent TLS peer monopolized serial accept"
        );
        assert!(
            matches!(first, Err(SessionError::Tls(TlsPskError::Io(error))) if error.kind() == io::ErrorKind::TimedOut)
        );
        assert!(second.is_ok());
    }
    use maho_net::{PskIdentity, TlsPskClient};
    use tempfile::tempdir;

    #[test]
    fn input_injection_preserves_behavior_with_trace_only_metadata() {
        use tracing_subscriber::prelude::*;
        struct Levels(Arc<Mutex<Vec<tracing::Level>>>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Levels {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _: tracing_subscriber::layer::Context<'_, S>,
            ) {
                self.0.lock().unwrap().push(*event.metadata().level());
            }
        }
        let levels = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Levels(levels.clone()));
        let event = InputEvent::decode(&[0; InputEvent::SIZE]).unwrap();
        let mut injected = Vec::new();
        tracing::subscriber::with_default(subscriber, || {
            inject_input(&event, |actual| {
                injected.push(actual.encode().unwrap());
                Ok::<(), io::Error>(())
            });
        });
        assert_eq!(injected, vec![event.encode().unwrap()]);
        assert_eq!(*levels.lock().unwrap(), vec![tracing::Level::TRACE]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn capture_overload_drops_frame_but_disconnection_stops_bridge() {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(crate::encode_vt::Command::ForceKeyFrame).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let frame = || CaptureFrame {
            width: 2,
            height: 2,
            bytes_per_row: 8,
            bgra: vec![0; 16],
            captured_at: Instant::now(),
        };
        let worker = thread::spawn(move || {
            let keep_running = submit_capture_frame(&tx, frame());
            done_tx.send(keep_running).unwrap();
            tx
        });
        let completed = done_rx.recv_timeout(Duration::from_secs(2));
        // Release the stalled baseline before asserting, so RED leaves no worker behind.
        drop(rx);
        let tx = worker.join().unwrap();
        assert_eq!(completed, Ok(true));
        assert!(!submit_capture_frame(&tx, frame()));
    }

    struct LifecycleSource {
        full: mpsc::Sender<()>,
        stopped: mpsc::Sender<()>,
        sender_exit: mpsc::Sender<()>,
    }

    struct LifecycleHandle {
        producer: Option<thread::JoinHandle<()>>,
        exited: Receiver<()>,
        stopped: mpsc::Sender<()>,
    }

    impl MediaSource for LifecycleSource {
        fn sender_exited(&self) {
            self.sender_exit.send(()).unwrap();
        }
        fn start(
            &self,
            sender: SyncSender<MediaEvent>,
        ) -> Result<Box<dyn MediaHandle>, SessionError> {
            let full = self.full.clone();
            let (exit_tx, exited) = mpsc::channel();
            let producer = thread::spawn(move || {
                let now = Instant::now();
                let frame = VideoFrame {
                    data: vec![0, 0, 0, 1, 0x26],
                    is_key_frame: true,
                    capture_at: now,
                    encode_started_at: now,
                    encode_completed_at: now,
                };
                for _ in 0..16 {
                    sender.send(MediaEvent::Video(frame.clone())).unwrap();
                }
                assert!(matches!(
                    sender.try_send(MediaEvent::Video(frame.clone())),
                    Err(mpsc::TrySendError::Full(_))
                ));
                full.send(()).unwrap();
                while sender.send(MediaEvent::Video(frame.clone())).is_ok() {}
                exit_tx.send(()).unwrap();
            });
            Ok(Box::new(LifecycleHandle {
                producer: Some(producer),
                exited,
                stopped: self.stopped.clone(),
            }))
        }
    }

    impl MediaHandle for LifecycleHandle {
        fn force_key_frame(&self) -> Result<(), SessionError> {
            Ok(())
        }
        fn update_bitrate(&self, _: u32) -> Result<(), SessionError> {
            Ok(())
        }
        fn stop(&mut self) {
            self.exited
                .recv_timeout(Duration::from_secs(2))
                .expect("producer exit before join");
            self.producer.take().unwrap().join().unwrap();
            self.stopped.send(()).unwrap();
        }
    }

    fn lifecycle_scenario(malformed: bool) {
        // Given: real paired TLS and an owned producer filling the actual media queue.
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let key = [3; 32];
        store
            .save(PairingRecord {
                id: "lifecycle".into(),
                name: "fixture".into(),
                key,
                added_at_unix_ms: 0,
            })
            .unwrap();
        let (consent, _) = mpsc::channel();
        let (full_tx, full_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (sender_exit, sender_exited) = mpsc::channel();
        let server = HostServer::bind_with_media(
            test_config(store, consent),
            Arc::new(LifecycleSource {
                full: full_tx,
                stopped: stop_tx,
                sender_exit,
            }),
        )
        .unwrap();
        let tcp_addr = server.tcp_addr().unwrap();
        let udp_addr = server.udp_addr().unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (tcp, peer) = server.tcp_listener.accept().unwrap();
            let tls = TlsPskServer::new(server.current_psks().unwrap()).unwrap();
            let result = server.handle_connection(
                tls.accept_stream(tcp).unwrap(),
                peer,
                Instant::now() + server.preauth_timeout,
            );
            done_tx.send(result).unwrap();
        });
        let client = TlsPskClient::new(PskIdentity::pairing("lifecycle", &key).unwrap()).unwrap();
        let mut tcp = client.connect(tcp_addr).unwrap();
        tcp.ssl_stream_mut()
            .get_mut()
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let handshake = Handshake {
            name: "fixture".into(),
            width: 0,
            height: 0,
            scale: 1.0,
            version: PROTOCOL_VERSION,
            capabilities: Capabilities::AUTHENTICATED_UDP_REGISTRATION,
            pairing_id: "lifecycle".into(),
            session_salt: [5; 16],
        };
        send_tcp_packet(
            &mut tcp,
            PacketType::Handshake,
            &handshake.encode().unwrap(),
        )
        .unwrap();
        assert_eq!(
            split_packet(&tcp.read_frame().unwrap())
                .unwrap()
                .0
                .packet_type,
            PacketType::HandshakeAck
        );
        full_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        // When: disconnect before discovery, or corrupt control after real UDP output.
        if malformed {
            let mut c2h = DatagramCipher::derive(&key, &[5; 16], Direction::ClientToHost).unwrap();
            let reg = c2h
                .seal_datagram(&PacketHeader::new(PacketType::Ping, 0, 0, 0), &[])
                .unwrap();
            udp.send_to(&reg, udp_addr).unwrap();
            let mut buffer = [0; 2048];
            let (length, _) = udp.recv_from(&mut buffer).unwrap();
            let mut cipher =
                DatagramCipher::derive(&key, &[5; 16], Direction::HostToClient).unwrap();
            assert_eq!(
                cipher
                    .open_datagram(&buffer[..length])
                    .unwrap()
                    .0
                    .packet_type,
                PacketType::FrameHeader
            );
            send_tcp_packet(&mut tcp, PacketType::Control, &[]).unwrap();
        } else {
            send_tcp_control(&mut tcp, ControlMessage::Disconnect).unwrap();
        }
        // Then: producer is disconnected and joined before the session completes.
        let stopped = stop_rx.recv_timeout(Duration::from_secs(3));
        let done = done_rx.recv_timeout(Duration::from_secs(3));
        let joined = worker.join();
        assert!(
            stopped.is_ok(),
            "owned producer must be stopped and joined: {stopped:?}"
        );
        assert!(joined.is_ok());
        if malformed {
            sender_exited.recv_timeout(Duration::from_secs(3)).unwrap();
        }
        let result = done.unwrap();
        if malformed {
            assert!(matches!(result, Err(SessionError::Codec(_))));
        } else {
            assert!(result.is_ok());
        }
    }

    #[test]
    fn disconnect_before_udp_unblocks_full_media_queue() {
        lifecycle_scenario(false);
    }

    #[test]
    fn protocol_error_stops_all_media_workers() {
        lifecycle_scenario(true);
    }

    fn test_config(store: PairingStore, consent_sender: mpsc::Sender<ConsentPrompt>) -> HostConfig {
        HostConfig {
            tcp_addr: "127.0.0.1:0".parse().unwrap(),
            udp_addr: "127.0.0.1:0".parse().unwrap(),
            bootstrap_pin: Some("12345678".into()),
            pairing_window: Duration::from_secs(5),
            pairing_store: store,
            host_name: "test-host".into(),
            display: DisplayInfo {
                desktop_x: 0,
                desktop_y: 0,
                logical_width: 640,
                logical_height: 360,
                pixel_width: 640,
                pixel_height: 360,
                scale_factor_milli: 1_000,
            },
            frames_per_second: 60,
            bitrate: 12_000_000,
            capture_audio: false,
            output_name: None,
            consent_sender: Some(consent_sender),
        }
    }

    fn decode_tcp_packet(packet: &[u8]) -> (PacketHeader, &[u8]) {
        split_packet(packet).unwrap()
    }

    #[test]
    fn resolve_output_target_precedence_and_strict_explicit_matching() {
        let monitors_focused: serde_json::Value = serde_json::json!([
            {"name": "DP-1", "focused": false},
            {"name": "HDMI-A-1", "focused": true}
        ]);
        let monitors_none_focused: serde_json::Value = serde_json::json!([
            {"name": "DP-1", "focused": false},
            {"name": "HDMI-A-1", "focused": false}
        ]);
        let empty_monitors: serde_json::Value = serde_json::json!([]);

        // 1. Explicit CLI argument takes highest precedence over env and compositor
        assert_eq!(
            resolve_output_target(
                Some("DP-2".to_string()),
                Some("HDMI-A-1".to_string()),
                Some(&monitors_focused),
            ),
            Some("DP-2".to_string())
        );

        // 2. Whitespace-only CLI argument falls through to env
        assert_eq!(
            resolve_output_target(
                Some("   ".to_string()),
                Some("HDMI-A-1".to_string()),
                Some(&monitors_focused),
            ),
            Some("HDMI-A-1".to_string())
        );

        // 3. Explicit env variable takes precedence over compositor when CLI is absent
        assert_eq!(
            resolve_output_target(None, Some("HDMI-A-1".to_string()), Some(&monitors_focused),),
            Some("HDMI-A-1".to_string())
        );

        // 4. Explicit non-existent target in env is strictly preserved (never silently redirected)
        assert_eq!(
            resolve_output_target(
                None,
                Some("NON_EXISTENT_MONITOR".to_string()),
                Some(&monitors_focused),
            ),
            Some("NON_EXISTENT_MONITOR".to_string())
        );

        // 5. Auto-detect with focused monitor selects the focused monitor
        assert_eq!(
            resolve_output_target(None, None, Some(&monitors_focused)),
            Some("HDMI-A-1".to_string())
        );

        // 6. Auto-detect with no focused monitor falls back to first named monitor
        assert_eq!(
            resolve_output_target(None, None, Some(&monitors_none_focused)),
            Some("DP-1".to_string())
        );

        // 7. Auto-detect with empty monitors array returns None
        assert_eq!(
            resolve_output_target(None, None, Some(&empty_monitors)),
            None
        );

        // 8. Auto-detect with no compositor data returns None
        assert_eq!(resolve_output_target(None, None, None), None);
    }

    #[test]
    fn select_focused_output_finds_focused_or_falls_back_to_first() {
        let json_with_focused: serde_json::Value = serde_json::json!([
            {"name": "DP-1", "focused": false},
            {"name": "HDMI-A-1", "focused": true}
        ]);
        assert_eq!(
            select_focused_output(&json_with_focused),
            Some("HDMI-A-1".to_string())
        );

        let json_no_focused: serde_json::Value = serde_json::json!([
            {"name": "HDMI-A-1", "focused": false},
            {"name": "DP-1", "focused": false}
        ]);
        assert_eq!(
            select_focused_output(&json_no_focused),
            Some("HDMI-A-1".to_string())
        );

        let empty_array: serde_json::Value = serde_json::json!([]);
        assert_eq!(select_focused_output(&empty_array), None);

        let not_an_array: serde_json::Value = serde_json::json!({"name": "HDMI-A-1"});
        assert_eq!(select_focused_output(&not_an_array), None);
    }

    #[test]
    fn pre_auth_gating_refuses_input_and_control() {
        assert!(!SessionState::PreAuth.allows(PacketType::InputEvent));
        assert!(!SessionState::PreAuth.allows(PacketType::Control));
        assert!(SessionState::PreAuth.allows(PacketType::PairingRequest));
        assert!(SessionState::Authenticated.allows(PacketType::InputEvent));
    }

    #[test]
    fn lockout_disables_bootstrap_after_five_failures() {
        let now = Instant::now();
        let mut lockout = BootstrapLockout::default();
        lockout.begin_pairing();
        for attempt in 0..4 {
            assert!(!lockout.record_failure(now + Duration::from_secs(attempt)));
        }
        assert!(lockout.record_failure(now + Duration::from_secs(4)));
        assert!(!lockout.is_allowed(now + Duration::from_secs(5)));
    }

    #[test]
    fn direction_keys_arm_and_only_matching_direction_opens() {
        let key = [7_u8; 32];
        let salt = [9_u8; 16];
        let header = PacketHeader::new(PacketType::Ping, 1, 2, 0);
        let mut host_send = DatagramCipher::derive(&key, &salt, Direction::HostToClient).unwrap();
        let mut client_receive =
            DatagramCipher::derive(&key, &salt, Direction::HostToClient).unwrap();
        let mut wrong = DatagramCipher::derive(&key, &salt, Direction::ClientToHost).unwrap();
        let datagram = host_send.seal_datagram(&header, b"armed").unwrap();
        assert_eq!(client_receive.open_datagram(&datagram).unwrap().1, b"armed");
        assert!(wrong.open_datagram(&datagram).is_err());
    }

    #[test]
    fn pairing_store_mirrors_swift_json_and_is_mode_0600() {
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let record = PairingRecord {
            id: "id".into(),
            name: "client".into(),
            key: [3; 32],
            added_at_unix_ms: 1_700_000_000_000,
        };
        store.save(record.clone()).unwrap();
        assert_eq!(store.load("id").unwrap(), Some(record));
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert!(json[0]["key"].is_string());
        assert!(json[0]["addedAt"].is_number());
    }

    #[test]
    fn timestamp_stats_round_trip() {
        let stats = TimestampStats {
            frame_id: 7,
            capture_us: 10,
            encode_start_us: 20,
            encode_end_us: 30,
            send_us: 40,
        };
        assert_eq!(TimestampStats::decode(&stats.encode()), Some(stats));
    }

    #[test]
    fn scripted_loopback_pairs_arms_ciphers_and_receives_ten_timestamped_frames() {
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let (consent_tx, consent_rx) = mpsc::channel();
        let server = HostServer::bind_synthetic(test_config(store, consent_tx), 12).unwrap();
        let tcp_addr = server.tcp_addr().unwrap();
        let udp_addr = server.udp_addr().unwrap();
        let server_thread = thread::spawn(move || server.serve_n(1).unwrap());
        let consent_thread = thread::spawn(move || {
            let prompt = consent_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(prompt.client_name, "scripted-client");
            prompt.approve();
        });

        let client = TlsPskClient::new(PskIdentity::bootstrap("12345678").unwrap()).unwrap();
        let mut tcp = client.connect(tcp_addr).unwrap();
        let request = PairingRequest {
            name: "scripted-client".into(),
        };
        let mut packet = PacketHeader::new(PacketType::PairingRequest, 0, 0, 0)
            .encode()
            .unwrap();
        packet.extend_from_slice(&request.encode().unwrap());
        tcp.write_frame(&packet).unwrap();
        let grant_packet = tcp.read_frame().unwrap();
        let (header, payload) = decode_tcp_packet(&grant_packet);
        assert_eq!(header.packet_type, PacketType::PairingGrant);
        let grant = PairingGrant::decode(payload).unwrap();

        let salt = [0x55_u8; 16];
        let handshake = Handshake {
            name: "scripted-client".into(),
            width: 0,
            height: 0,
            scale: 1.0,
            version: PROTOCOL_VERSION,
            capabilities: Capabilities::AUTHENTICATED_UDP_REGISTRATION,
            pairing_id: grant.pairing_id,
            session_salt: salt,
        };
        let mut packet = PacketHeader::new(PacketType::Handshake, 0, 0, 0)
            .encode()
            .unwrap();
        packet.extend_from_slice(&handshake.encode().unwrap());
        tcp.write_frame(&packet).unwrap();
        let ack = tcp.read_frame().unwrap();
        assert_eq!(
            decode_tcp_packet(&ack).0.packet_type,
            PacketType::HandshakeAck
        );

        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut c2h = DatagramCipher::derive(&grant.key, &salt, Direction::ClientToHost).unwrap();
        let reg = c2h
            .seal_datagram(&PacketHeader::new(PacketType::Ping, 0, 0, 0), &[])
            .unwrap();
        udp.send_to(&reg, udp_addr).unwrap();
        let mut receive =
            DatagramCipher::derive(&grant.key, &salt, Direction::HostToClient).unwrap();
        let mut buffer = [0_u8; 2_048];
        let mut timestamps = Vec::new();
        while timestamps.len() < 10 {
            let (length, _) = udp.recv_from(&mut buffer).unwrap();
            let (header, payload) = receive.open_datagram(&buffer[..length]).unwrap();
            if header.packet_type == PacketType::Ping {
                if let Some(stats) = TimestampStats::decode(&payload) {
                    assert!(stats.capture_us <= stats.encode_start_us);
                    assert!(stats.encode_start_us <= stats.encode_end_us);
                    assert!(stats.encode_end_us <= stats.send_us);
                    timestamps.push(stats);
                }
            }
        }
        assert!(timestamps
            .windows(2)
            .all(|pair| pair[0].frame_id < pair[1].frame_id));
        send_tcp_control(&mut tcp, ControlMessage::Disconnect).unwrap();
        consent_thread.join().unwrap();
        server_thread.join().unwrap();
    }

    struct BitrateRecordSource {
        bitrates: Arc<Mutex<Vec<u32>>>,
    }

    struct BitrateRecordHandle {
        bitrates: Arc<Mutex<Vec<u32>>>,
    }

    impl MediaSource for BitrateRecordSource {
        fn start(
            &self,
            _sender: SyncSender<MediaEvent>,
        ) -> Result<Box<dyn MediaHandle>, SessionError> {
            Ok(Box::new(BitrateRecordHandle {
                bitrates: Arc::clone(&self.bitrates),
            }))
        }
    }

    impl MediaHandle for BitrateRecordHandle {
        fn force_key_frame(&self) -> Result<(), SessionError> {
            Ok(())
        }
        fn update_bitrate(&self, bitrate: u32) -> Result<(), SessionError> {
            self.bitrates.lock().unwrap().push(bitrate);
            Ok(())
        }
        fn stop(&mut self) {}
    }

    #[test]
    fn bitrate_adjust_updates_encoder_and_rejects_malformed_limits() {
        // Given: paired host server with tracked media handle.
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let key = [0x5a; 32];
        store
            .save(PairingRecord {
                id: "bitrate-test".into(),
                name: "fixture".into(),
                key,
                added_at_unix_ms: 0,
            })
            .unwrap();
        let (consent_tx, _) = mpsc::channel();
        let config = test_config(store, consent_tx);
        let bitrates = Arc::new(Mutex::new(Vec::new()));
        let source = Arc::new(BitrateRecordSource {
            bitrates: Arc::clone(&bitrates),
        });
        let server = HostServer::bind_with_media(config, source).unwrap();
        let tcp_addr = server.tcp_addr().unwrap();
        let server_thread = thread::spawn(move || server.serve_n(1).unwrap());

        let client =
            TlsPskClient::new(PskIdentity::pairing("bitrate-test", &key).unwrap()).unwrap();
        let mut tcp = client.connect(tcp_addr).unwrap();
        let handshake = Handshake {
            name: "scripted-client".into(),
            width: 0,
            height: 0,
            scale: 1.0,
            version: PROTOCOL_VERSION,
            capabilities: Capabilities::AUTHENTICATED_UDP_REGISTRATION,
            pairing_id: "bitrate-test".into(),
            session_salt: [0x33; 16],
        };
        let mut packet = PacketHeader::new(PacketType::Handshake, 0, 0, 0)
            .encode()
            .unwrap();
        packet.extend_from_slice(&handshake.encode().unwrap());
        tcp.write_frame(&packet).unwrap();
        let ack = tcp.read_frame().unwrap();
        assert_eq!(
            decode_tcp_packet(&ack).0.packet_type,
            PacketType::HandshakeAck
        );

        // When: client submits malformed non-positive and valid positive bitrates.
        send_tcp_control(
            &mut tcp,
            ControlMessage::BitrateAdjust(BitrateAdjust { target_bitrate: 0 }),
        )
        .unwrap();
        send_tcp_control(
            &mut tcp,
            ControlMessage::BitrateAdjust(BitrateAdjust {
                target_bitrate: -100,
            }),
        )
        .unwrap();
        send_tcp_control(
            &mut tcp,
            ControlMessage::BitrateAdjust(BitrateAdjust {
                target_bitrate: 4_000_000,
            }),
        )
        .unwrap();
        send_tcp_control(
            &mut tcp,
            ControlMessage::BitrateAdjust(BitrateAdjust {
                target_bitrate: 15_000_000,
            }),
        )
        .unwrap();
        send_tcp_control(&mut tcp, ControlMessage::Ping).unwrap();
        let pong = tcp.read_frame().unwrap();
        let (header, payload) = decode_tcp_packet(&pong);
        assert_eq!(header.packet_type, PacketType::Control);
        assert_eq!(
            ControlMessage::decode(payload).unwrap(),
            ControlMessage::Pong
        );

        // Then: host applied only positive bitrates to media encoder in exact order.
        assert_eq!(*bitrates.lock().unwrap(), vec![4_000_000, 15_000_000]);

        send_tcp_control(&mut tcp, ControlMessage::Disconnect).unwrap();
        server_thread.join().unwrap();
    }

    #[test]
    fn stream_configuration_negotiates_valid_and_rejects_invalid_values() {
        // Given: paired host server with tracked media handle.
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let key = [0x7c; 32];
        store
            .save(PairingRecord {
                id: "config-test".into(),
                name: "fixture".into(),
                key,
                added_at_unix_ms: 0,
            })
            .unwrap();
        let (consent_tx, _) = mpsc::channel();
        let config = test_config(store, consent_tx);
        let bitrates = Arc::new(Mutex::new(Vec::new()));
        let source = Arc::new(BitrateRecordSource {
            bitrates: Arc::clone(&bitrates),
        });
        let server = HostServer::bind_with_media(config, source).unwrap();
        let tcp_addr = server.tcp_addr().unwrap();
        let server_thread = thread::spawn(move || server.serve_n(1).unwrap());

        let client = TlsPskClient::new(PskIdentity::pairing("config-test", &key).unwrap()).unwrap();
        let mut tcp = client.connect(tcp_addr).unwrap();
        let handshake = Handshake {
            name: "scripted-client".into(),
            width: 0,
            height: 0,
            scale: 1.0,
            version: PROTOCOL_VERSION,
            capabilities: Capabilities::AUTHENTICATED_UDP_REGISTRATION,
            pairing_id: "config-test".into(),
            session_salt: [0x88; 16],
        };
        let mut packet = PacketHeader::new(PacketType::Handshake, 0, 0, 0)
            .encode()
            .unwrap();
        packet.extend_from_slice(&handshake.encode().unwrap());
        tcp.write_frame(&packet).unwrap();
        let ack = tcp.read_frame().unwrap();
        assert_eq!(
            decode_tcp_packet(&ack).0.packet_type,
            PacketType::HandshakeAck
        );

        // When: client requests invalid dimensions.
        let invalid_dim = maho_proto::StreamConfigurationRequest {
            request_id: 101,
            desired: maho_proto::StreamConfiguration {
                width: 0,
                height: 1080,
                bitrate: 5_000_000,
                frames_per_second: 60,
            },
        };
        send_tcp_control(&mut tcp, ControlMessage::StreamConfigRequest(invalid_dim)).unwrap();
        let reject_frame = tcp.read_frame().unwrap();
        let (header, payload) = decode_tcp_packet(&reject_frame);
        assert_eq!(header.packet_type, PacketType::Control);
        match ControlMessage::decode(payload).unwrap() {
            ControlMessage::StreamConfigReject(rej) => {
                assert_eq!(rej.request_id, 101);
                assert_eq!(
                    rej.reason,
                    StreamConfigurationErrorCode::UnsupportedDimensions
                );
            }
            other => panic!("expected reject, got {other:?}"),
        }

        // When: client requests invalid fps.
        let invalid_fps = maho_proto::StreamConfigurationRequest {
            request_id: 102,
            desired: maho_proto::StreamConfiguration {
                width: 1920,
                height: 1080,
                bitrate: 5_000_000,
                frames_per_second: 0,
            },
        };
        send_tcp_control(&mut tcp, ControlMessage::StreamConfigRequest(invalid_fps)).unwrap();
        let reject_frame = tcp.read_frame().unwrap();
        let (_, payload) = decode_tcp_packet(&reject_frame);
        match ControlMessage::decode(payload).unwrap() {
            ControlMessage::StreamConfigReject(rej) => {
                assert_eq!(rej.request_id, 102);
                assert_eq!(rej.reason, StreamConfigurationErrorCode::UnsupportedFps);
            }
            other => panic!("expected reject, got {other:?}"),
        }

        // When: client requests valid configuration.
        let valid_req = maho_proto::StreamConfigurationRequest {
            request_id: 103,
            desired: maho_proto::StreamConfiguration {
                width: 2560,
                height: 1440,
                bitrate: 8_000_000,
                frames_per_second: 60,
            },
        };
        send_tcp_control(&mut tcp, ControlMessage::StreamConfigRequest(valid_req)).unwrap();
        let resp_frame = tcp.read_frame().unwrap();
        let (_, payload) = decode_tcp_packet(&resp_frame);
        match ControlMessage::decode(payload).unwrap() {
            ControlMessage::StreamConfigResponse(resp) => {
                assert_eq!(resp.request_id, 103);
                assert_eq!(resp.active, valid_req.desired);
            }
            other => panic!("expected response, got {other:?}"),
        }

        // Then: host updated encoder with negotiated bitrate.
        assert_eq!(*bitrates.lock().unwrap(), vec![8_000_000]);

        send_tcp_control(&mut tcp, ControlMessage::Disconnect).unwrap();
        server_thread.join().unwrap();
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TrackingMediaSource {
        started_count: Arc<AtomicUsize>,
    }

    struct TrackingMediaHandle;
    impl MediaHandle for TrackingMediaHandle {
        fn force_key_frame(&self) -> Result<(), SessionError> {
            Ok(())
        }
        fn update_bitrate(&self, _bitrate: u32) -> Result<(), SessionError> {
            Ok(())
        }
        fn stop(&mut self) {}
    }

    impl MediaSource for TrackingMediaSource {
        fn start(
            &self,
            _sender: SyncSender<MediaEvent>,
        ) -> Result<Box<dyn MediaHandle>, SessionError> {
            self.started_count.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(TrackingMediaHandle))
        }
    }

    fn make_handshake_packet(pairing_id: &str, session_salt: [u8; 16]) -> Vec<u8> {
        let handshake = Handshake {
            name: "test-client".into(),
            width: 0,
            height: 0,
            scale: 1.0,
            version: PROTOCOL_VERSION,
            capabilities: Capabilities::AUTHENTICATED_UDP_REGISTRATION,
            pairing_id: pairing_id.to_string(),
            session_salt,
        };
        let mut packet = PacketHeader::new(PacketType::Handshake, 0, 0, 0)
            .encode()
            .unwrap();
        packet.extend_from_slice(&handshake.encode().unwrap());
        packet
    }

    #[test]
    fn test_bootstrap_without_consent_handshake_rejected_with_no_capture_or_input() {
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let key_a = [0x42; 32];
        store
            .save(PairingRecord {
                id: "PAIR_A".into(),
                name: "client-A".into(),
                key: key_a,
                added_at_unix_ms: 0,
            })
            .unwrap();
        let (consent_tx, _consent_rx) = mpsc::channel();
        let config = test_config(store, consent_tx);
        let media_starts = Arc::new(AtomicUsize::new(0));
        let server = HostServer::bind_with_media(
            config,
            Arc::new(TrackingMediaSource {
                started_count: Arc::clone(&media_starts),
            }),
        )
        .unwrap();
        let tcp_addr = server.tcp_addr().unwrap();
        let server_thread = thread::spawn(move || server.serve_n(1));

        let client = TlsPskClient::new(PskIdentity::bootstrap("12345678").unwrap()).unwrap();
        let mut tcp = client.connect(tcp_addr).unwrap();
        tcp.ssl_stream()
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        // Transmit unauthenticated Control and InputEvent packets before handshake; host must refuse them
        let control_packet = {
            let mut p = PacketHeader::new(PacketType::Control, 0, 0, 0)
                .encode()
                .unwrap();
            p.extend_from_slice(&ControlMessage::Ping.encode().unwrap());
            p
        };
        tcp.write_frame(&control_packet).unwrap();

        let input_packet = {
            let event = InputEvent {
                event_type: maho_proto::InputEventType::MouseMove,
                x: 100.0,
                y: 100.0,
                key_code: 0,
                modifiers: maho_proto::Modifiers::empty(),
                scroll_dx: 0.0,
                scroll_dy: 0.0,
            };
            let mut p = PacketHeader::new(PacketType::InputEvent, 0, 0, 0)
                .encode()
                .unwrap();
            p.extend_from_slice(&event.encode().unwrap());
            p
        };
        tcp.write_frame(&input_packet).unwrap();

        let handshake_packet = make_handshake_packet("PAIR_A", [0x11; 16]);
        tcp.write_frame(&handshake_packet).unwrap();

        let read_result = tcp.read_frame();
        let _ = send_tcp_control(&mut tcp, ControlMessage::Disconnect);
        drop(tcp);
        let server_result = server_thread.join().unwrap();

        assert!(
            matches!(server_result, Err(SessionError::PreAuth)),
            "server must reject unconsented bootstrap handshake with PreAuth, got: {server_result:?}"
        );
        assert_eq!(
            media_starts.load(Ordering::SeqCst),
            0,
            "media capture must not start without consent"
        );
        if let Ok(frame) = read_result {
            let (hdr, _) = decode_tcp_packet(&frame);
            assert_ne!(
                hdr.packet_type,
                PacketType::HandshakeAck,
                "server must not send HandshakeAck for unconsented handshake"
            );
            assert_ne!(
                hdr.packet_type,
                PacketType::Control,
                "server must not send Control/InputAck for unauthenticated input"
            );
        }
    }

    #[test]
    fn test_bootstrap_consent_b_cannot_use_a() {
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let key_a = [0x42; 32];
        store
            .save(PairingRecord {
                id: "PAIR_A".into(),
                name: "client-A".into(),
                key: key_a,
                added_at_unix_ms: 0,
            })
            .unwrap();
        let (consent_tx, consent_rx) = mpsc::channel();
        let config = test_config(store, consent_tx);
        let media_starts = Arc::new(AtomicUsize::new(0));
        let server = HostServer::bind_with_media(
            config,
            Arc::new(TrackingMediaSource {
                started_count: Arc::clone(&media_starts),
            }),
        )
        .unwrap();
        let tcp_addr = server.tcp_addr().unwrap();
        let server_thread = thread::spawn(move || server.serve_n(1));
        let consent_thread = thread::spawn(move || {
            let prompt: ConsentPrompt = consent_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(prompt.client_name, "client-B");
            prompt.approve();
        });

        let client = TlsPskClient::new(PskIdentity::bootstrap("12345678").unwrap()).unwrap();
        let mut tcp = client.connect(tcp_addr).unwrap();
        tcp.ssl_stream()
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        let request = PairingRequest {
            name: "client-B".into(),
        };
        let mut req_packet = PacketHeader::new(PacketType::PairingRequest, 0, 0, 0)
            .encode()
            .unwrap();
        req_packet.extend_from_slice(&request.encode().unwrap());
        tcp.write_frame(&req_packet).unwrap();

        let grant_frame = tcp.read_frame().unwrap();
        let (grant_hdr, grant_payload) = decode_tcp_packet(&grant_frame);
        assert_eq!(grant_hdr.packet_type, PacketType::PairingGrant);
        let _grant = PairingGrant::decode(grant_payload).unwrap();
        consent_thread.join().unwrap();

        let handshake_packet = make_handshake_packet("PAIR_A", [0x22; 16]);
        tcp.write_frame(&handshake_packet).unwrap();

        let read_result = tcp.read_frame();
        let _ = send_tcp_control(&mut tcp, ControlMessage::Disconnect);
        drop(tcp);
        let server_result = server_thread.join().unwrap();

        assert!(
            matches!(server_result, Err(SessionError::IdentityMismatch)),
            "server must reject handshake with mismatched pairing ID with IdentityMismatch, got: {server_result:?}"
        );
        assert_eq!(
            media_starts.load(Ordering::SeqCst),
            0,
            "media capture must not start on mismatched ID"
        );
        if let Ok(frame) = read_result {
            let (hdr, _) = decode_tcp_packet(&frame);
            assert_ne!(
                hdr.packet_type,
                PacketType::HandshakeAck,
                "server must not send HandshakeAck for mismatched pairing ID"
            );
        }
    }

    #[test]
    fn test_bootstrap_normal_b_and_paired_a_work() {
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let key_a = [0x42; 32];
        store
            .save(PairingRecord {
                id: "PAIR_A".into(),
                name: "client-A".into(),
                key: key_a,
                added_at_unix_ms: 0,
            })
            .unwrap();

        // Part 1: Normal B works over bootstrap TLS
        {
            let (consent_tx, consent_rx) = mpsc::channel();
            let config = test_config(store.clone(), consent_tx);
            let media_starts = Arc::new(AtomicUsize::new(0));
            let server = HostServer::bind_with_media(
                config,
                Arc::new(TrackingMediaSource {
                    started_count: Arc::clone(&media_starts),
                }),
            )
            .unwrap();
            let tcp_addr = server.tcp_addr().unwrap();
            let server_thread = thread::spawn(move || server.serve_n(1));
            let consent_thread = thread::spawn(move || {
                let prompt: ConsentPrompt =
                    consent_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                assert_eq!(prompt.client_name, "client-B");
                prompt.approve();
            });

            let client = TlsPskClient::new(PskIdentity::bootstrap("12345678").unwrap()).unwrap();
            let mut tcp = client.connect(tcp_addr).unwrap();
            tcp.ssl_stream()
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();

            let request = PairingRequest {
                name: "client-B".into(),
            };
            let mut req_packet = PacketHeader::new(PacketType::PairingRequest, 0, 0, 0)
                .encode()
                .unwrap();
            req_packet.extend_from_slice(&request.encode().unwrap());
            tcp.write_frame(&req_packet).unwrap();

            let grant_frame = tcp.read_frame().unwrap();
            let (grant_hdr, grant_payload) = decode_tcp_packet(&grant_frame);
            assert_eq!(grant_hdr.packet_type, PacketType::PairingGrant);
            let grant = PairingGrant::decode(grant_payload).unwrap();
            consent_thread.join().unwrap();

            let handshake_packet = make_handshake_packet(&grant.pairing_id, [0x33; 16]);
            tcp.write_frame(&handshake_packet).unwrap();

            let ack_frame = tcp.read_frame().unwrap();
            let (ack_hdr, _) = decode_tcp_packet(&ack_frame);
            assert_eq!(ack_hdr.packet_type, PacketType::HandshakeAck);
            assert_eq!(media_starts.load(Ordering::SeqCst), 1);

            send_tcp_control(&mut tcp, ControlMessage::Disconnect).unwrap();
            assert!(server_thread.join().unwrap().is_ok());
        }

        // Part 2: Paired A works over pairing TLS
        {
            let (consent_tx, _consent_rx) = mpsc::channel();
            let config = test_config(store.clone(), consent_tx);
            let media_starts = Arc::new(AtomicUsize::new(0));
            let server = HostServer::bind_with_media(
                config,
                Arc::new(TrackingMediaSource {
                    started_count: Arc::clone(&media_starts),
                }),
            )
            .unwrap();
            let tcp_addr = server.tcp_addr().unwrap();
            let server_thread = thread::spawn(move || server.serve_n(1));

            let client =
                TlsPskClient::new(PskIdentity::pairing("PAIR_A", &key_a).unwrap()).unwrap();
            let mut tcp = client.connect(tcp_addr).unwrap();
            tcp.ssl_stream()
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();

            let handshake_packet = make_handshake_packet("PAIR_A", [0x44; 16]);
            tcp.write_frame(&handshake_packet).unwrap();

            let ack_frame = tcp.read_frame().unwrap();
            let (ack_hdr, _) = decode_tcp_packet(&ack_frame);
            assert_eq!(ack_hdr.packet_type, PacketType::HandshakeAck);
            assert_eq!(media_starts.load(Ordering::SeqCst), 1);

            send_tcp_control(&mut tcp, ControlMessage::Disconnect).unwrap();
            assert!(server_thread.join().unwrap().is_ok());
        }
    }

    #[test]
    fn test_bootstrap_authenticated_session_rejects_duplicate_handshake() {
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let key_a = [0x42; 32];
        store
            .save(PairingRecord {
                id: "PAIR_A".into(),
                name: "client-A".into(),
                key: key_a,
                added_at_unix_ms: 0,
            })
            .unwrap();

        let (consent_tx, _consent_rx) = mpsc::channel();
        let config = test_config(store, consent_tx);
        let media_starts = Arc::new(AtomicUsize::new(0));
        let server = HostServer::bind_with_media(
            config,
            Arc::new(TrackingMediaSource {
                started_count: Arc::clone(&media_starts),
            }),
        )
        .unwrap();
        let tcp_addr = server.tcp_addr().unwrap();
        let server_thread = thread::spawn(move || server.serve_n(1));

        let client = TlsPskClient::new(PskIdentity::pairing("PAIR_A", &key_a).unwrap()).unwrap();
        let mut tcp = client.connect(tcp_addr).unwrap();
        tcp.ssl_stream()
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        // 1. First Handshake: must succeed
        let handshake_packet = make_handshake_packet("PAIR_A", [0x55; 16]);
        tcp.write_frame(&handshake_packet).unwrap();
        let ack_frame = tcp.read_frame().unwrap();
        assert_eq!(
            decode_tcp_packet(&ack_frame).0.packet_type,
            PacketType::HandshakeAck
        );
        assert_eq!(media_starts.load(Ordering::SeqCst), 1);

        // 2. Second Handshake on authenticated session: must be rejected with error and closed
        let duplicate_handshake = make_handshake_packet("PAIR_A", [0x66; 16]);
        tcp.write_frame(&duplicate_handshake).unwrap();

        let read_second = tcp.read_frame();
        let _ = send_tcp_control(&mut tcp, ControlMessage::Disconnect);
        drop(tcp);
        let server_result = server_thread.join().unwrap();

        assert!(
            matches!(server_result, Err(SessionError::AlreadyAuthenticated)),
            "server must reject duplicate handshake with AlreadyAuthenticated, got: {server_result:?}"
        );
        assert_eq!(
            media_starts.load(Ordering::SeqCst),
            1,
            "media capture must not restart on duplicate handshake"
        );
        if let Ok(frame) = read_second {
            let (hdr, _) = decode_tcp_packet(&frame);
            assert_ne!(
                hdr.packet_type,
                PacketType::HandshakeAck,
                "server must not send HandshakeAck for duplicate handshake"
            );
        }
    }

    #[test]
    fn test_serve_with_stop_exits_when_flag_set() {
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let (consent_tx, _consent_rx) = mpsc::channel();
        let config = test_config(store, consent_tx);
        let server = HostServer::bind(config).unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let server_thread = thread::spawn(move || server.serve_with_stop(stop_clone));

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let result = server_thread.join().expect("server thread joined");
        assert!(result.is_ok());
    }

    #[test]
    fn test_udp_discover_peer_rejects_invalid_packets_and_fixes_endpoint() {
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let (consent_tx, _consent_rx) = mpsc::channel();
        let config = test_config(store, consent_tx);
        let server = HostServer::bind(config).unwrap();
        let host_udp_addr = server.udp_socket.local_addr().unwrap();

        let key = [0x77; 32];
        let salt = [0x88; 16];
        let prior_salt = [0x33; 16];

        // Independent client-send and host-receive cipher instances
        let mut client_c2h = DatagramCipher::derive(&key, &salt, Direction::ClientToHost).unwrap();
        let mut host_c2h = DatagramCipher::derive(&key, &salt, Direction::ClientToHost).unwrap();
        let mut host_h2c = DatagramCipher::derive(&key, &salt, Direction::HostToClient).unwrap();
        let mut client_h2c = DatagramCipher::derive(&key, &salt, Direction::HostToClient).unwrap();
        let mut prior_client_c2h =
            DatagramCipher::derive(&key, &prior_salt, Direction::ClientToHost).unwrap();

        let client_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        client_udp
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let client_addr = client_udp.local_addr().unwrap();

        let attacker_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        attacker_udp.set_nonblocking(true).unwrap();
        let attacker_addr = attacker_udp.local_addr().unwrap();

        let tcp_peer = client_addr; // Same IP: 127.0.0.1
        let mut udp_peer: Option<SocketAddr> = None;

        let mut peek_buf = [0_u8; 2048];
        let mut check_buf = [0_u8; 64];

        // Stage 1a: Plaintext 0xff probe from attacker must be consumed but NOT set udp_peer
        attacker_udp.send_to(&[0xff], host_udp_addr).unwrap();
        let (peek_len, peek_from) = peek_datagram(&server.udp_socket, &mut peek_buf);
        assert_eq!(peek_len, 1);
        assert_eq!(peek_from, attacker_addr);
        server
            .discover_udp_peer(tcp_peer, &mut udp_peer, Some(&mut host_c2h))
            .unwrap();
        assert_eq!(udp_peer, None, "plaintext probe must not register endpoint");
        assert!(
            matches!(server.udp_socket.recv_from(&mut check_buf), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock),
            "plaintext probe must have been consumed from socket"
        );

        // Stage 1b: Short malformed packet (< 40 bytes)
        let malformed = [0x45, 0x52, 0x07, 0x00, 0x01, 0x02];
        attacker_udp.send_to(&malformed, host_udp_addr).unwrap();
        let (peek_len, _) = peek_datagram(&server.udp_socket, &mut peek_buf);
        assert_eq!(peek_len, malformed.len());
        server
            .discover_udp_peer(tcp_peer, &mut udp_peer, Some(&mut host_c2h))
            .unwrap();
        assert_eq!(udp_peer, None, "short packet must not register endpoint");
        assert!(
            matches!(server.udp_socket.recv_from(&mut check_buf), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock),
            "short packet must have been consumed from socket"
        );

        // Stage 1c: Packet encrypted with wrong key
        let wrong_key = [0x99; 32];
        let mut wrong_cipher =
            DatagramCipher::derive(&wrong_key, &salt, Direction::ClientToHost).unwrap();
        let ping_hdr = PacketHeader::new(PacketType::Ping, 0, 1000, 0);
        let wrong_packet = wrong_cipher.seal_datagram(&ping_hdr, &[]).unwrap();
        attacker_udp.send_to(&wrong_packet, host_udp_addr).unwrap();
        let (peek_len, _) = peek_datagram(&server.udp_socket, &mut peek_buf);
        assert_eq!(peek_len, wrong_packet.len());
        server
            .discover_udp_peer(tcp_peer, &mut udp_peer, Some(&mut host_c2h))
            .unwrap();
        assert_eq!(
            udp_peer, None,
            "wrong-key packet must not register endpoint"
        );
        assert!(
            matches!(server.udp_socket.recv_from(&mut check_buf), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock),
            "wrong-key packet must have been consumed from socket"
        );

        // Stage 1d: Non-registration packet (valid encryption, but PacketType::InputEvent)
        let input_hdr = PacketHeader::new(PacketType::InputEvent, 1, 1000, 0);
        let non_reg_packet = client_c2h.seal_datagram(&input_hdr, &[]).unwrap();
        client_udp.send_to(&non_reg_packet, host_udp_addr).unwrap();
        let (peek_len, _) = peek_datagram(&server.udp_socket, &mut peek_buf);
        assert_eq!(peek_len, non_reg_packet.len());
        server
            .discover_udp_peer(tcp_peer, &mut udp_peer, Some(&mut host_c2h))
            .unwrap();
        assert_eq!(
            udp_peer, None,
            "non-registration packet must not register endpoint"
        );
        assert!(
            matches!(server.udp_socket.recv_from(&mut check_buf), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock),
            "non-registration packet must have been consumed from socket"
        );

        // Stage 1e: Ping with non-empty payload
        let ping_payload_hdr = PacketHeader::new(PacketType::Ping, 2, 1000, 0);
        let payload_packet = client_c2h
            .seal_datagram(&ping_payload_hdr, &[1, 2, 3])
            .unwrap();
        client_udp.send_to(&payload_packet, host_udp_addr).unwrap();
        let (peek_len, _) = peek_datagram(&server.udp_socket, &mut peek_buf);
        assert_eq!(peek_len, payload_packet.len());
        server
            .discover_udp_peer(tcp_peer, &mut udp_peer, Some(&mut host_c2h))
            .unwrap();
        assert_eq!(
            udp_peer, None,
            "ping with non-empty payload must not register endpoint"
        );
        assert!(
            matches!(server.udp_socket.recv_from(&mut check_buf), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock),
            "non-empty ping must have been consumed from socket"
        );

        // Stage 1f: Prior-session salt packet
        let prior_hdr = PacketHeader::new(PacketType::Ping, 0, 1000, 0);
        let prior_packet = prior_client_c2h.seal_datagram(&prior_hdr, &[]).unwrap();
        client_udp.send_to(&prior_packet, host_udp_addr).unwrap();
        let (peek_len, _) = peek_datagram(&server.udp_socket, &mut peek_buf);
        assert_eq!(peek_len, prior_packet.len());
        server
            .discover_udp_peer(tcp_peer, &mut udp_peer, Some(&mut host_c2h))
            .unwrap();
        assert_eq!(
            udp_peer, None,
            "prior-session packet must not register endpoint"
        );
        assert!(
            matches!(server.udp_socket.recv_from(&mut check_buf), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock),
            "prior-session packet must have been consumed from socket"
        );

        // Stage 2: Genuine registration packet from legitimate client (sealed empty Ping)
        let reg_hdr = PacketHeader::new(PacketType::Ping, 3, 1000, 0);
        let reg_packet = client_c2h.seal_datagram(&reg_hdr, &[]).unwrap();
        client_udp.send_to(&reg_packet, host_udp_addr).unwrap();
        let (peek_len, _) = peek_datagram(&server.udp_socket, &mut peek_buf);
        assert_eq!(peek_len, reg_packet.len());
        server
            .discover_udp_peer(tcp_peer, &mut udp_peer, Some(&mut host_c2h))
            .unwrap();
        assert_eq!(
            udp_peer,
            Some(client_addr),
            "valid registration must register legitimate client endpoint"
        );

        // Stage 3: Controlled frame delivery to registered valid socket
        let frame_hdr = PacketHeader::new(PacketType::FrameHeader, 1, 1000, 0);
        let frame_payload = b"controlled-test-frame-content";
        let sealed_frame = host_h2c.seal_datagram(&frame_hdr, frame_payload).unwrap();
        server
            .udp_socket
            .send_to(&sealed_frame, udp_peer.unwrap())
            .unwrap();

        let mut recv_buf = [0_u8; 1024];
        let (recv_len, from_addr) = client_udp.recv_from(&mut recv_buf).unwrap();
        assert_eq!(from_addr, host_udp_addr);
        let (opened_hdr, opened_payload) = client_h2c.open_datagram(&recv_buf[..recv_len]).unwrap();
        assert_eq!(opened_hdr.packet_type, PacketType::FrameHeader);
        assert_eq!(opened_payload, frame_payload);

        // Attacker received nothing
        let mut attacker_buf = [0_u8; 1024];
        assert!(
            matches!(attacker_udp.recv_from(&mut attacker_buf), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock)
        );

        // Stage 4: Post-registration hijack attempt from attacker socket
        let hijack_hdr = PacketHeader::new(PacketType::Ping, 4, 1000, 0);
        let hijack_packet = client_c2h.seal_datagram(&hijack_hdr, &[]).unwrap();
        attacker_udp.send_to(&hijack_packet, host_udp_addr).unwrap();
        let (peek_len, _) = peek_datagram(&server.udp_socket, &mut peek_buf);
        assert_eq!(peek_len, hijack_packet.len());
        server
            .discover_udp_peer(tcp_peer, &mut udp_peer, Some(&mut host_c2h))
            .unwrap();
        assert_eq!(
            udp_peer,
            Some(client_addr),
            "post-registration packet must NOT change registered endpoint"
        );
        assert!(
            matches!(server.udp_socket.recv_from(&mut check_buf), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock),
            "hijack packet must have been consumed from socket"
        );

        // Another frame sent still reaches legitimate client, not attacker
        let frame_hdr2 = PacketHeader::new(PacketType::FrameHeader, 2, 2000, 0);
        let sealed_frame2 = host_h2c.seal_datagram(&frame_hdr2, frame_payload).unwrap();
        server
            .udp_socket
            .send_to(&sealed_frame2, udp_peer.unwrap())
            .unwrap();

        let (recv_len2, _) = client_udp.recv_from(&mut recv_buf).unwrap();
        let (opened_hdr2, _) = client_h2c.open_datagram(&recv_buf[..recv_len2]).unwrap();
        assert_eq!(opened_hdr2.sequence, 2);
        assert!(
            matches!(attacker_udp.recv_from(&mut attacker_buf), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn test_udp_registration_rejects_prior_session_datagram() {
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let (consent_tx, _consent_rx) = mpsc::channel();
        let config = test_config(store, consent_tx);
        let server = HostServer::bind(config).unwrap();
        let host_udp_addr = server.udp_socket.local_addr().unwrap();

        let key = [0x77; 32];
        let current_salt = [0x88; 16];
        let prior_salt = [0x11; 16];

        // Independent client-send with prior salt and host-receive with current salt
        let mut prior_client_c2h =
            DatagramCipher::derive(&key, &prior_salt, Direction::ClientToHost).unwrap();
        let mut host_c2h =
            DatagramCipher::derive(&key, &current_salt, Direction::ClientToHost).unwrap();

        let client_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        client_udp.set_nonblocking(true).unwrap();
        let client_addr = client_udp.local_addr().unwrap();

        let tcp_peer = client_addr;
        let mut udp_peer: Option<SocketAddr> = None;

        // Packet sealed with prior session's salt
        let reg_hdr = PacketHeader::new(PacketType::Ping, 0, 1000, 0);
        let prior_packet = prior_client_c2h.seal_datagram(&reg_hdr, &[]).unwrap();
        client_udp.send_to(&prior_packet, host_udp_addr).unwrap();

        // Prove packet arrived in host socket queue
        let mut peek_buf = [0_u8; 2048];
        let (peek_len, peek_from) = peek_datagram(&server.udp_socket, &mut peek_buf);
        assert_eq!(peek_len, prior_packet.len());
        assert_eq!(peek_from, client_addr);

        server
            .discover_udp_peer(tcp_peer, &mut udp_peer, Some(&mut host_c2h))
            .unwrap();
        assert_eq!(
            udp_peer, None,
            "datagram from prior session must not register endpoint"
        );

        // Prove packet was consumed and discarded by discover_udp_peer
        let mut check_buf = [0_u8; 64];
        assert!(
            matches!(server.udp_socket.recv_from(&mut check_buf), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock),
            "prior-session packet must have been consumed from socket"
        );
    }

    #[test]
    fn test_udp_host_rejects_client_missing_authenticated_registration_capability() {
        let directory = tempdir().unwrap();
        let store = PairingStore::new(directory.path().join("pairing-keys.json"));
        let key = [0x33; 32];
        store
            .save(PairingRecord {
                id: "PAIR_TEST".into(),
                name: "client-test".into(),
                key,
                added_at_unix_ms: 0,
            })
            .unwrap();

        let (consent_tx, _consent_rx) = mpsc::channel();
        let config = test_config(store, consent_tx);
        let media_starts = Arc::new(AtomicUsize::new(0));
        let server = HostServer::bind_with_media(
            config,
            Arc::new(TrackingMediaSource {
                started_count: Arc::clone(&media_starts),
            }),
        )
        .unwrap();
        let tcp_addr = server.tcp_addr().unwrap();
        let server_thread = thread::spawn(move || server.serve_n(1));

        let client = TlsPskClient::new(PskIdentity::pairing("PAIR_TEST", &key).unwrap()).unwrap();
        let mut tcp = client.connect(tcp_addr).unwrap();
        tcp.ssl_stream()
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        // Client sends handshake with Capabilities::empty() (missing AUTHENTICATED_UDP_REGISTRATION)
        let handshake = Handshake {
            name: "test-client".into(),
            width: 0,
            height: 0,
            scale: 1.0,
            version: PROTOCOL_VERSION,
            capabilities: Capabilities::empty(),
            pairing_id: "PAIR_TEST".into(),
            session_salt: [0x55; 16],
        };
        let mut packet = PacketHeader::new(PacketType::Handshake, 0, 0, 0)
            .encode()
            .unwrap();
        packet.extend_from_slice(&handshake.encode().unwrap());
        tcp.write_frame(&packet).unwrap();

        // Host must terminate without sending HandshakeAck and without starting media
        let read_res = tcp.read_frame();
        if let Ok(frame) = read_res {
            let (hdr, _) = decode_tcp_packet(&frame);
            assert_ne!(
                hdr.packet_type,
                PacketType::HandshakeAck,
                "host must NOT send HandshakeAck to client lacking authenticated UDP registration capability"
            );
        }
        let server_res = server_thread.join().unwrap();
        assert!(
            server_res.is_err(),
            "server must return error when client lacks capability"
        );
        assert_eq!(
            media_starts.load(Ordering::SeqCst),
            0,
            "media capture must NOT start when client lacks capability"
        );
    }
}
