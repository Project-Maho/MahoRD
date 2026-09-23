// allow: SIZE_OK — core client session state machine and network driver
use std::{
    collections::VecDeque,
    io,
    net::{SocketAddr, ToSocketAddrs, UdpSocket},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use maho_net::{
    DatagramCipher, DatagramError, Direction, PskIdentity, TlsPskClient, TlsPskError, TlsPskStream,
};
use maho_proto::{
    AudioFragment, BitrateAdjust, Capabilities, ClipboardSyncDirection, ClipboardSyncOrigin,
    ClipboardSyncUpdate, ControlMessage, CursorUpdate, FrameChunk, FrameHeader, Handshake,
    InputEvent, PacketHeader, PacketType, PairingGrant, PairingReject, PairingRejectReason,
    PairingRequest, WireCodec, PROTOCOL_VERSION,
};
use rand::{rngs::OsRng, RngCore};
use thiserror::Error;

use crate::{
    AssembledFrame, AudioFragmentReassembler, CursorState, FrameAssembler, PairingRecord,
    PairingStore, PairingStoreError, PlatformClipboard,
};

#[cfg(test)]
#[path = "tcp_write_tests.rs"]
mod tcp_write_tests;

#[path = "receiver_trace.rs"]
mod receiver_trace;
pub use receiver_trace::*;

pub const DEFAULT_TCP_PORT: u16 = 19_730;
pub const DEFAULT_UDP_PORT: u16 = 19_731;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
pub const HANDSHAKE_ACK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Disconnected,
    Connecting,
    AwaitingPairing,
    AwaitingHandshakeAck,
    Ready,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub host: String,
    pub tcp_port: u16,
    pub udp_port: u16,
    pub client_name: String,
    pub capabilities: Capabilities,
    pub pairing_store_path: Option<PathBuf>,
    pub connect_timeout: Duration,
    pub handshake_ack_timeout: Duration,
}

impl SessionConfig {
    pub fn direct(host: impl Into<String>, client_name: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            tcp_port: DEFAULT_TCP_PORT,
            udp_port: DEFAULT_UDP_PORT,
            client_name: client_name.into(),
            capabilities: Capabilities::STREAM_CONFIGURATION
                | Capabilities::TEXT_CLIPBOARD_SYNC
                | Capabilities::AUTHENTICATED_UDP_REGISTRATION,
            pairing_store_path: None,
            connect_timeout: CONNECT_TIMEOUT,
            handshake_ack_timeout: HANDSHAKE_ACK_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReadySession {
    pub pairing: PairingRecord,
    pub server: Handshake,
    pub session_salt: [u8; 16],
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Frame(AssembledFrame),
    Audio(Vec<u8>),
    Cursor(CursorState),
    Clipboard(String),
    StreamConfig(ControlMessage),
    InputAck {
        sequence: u32,
        success: bool,
        error_code: u8,
    },
    Ping,
    Ignored,
}

/// Whether a session failure is worth re-handshaking for.
///
/// The Windows host service replaces its session worker whenever the console
/// switches desktop (entering or leaving the logon, lock or UAC secure desktop).
/// That terminates the worker holding this client's control channel and media
/// session, so a connected client goes silent even though the host is healthy
/// and already serving a new worker. Those failures are recoverable by
/// reconnecting with the stored pairing; credential and addressing failures are
/// not, and retrying them would spin.
pub fn should_reconnect(error: &SessionError) -> bool {
    matches!(
        error,
        SessionError::NotReady
            | SessionError::TcpRuntimeStopped
            | SessionError::TcpRuntimePanicked(_)
            | SessionError::HandshakeAckTimeout
    )
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("session is not ready")]
    NotReady,
    #[error("pairing rejected: {0:?}")]
    PairingRejected(PairingRejectReason),
    #[error("pairing record {0} was not found")]
    PairingNotFound(String),
    #[error("expected {expected:?}, received {actual:?}")]
    UnexpectedPacket {
        expected: PacketType,
        actual: PacketType,
    },
    #[error("handshake acknowledgement timed out")]
    HandshakeAckTimeout,
    #[error("session lock was poisoned")]
    Poisoned,
    #[error("address resolution returned no endpoints")]
    NoAddress,
    #[error("TCP runtime worker panicked: {0}")]
    TcpRuntimePanicked(String),
    #[error("TCP runtime stopped")]
    TcpRuntimeStopped,
    #[error("TCP runtime already owns the connection; stop it before reconnecting")]
    TcpRuntimeAlreadyRunning,
    #[error("protocol error: {0}")]
    Protocol(#[from] maho_proto::CodecError),
    #[error("TLS transport error: {0}")]
    Tls(#[from] TlsPskError),
    #[error("UDP transport error: {0}")]
    Datagram(#[from] DatagramError),
    #[error("pairing store error: {0}")]
    PairingStore(#[from] PairingStoreError),
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("host lacks authenticated UDP registration capability")]
    MissingAuthenticatedRegistration,
    #[error("operation cancelled")]
    Cancelled,
}

struct SessionStateInner {
    state: SessionState,
    next_tcp_sequence: u32,
    next_udp_sequence: u32,
    pairing: Option<PairingRecord>,
    server: Option<Handshake>,
    session_salt: Option<[u8; 16]>,
    frames: FrameAssembler,
    receiver: crate::receiver_stats::ReceiverStats,
    audio: AudioFragmentReassembler,
    cursor: CursorState,
}

impl Default for SessionStateInner {
    fn default() -> Self {
        Self {
            state: SessionState::Disconnected,
            next_tcp_sequence: 0,
            next_udp_sequence: 0,
            pairing: None,
            server: None,
            session_salt: None,
            frames: FrameAssembler::default(),
            receiver: crate::receiver_stats::ReceiverStats::default(),
            audio: AudioFragmentReassembler::default(),
            cursor: CursorState::default(),
        }
    }
}

/// Thread-safe client session orchestrator. Network reads are explicit so callers can own their runtime.
#[derive(Clone)]
pub struct SessionInterruptHandle {
    socket: Arc<Mutex<Option<std::net::TcpStream>>>,
    cancelled: Arc<AtomicBool>,
    udp: Arc<Mutex<Option<Arc<UdpTransport>>>>,
}

impl SessionInterruptHandle {
    pub fn interrupt(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Ok(guard) = self.socket.lock() {
            if let Some(ref socket) = *guard {
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
        }
        if let Ok(guard) = self.udp.lock() {
            if let Some(udp) = guard.as_ref() {
                udp.cancel.send_replace(true);
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
pub struct ClientSession {
    config: SessionConfig,
    store: PairingStore,
    state: Arc<Mutex<SessionStateInner>>,
    tcp: Arc<Mutex<Option<TlsPskStream<std::net::TcpStream>>>>,
    interrupt_socket: Arc<Mutex<Option<std::net::TcpStream>>>,
    cancelled: Arc<AtomicBool>,
    // Lock before state/tcp when admitting sends or changing runtime ownership.
    tcp_runtime: Arc<Mutex<Option<Arc<tokio::sync::Notify>>>>,
    udp: Arc<Mutex<Option<Arc<UdpTransport>>>>,
    udp_send: Arc<Mutex<Option<DatagramCipher>>>,
    udp_receive: Arc<Mutex<Option<DatagramCipher>>>,
    last_input_ack: Arc<Mutex<Option<(u32, bool, u8)>>>,
    udp_registered: Arc<AtomicBool>,
    registration_attempts: Arc<AtomicU32>,
    relay_endpoint: Arc<Mutex<Option<(String, u16, u16)>>>,
    relay_stop: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    trace: ReceiverTrace,
    #[cfg(test)]
    tcp_wait: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    #[cfg(test)]
    tcp_write_wait: Arc<Mutex<Option<mpsc::Sender<()>>>>,
}

impl ClientSession {
    pub fn new(config: SessionConfig) -> Result<Self, SessionError> {
        let store = match config.pairing_store_path.clone() {
            Some(path) => PairingStore::new(path),
            None => PairingStore::open_default()?,
        };
        Ok(Self {
            config,
            store,
            state: Arc::new(Mutex::new(SessionStateInner::default())),
            tcp: Arc::new(Mutex::new(None)),
            interrupt_socket: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(AtomicBool::new(false)),
            tcp_runtime: Arc::new(Mutex::new(None)),
            udp: Arc::new(Mutex::new(None)),
            udp_send: Arc::new(Mutex::new(None)),
            udp_receive: Arc::new(Mutex::new(None)),
            last_input_ack: Arc::new(Mutex::new(None)),
            udp_registered: Arc::new(AtomicBool::new(false)),
            registration_attempts: Arc::new(AtomicU32::new(0)),
            relay_endpoint: Arc::new(Mutex::new(None)),
            relay_stop: Arc::new(Mutex::new(None)),
            trace: std::env::var_os("MAHO_RECEIVER_TRACE_PATH")
                .map(|path| ReceiverTrace::at_path(path.into()))
                .unwrap_or_default(),
            #[cfg(test)]
            tcp_wait: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            tcp_write_wait: Arc::new(Mutex::new(None)),
        })
    }

    pub fn state(&self) -> Result<SessionState, SessionError> {
        Ok(self.state.lock().map_err(|_| SessionError::Poisoned)?.state)
    }

    pub fn receiver_trace(&self) -> ReceiverTrace {
        self.trace.clone()
    }

    /// Returns the most recently received input acknowledgement (sequence, success, error_code).
    pub fn last_input_ack(&self) -> Option<(u32, bool, u8)> {
        *self.last_input_ack.lock().ok()?
    }

    /// Configure before connecting; clones made later share this endpoint trace.
    pub fn set_receiver_trace(&mut self, trace: ReceiverTrace) {
        self.trace = trace;
    }

    /// Returns bounded receiver observations at the current client monotonic time.
    pub fn receiver_snapshot(&self) -> Result<crate::ReceiverSnapshot, SessionError> {
        self.receiver_snapshot_at(std::time::Instant::now())
    }

    /// Explicit-clock snapshot; use nondecreasing times on the receiving clock.
    /// Reads finalize expired packet gaps, but never infer trailing traffic.
    pub fn receiver_snapshot_at(
        &self,
        now: std::time::Instant,
    ) -> Result<crate::ReceiverSnapshot, SessionError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| SessionError::Poisoned)?
            .receiver
            .snapshot(now))
    }

    pub fn pair_with_pin(&self, pin: &str) -> Result<ReadySession, SessionError> {
        let psk = PskIdentity::bootstrap(pin)?;
        self.connect_with_psk(psk, SessionState::AwaitingPairing)?;
        self.send_packet(
            PacketType::PairingRequest,
            &PairingRequest {
                name: self.config.client_name.clone(),
            }
            .encode()?,
        )?;
        let (header, payload) = self.read_tcp_packet()?;
        let pairing = match header.packet_type {
            PacketType::PairingGrant => {
                let grant = PairingGrant::decode(&payload)?;
                let record = PairingRecord {
                    id: grant.pairing_id,
                    name: grant.host_name,
                    key: grant.key.to_vec(),
                    added_at_unix_ms: current_unix_ms(),
                    last_endpoint: None,
                    endpoint_aliases: Vec::new(),
                    relay_url: None,
                    relay_host_id: None,
                };
                self.store.save(record.clone())?;
                record
            }
            PacketType::PairingReject => {
                return Err(SessionError::PairingRejected(
                    PairingReject::decode(&payload)?.reason,
                ));
            }
            actual => {
                return Err(SessionError::UnexpectedPacket {
                    expected: PacketType::PairingGrant,
                    actual,
                });
            }
        };
        self.begin_handshake(pairing)
    }

    pub fn reconnect(&self, pairing_id: &str) -> Result<ReadySession, SessionError> {
        let pairing = self
            .store
            .load(pairing_id)?
            .ok_or_else(|| SessionError::PairingNotFound(pairing_id.to_owned()))?;
        self.connect_with_pairing(pairing)
    }

    /// Connects directly using an explicit pairing record without consulting the pairing store.
    pub fn connect_with_pairing(
        &self,
        pairing: PairingRecord,
    ) -> Result<ReadySession, SessionError> {
        let psk = PskIdentity::pairing(&pairing.id, &pairing.key)?;
        *self
            .relay_endpoint
            .lock()
            .map_err(|_| SessionError::Poisoned)? = None;
        match self.connect_with_psk(psk.clone(), SessionState::AwaitingHandshakeAck) {
            Ok(()) => self.begin_handshake(pairing),
            Err(direct_error) => {
                if let Some(endpoint) = self.open_relay_fallback(&pairing) {
                    *self
                        .relay_endpoint
                        .lock()
                        .map_err(|_| SessionError::Poisoned)? = Some(endpoint);
                    self.connect_with_psk(psk, SessionState::AwaitingHandshakeAck)?;
                    self.begin_handshake(pairing)
                } else {
                    Err(direct_error)
                }
            }
        }
    }

    fn stop_relay_bridge(&self) {
        if let Ok(mut guard) = self.relay_stop.lock() {
            if let Some(stop) = guard.take() {
                let _ = stop.send(());
            }
        }
        if let Ok(mut endpoint) = self.relay_endpoint.lock() {
            *endpoint = None;
        }
    }

    fn open_relay_fallback(&self, pairing: &PairingRecord) -> Option<(String, u16, u16)> {
        let url = pairing.relay_url.clone()?;
        let secret = std::env::var("RELAY_AUTH_SECRET").ok()?;
        if secret.is_empty() {
            tracing::warn!(relay = %url, "relay fallback skipped: RELAY_AUTH_SECRET not set");
            return None;
        }
        let host_id = pairing.relay_host_id.clone().unwrap_or_else(|| {
            pairing
                .name
                .chars()
                .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
                .take(128)
                .collect()
        });
        if host_id.is_empty() {
            return None;
        }
        self.stop_relay_bridge();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        *self.relay_stop.lock().ok()? = Some(stop_tx);
        let url_for_thread = url;
        let secret_for_thread = secret.into_bytes();
        let host_for_thread = host_id;
        if std::thread::Builder::new()
            .name("maho-client-relay".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                runtime.block_on(async move {
                    match crate::relay::start_client_bridge(
                        &url_for_thread,
                        &host_for_thread,
                        &secret_for_thread,
                    )
                    .await
                    {
                        Ok(bridge) => {
                            tracing::info!(
                                local_tcp = %bridge.local_tcp,
                                local_udp = %bridge.local_udp,
                                host_id = %host_for_thread,
                                "relay fallback bridge established"
                            );
                            let _ = ready_tx.send(Ok((
                                bridge.local_tcp.ip().to_string(),
                                bridge.local_tcp.port(),
                                bridge.local_udp.port(),
                            )));
                            let _ = tokio::task::spawn_blocking(move || stop_rx.recv()).await;
                        }
                        Err(error) => {
                            tracing::warn!(%error, "relay fallback bridge failed");
                            let _ = ready_tx.send(Err(error));
                        }
                    }
                });
            })
            .is_err()
        {
            return None;
        }
        ready_rx.recv().ok().and_then(Result::ok)
    }

    pub fn set_udp_read_timeout(&self, timeout: Option<Duration>) -> Result<(), SessionError> {
        let udp = self.udp.lock().map_err(|_| SessionError::Poisoned)?;
        if let Some(udp) = udp.as_ref() {
            *udp.timeout.lock().map_err(|_| SessionError::Poisoned)? = timeout;
        }
        Ok(())
    }

    pub fn send_input(&self, event: InputEvent) -> Result<(), SessionError> {
        self.send_ready_packet(PacketType::InputEvent, &event.encode()?)
    }

    pub fn send_control(&self, control: ControlMessage) -> Result<(), SessionError> {
        self.send_ready_packet(PacketType::Control, &control.encode()?)
    }

    /// Resends the authenticated UDP registration ping with a fresh AEAD nonce.
    pub fn send_udp_registration(&self) -> Result<(), SessionError> {
        let udp = {
            let socket_guard = self.udp.lock().map_err(|_| SessionError::Poisoned)?;
            socket_guard
                .as_ref()
                .cloned()
                .ok_or(SessionError::NotReady)?
        };
        let mut cipher_guard = self.udp_send.lock().map_err(|_| SessionError::Poisoned)?;
        let cipher = cipher_guard.as_mut().ok_or(SessionError::NotReady)?;
        let reg_header = PacketHeader::new(PacketType::Ping, 0, current_unix_ms() as u32, 0);
        let reg_packet = cipher.seal_datagram(&reg_header, &[])?;
        udp.socket.send(&reg_packet)?;
        Ok(())
    }

    pub fn send_udp(&self, packet_type: PacketType, payload: &[u8]) -> Result<(), SessionError> {
        let (sequence, udp) = {
            let mut state = self.state.lock().map_err(|_| SessionError::Poisoned)?;
            if state.state != SessionState::Ready {
                return Err(SessionError::NotReady);
            }
            state.next_udp_sequence = state.next_udp_sequence.wrapping_add(1);
            let udp_guard = self.udp.lock().map_err(|_| SessionError::Poisoned)?;
            let udp = udp_guard.as_ref().cloned().ok_or(SessionError::NotReady)?;
            (state.next_udp_sequence, udp)
        };
        let header = PacketHeader::new(packet_type, sequence, current_unix_ms() as u32, 0);
        let mut cipher_guard = self.udp_send.lock().map_err(|_| SessionError::Poisoned)?;
        let datagram = cipher_guard
            .as_mut()
            .ok_or(SessionError::NotReady)?
            .seal_datagram(&header, payload)?;
        drop(cipher_guard);
        udp.socket.send(&datagram)?;
        Ok(())
    }

    pub fn send_bitrate_adjust(&self, target_bitrate: i32) -> Result<(), SessionError> {
        self.send_control(ControlMessage::BitrateAdjust(BitrateAdjust {
            target_bitrate,
        }))
    }

    pub fn evaluate_abr(
        &self,
        abr: &mut crate::AbrController,
        now: std::time::Instant,
    ) -> Result<Option<i32>, SessionError> {
        let loss_ratio = self
            .state
            .lock()
            .map_err(|_| SessionError::Poisoned)?
            .frames
            .loss_ratio(now);
        let target = abr.evaluate(loss_ratio, now);
        if let Some(target) = target {
            self.send_bitrate_adjust(target)?;
        }
        Ok(target)
    }

    pub fn send_clipboard_text(&self, text: String) -> Result<(), SessionError> {
        self.send_control(ControlMessage::ClipboardSyncUpdate(ClipboardSyncUpdate {
            request_id: 0,
            direction: ClipboardSyncDirection::ClientToHost,
            origin: ClipboardSyncOrigin::LocalPasteboard,
            text,
        }))
    }

    pub fn request_stream_config(
        &self,
        request_id: u32,
        desired: maho_proto::StreamConfiguration,
    ) -> Result<(), SessionError> {
        self.send_control(ControlMessage::StreamConfigRequest(
            maho_proto::StreamConfigurationRequest {
                request_id,
                desired,
            },
        ))
    }

    pub fn apply_remote_clipboard<C: PlatformClipboard>(
        &self,
        clipboard: &C,
        text: &str,
    ) -> Result<u64, SessionError> {
        clipboard
            .set_text(text)
            .map_err(|error| SessionError::Io(io::Error::other(error.to_string())))
    }

    pub fn receive_tcp_event(&self) -> Result<SessionEvent, SessionError> {
        let (header, payload) = self.read_tcp_packet()?;
        self.handle_packet(header, payload, None)
    }

    /// Starts the sole TCP readiness owner. Sends admit ordered frames to a
    /// bounded queue; saturation is reported as WouldBlock, never dropped.
    /// Stopping closes TCP and cancels all pending frames. Reconnect only after
    /// this runtime has stopped; a partial encrypted connection is not reusable.
    pub fn spawn_tcp_runtime(&self) -> Result<SessionRuntime, SessionError> {
        if self.state()? != SessionState::Ready {
            return Err(SessionError::NotReady);
        }
        let readiness_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()?;
        let mut owner = self
            .tcp_runtime
            .lock()
            .map_err(|_| SessionError::Poisoned)?;
        if owner.is_some() {
            return Err(SessionError::TcpRuntimeAlreadyRunning);
        }
        let readiness = {
            let mut tcp = self.tcp.lock().map_err(|_| SessionError::Poisoned)?;
            let socket = tcp
                .as_mut()
                .ok_or(SessionError::NotReady)?
                .ssl_stream()
                .get_ref();
            let observer = socket.try_clone()?;
            socket.set_nonblocking(true)?;
            let registered = {
                let _entered = readiness_runtime.enter();
                tokio::net::TcpStream::from_std(observer)
            };
            match registered {
                Ok(registered) => registered,
                Err(error) => {
                    socket.set_nonblocking(false)?;
                    return Err(error.into());
                }
            }
        };
        let wake = Arc::new(tokio::sync::Notify::new());
        *owner = Some(wake.clone());
        drop(owner);
        let session = self.clone();
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
        let event_rx = RuntimeEvents::default();
        let event_tx = event_rx.clone();
        let worker = thread::spawn(move || {
            readiness_runtime.block_on(async { loop {
            if stop_rx.try_recv().is_ok() { break; }
            let mut interest = tokio::io::Interest::READABLE;
            let mut progressed = false;
            let result = {
                let mut tcp = session.tcp.lock().map_err(|_| SessionError::Poisoned);
                match tcp.as_mut() {
                    Ok(tcp) => match tcp.as_mut() {
                        Some(stream) => {
                            let written = if stream.has_pending_frames() {
                                match stream.write_frame_step() {
                                    Ok(()) => { progressed = true; Ok(()) }
                                    Err(TlsPskError::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                                        #[cfg(test)]
                                        if let Some(wait) = session.tcp_write_wait.lock().unwrap().take() { wait.send(()).unwrap(); }
                                        if !stream.write_needs_read() { interest |= tokio::io::Interest::WRITABLE; }
                                        Ok(())
                                    }
                                    Err(error) => Err(SessionError::Tls(error)),
                                }
                            } else { Ok(()) };
                            // Resume SSL_write WANT_READ with SSL_write, not SSL_read.
                            if stream.has_pending_frames() && stream.write_needs_read() {
                                written.and(Err(SessionError::Tls(TlsPskError::Io(io::ErrorKind::WouldBlock.into()))))
                            } else {
                                written.and_then(|()| {
                                    let result = stream.read_frame_step().map_err(SessionError::from);
                                    if stream.read_needs_write() { interest |= tokio::io::Interest::WRITABLE; }
                                    result
                                })
                            }
                        }
                        None => Err(SessionError::NotReady),
                    },
                    Err(_) => Err(SessionError::Poisoned),
                }
            };
            let event = match result {
                Ok(Some(frame)) => {
                    if frame.len() < PacketHeader::SIZE {
                        Err(SessionError::Io(io::Error::from(io::ErrorKind::InvalidData)))
                    } else {
                        PacketHeader::decode(&frame[..PacketHeader::SIZE]).map_err(SessionError::from)
                            .and_then(|header| session.handle_packet(header, frame[PacketHeader::SIZE..].to_vec(), None))
                    }
                }
                Ok(None) => continue,
                Err(SessionError::Tls(TlsPskError::Io(error))) if error.kind() == io::ErrorKind::WouldBlock => {
                    if progressed { continue; }
                    #[cfg(test)]
                    if let Some(wait) = session.tcp_wait.lock().unwrap().take() { wait.send(()).unwrap(); }
                    tokio::select! {
                        biased;
                        _ = &mut stop_rx => break,
                        _ = wake.notified() => {},
                        ready = readiness.ready(interest) => {
                            if let Err(error) = ready { event_tx.push(Err(SessionError::Io(error))); break; }
                            let _: io::Result<()> = readiness.try_io(interest, || Err(io::ErrorKind::WouldBlock.into()));
                        }
                    }
                    continue;
                }
                Err(error) => Err(error),
            };
            if stop_rx.try_recv().is_ok() { break; }
            match event {
                Ok(SessionEvent::Ignored) => {}
                Ok(event) => {
                    event_tx.push(Ok(event));
                }
                Err(error) => {
                    event_tx.push(Err(error));
                    break;
                }
            }
        }});
            // Cancellation is terminal for this TCP generation. Drop the queue
            // and socket together, never reset a partial stream to blocking mode.
            let cleanup = (|| -> Result<(), SessionError> {
                let mut owner = session
                    .tcp_runtime
                    .lock()
                    .map_err(|_| SessionError::Poisoned)?;
                let mut tcp = session.tcp.lock().map_err(|_| SessionError::Poisoned)?;
                let result = tcp.take().map(|tcp| {
                    tcp.ssl_stream()
                        .get_ref()
                        .shutdown(std::net::Shutdown::Both)
                });
                *owner = None;
                if let Ok(mut state) = session.state.lock() {
                    state.state = SessionState::Disconnected;
                }
                match result {
                    Some(Err(error)) if error.kind() != io::ErrorKind::NotConnected => {
                        Err(error.into())
                    }
                    _ => Ok(()),
                }
            })();
            if let Err(error) = cleanup {
                event_tx.push(Err(error));
            }
            event_tx.close();
        });
        Ok(SessionRuntime {
            events: event_rx,
            stop: Some(stop_tx),
            worker: Some(worker),
        })
    }

    pub fn receive_udp_event(&self) -> Result<SessionEvent, SessionError> {
        self.receive_udp_event_clock(None)
    }

    /// Real socket/authentication ingress with an explicit receive clock for replay tests.
    pub fn receive_udp_event_at(
        &self,
        at: std::time::Instant,
    ) -> Result<SessionEvent, SessionError> {
        self.receive_udp_event_clock(Some(at))
    }

    fn receive_udp_event_clock(
        &self,
        at: Option<std::time::Instant>,
    ) -> Result<SessionEvent, SessionError> {
        let udp = {
            let socket_guard = self.udp.lock().map_err(|_| SessionError::Poisoned)?;
            socket_guard
                .as_ref()
                .cloned()
                .ok_or(SessionError::NotReady)?
        };
        let (datagram, count) = match udp.receive() {
            Ok(received) => received,
            Err(error) => {
                let mut record = ReceiverTraceRecord::new(ReceiverTraceEvent::ReceiveError);
                record.result = match &error {
                    SessionError::Io(error) => error.raw_os_error().unwrap_or(-1),
                    _ => -1,
                };
                self.trace.record(std::time::Instant::now(), record);
                if let SessionError::Io(ref io_err) = error {
                    if matches!(
                        io_err.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) && !self.udp_registered.load(Ordering::Relaxed)
                    {
                        let attempts = self.registration_attempts.fetch_add(1, Ordering::Relaxed);
                        if attempts < 10 {
                            let _ = self.send_udp_registration();
                        }
                    }
                }
                return Err(error);
            }
        };
        let received_at = at.unwrap_or_else(std::time::Instant::now);
        let mut record = ReceiverTraceRecord::new(ReceiverTraceEvent::Receive);
        record.bytes = count as u64;
        if let Some(bytes) = datagram[..count].get(..PacketHeader::SIZE) {
            if let Ok(header) = PacketHeader::decode(bytes) {
                record.sequence = Some(header.sequence);
                record.kind = Some(header.packet_type as u8);
            }
        }
        self.trace.record(received_at, record);
        let mut cipher_guard = self
            .udp_receive
            .lock()
            .map_err(|_| SessionError::Poisoned)?;
        let opened = cipher_guard
            .as_mut()
            .ok_or(SessionError::NotReady)?
            .open_datagram(&datagram[..count]);
        let (header, payload) = match opened {
            Ok(packet) => packet,
            Err(error) => {
                record.event = ReceiverTraceEvent::AuthRejected;
                record.result = match error {
                    DatagramError::Authentication => 1,
                    DatagramError::Replay => 2,
                    DatagramError::Truncated => 3,
                    DatagramError::WrongDirection => 4,
                    DatagramError::InvalidHeader(_) => 5,
                    DatagramError::InvalidMasterKey => 6,
                    DatagramError::InvalidSessionSalt => 7,
                    DatagramError::CounterExhausted => 8,
                };
                self.trace.record(received_at, record);
                return Err(error.into());
            }
        };
        record.event = ReceiverTraceEvent::Authenticated;
        self.udp_registered.store(true, Ordering::Relaxed);
        record.frame = if self.trace.enabled() {
            match header.packet_type {
                PacketType::FrameHeader => FrameHeader::decode(&payload).ok().map(|h| h.frame_id),
                PacketType::FrameChunk => payload
                    .get(..4)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(u32::from_le_bytes),
                PacketType::Ping => {
                    maho_proto::TimestampStats::decode(&payload).map(|s| s.frame_id)
                }
                _ => None,
            }
        } else {
            None
        };
        self.trace.record(received_at, record);
        // Keep this cipher generation pinned until the authenticated observation
        // is consumed; a new handshake cannot install its cipher in between.
        self.handle_packet(header, payload, Some((received_at, count)))
    }

    pub fn interrupt_handle(&self) -> SessionInterruptHandle {
        SessionInterruptHandle {
            socket: self.interrupt_socket.clone(),
            cancelled: self.cancelled.clone(),
            udp: self.udp.clone(),
        }
    }

    pub fn interrupt(&self) {
        self.interrupt_handle().interrupt();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn disconnect(&self) -> Result<(), SessionError> {
        self.stop_relay_bridge();
        self.interrupt();
        let trace_result = self
            .receiver_snapshot()
            .and_then(|snapshot| self.trace.write(&snapshot).map_err(SessionError::Io));
        {
            let mut state = self.state.lock().map_err(|_| SessionError::Poisoned)?;
            state.state = SessionState::Disconnected;
            state.frames.clear();
            state.audio.clear();
            state.receiver = crate::receiver_stats::ReceiverStats::default();
        }
        if let Some(udp) = self.udp.lock().map_err(|_| SessionError::Poisoned)?.take() {
            udp.cancel.send_replace(true);
        }
        if let Some(tcp) = self.tcp.lock().map_err(|_| SessionError::Poisoned)?.take() {
            match tcp
                .ssl_stream()
                .get_ref()
                .shutdown(std::net::Shutdown::Both)
            {
                Ok(()) => {}
                // The peer may have already closed; cancellation is already satisfied.
                Err(error) if error.kind() == io::ErrorKind::NotConnected => {}
                Err(error) => return Err(SessionError::Io(error)),
            }
        }
        *self.udp_send.lock().map_err(|_| SessionError::Poisoned)? = None;
        *self
            .udp_receive
            .lock()
            .map_err(|_| SessionError::Poisoned)? = None;
        trace_result
    }

    #[cfg(test)]
    fn udp_recv_buffer_ptr(&self) -> Option<usize> {
        let udp_guard = self.udp.lock().ok()?;
        let ptr = *udp_guard.as_ref()?.last_buffer_ptr.lock().ok()?;
        ptr
    }

    #[cfg(test)]
    fn udp_recv_alloc_count(&self) -> Option<usize> {
        let udp_guard = self.udp.lock().ok()?;
        let count = udp_guard
            .as_ref()?
            .alloc_count
            .load(std::sync::atomic::Ordering::SeqCst);
        Some(count)
    }

    #[cfg(test)]
    fn cancel_udp_receive_for_test(&self) {
        if let Ok(guard) = self.udp.lock() {
            if let Some(udp) = guard.as_ref() {
                udp.cancel.send_replace(true);
            }
        }
    }

    #[cfg(test)]
    fn reset_udp_cancel_for_test(&self) {
        if let Ok(guard) = self.udp.lock() {
            if let Some(udp) = guard.as_ref() {
                udp.cancel.send_replace(false);
            }
        }
    }

    #[cfg(test)]
    fn register_udp_entered_hook_for_test(&self, sender: mpsc::Sender<()>) {
        if let Ok(guard) = self.udp.lock() {
            if let Some(udp) = guard.as_ref() {
                *udp.entered.lock().unwrap() = Some(sender);
            }
        }
    }

    fn connect_with_psk(
        &self,
        psk: PskIdentity,
        next_state: SessionState,
    ) -> Result<(), SessionError> {
        let owner = self
            .tcp_runtime
            .lock()
            .map_err(|_| SessionError::Poisoned)?;
        if owner.is_some() {
            return Err(SessionError::TcpRuntimeAlreadyRunning);
        }
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(SessionError::Cancelled);
        }
        {
            let mut state = self.state.lock().map_err(|_| SessionError::Poisoned)?;
            state.state = SessionState::Connecting;
            state.frames.clear();
            state.receiver = crate::receiver_stats::ReceiverStats::default();
        }
        let address = {
            let override_endpoint = self
                .relay_endpoint
                .lock()
                .map_err(|_| SessionError::Poisoned)?;
            if let Some((host, tcp_port, _)) = override_endpoint.as_ref() {
                resolve_one((host.as_str(), *tcp_port))?
            } else {
                resolve_one((&*self.config.host, self.config.tcp_port))?
            }
        };
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(SessionError::Cancelled);
        }
        self.state
            .lock()
            .map_err(|_| SessionError::Poisoned)?
            .receiver
            .set_trace(self.trace.clone());
        self.state
            .lock()
            .map_err(|_| SessionError::Poisoned)?
            .frames
            .set_receiver_trace(self.trace.clone());
        let tcp = std::net::TcpStream::connect_timeout(&address, self.config.connect_timeout)?;
        tcp.set_read_timeout(Some(self.config.connect_timeout))?;
        tcp.set_write_timeout(Some(self.config.connect_timeout))?;
        tcp.set_nodelay(true)?;

        let interrupt_clone = tcp.try_clone()?;
        *self
            .interrupt_socket
            .lock()
            .map_err(|_| SessionError::Poisoned)? = Some(interrupt_clone);

        if self.cancelled.load(Ordering::SeqCst) {
            let _ = tcp.shutdown(std::net::Shutdown::Both);
            return Err(SessionError::Cancelled);
        }

        let stream = match TlsPskClient::new(psk)?.connect_stream(tcp) {
            Ok(s) => s,
            Err(e) => {
                if self.cancelled.load(Ordering::SeqCst) {
                    return Err(SessionError::Cancelled);
                }
                return Err(SessionError::Tls(e));
            }
        };

        if self.cancelled.load(Ordering::SeqCst) {
            let _ = stream
                .ssl_stream()
                .get_ref()
                .shutdown(std::net::Shutdown::Both);
            return Err(SessionError::Cancelled);
        }

        *self.tcp.lock().map_err(|_| SessionError::Poisoned)? = Some(stream);
        let mut state = self.state.lock().map_err(|_| SessionError::Poisoned)?;
        state.state = next_state;
        Ok(())
    }

    fn begin_handshake(&self, pairing: PairingRecord) -> Result<ReadySession, SessionError> {
        let mut salt = [0_u8; 16];
        OsRng.fill_bytes(&mut salt);
        self.trace.begin_session(salt);
        let handshake = Handshake {
            name: self.config.client_name.clone(),
            width: 0,
            height: 0,
            scale: 1.0,
            version: PROTOCOL_VERSION,
            capabilities: self.config.capabilities,
            pairing_id: pairing.id.clone(),
            session_salt: salt,
        };
        {
            let mut state = self.state.lock().map_err(|_| SessionError::Poisoned)?;
            state.pairing = Some(pairing.clone());
            state.session_salt = Some(salt);
            state.state = SessionState::AwaitingHandshakeAck;
            let mut tcp = self.tcp.lock().map_err(|_| SessionError::Poisoned)?;
            if let Some(tcp) = tcp.as_mut() {
                tcp.ssl_stream_mut()
                    .get_ref()
                    .set_read_timeout(Some(self.config.handshake_ack_timeout))?;
            }
        }
        self.send_packet(PacketType::Handshake, &handshake.encode()?)?;
        let (header, payload) = match self.read_tcp_packet() {
            Ok(packet) => packet,
            Err(SessionError::Tls(TlsPskError::Io(error)))
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(SessionError::HandshakeAckTimeout);
            }
            Err(error) => return Err(error),
        };
        if header.packet_type != PacketType::HandshakeAck {
            return Err(SessionError::UnexpectedPacket {
                expected: PacketType::HandshakeAck,
                actual: header.packet_type,
            });
        }
        let server = Handshake::decode(&payload)?;
        if !server
            .capabilities
            .contains(Capabilities::AUTHENTICATED_UDP_REGISTRATION)
        {
            return Err(SessionError::MissingAuthenticatedRegistration);
        }
        let key = pairing.key_array()?;
        let mut udp_send = DatagramCipher::derive(&key, &salt, Direction::ClientToHost)?;
        let udp_receive = DatagramCipher::derive(&key, &salt, Direction::HostToClient)?;
        let udp_address = {
            let override_endpoint = self
                .relay_endpoint
                .lock()
                .map_err(|_| SessionError::Poisoned)?;
            if let Some((host, _, udp_port)) = override_endpoint.as_ref() {
                resolve_one((host.as_str(), *udp_port))?
            } else {
                resolve_one((&*self.config.host, self.config.udp_port))?
            }
        };
        let udp = UdpSocket::bind(if udp_address.is_ipv6() {
            "[::]:0"
        } else {
            "0.0.0.0:0"
        })?;
        let receive_buffer =
            rustix::net::sockopt::set_socket_recv_buffer_size(&udp, 4 * 1024 * 1024);
        let mut record = ReceiverTraceRecord::new(ReceiverTraceEvent::SocketBuffer);
        record.count = 4 * 1024 * 1024;
        record.result = receive_buffer.err().map_or(0, |error| error.raw_os_error());
        match rustix::net::sockopt::socket_recv_buffer_size(&udp) {
            Ok(bytes) => record.bytes = bytes as u64,
            Err(error) => {
                record.sequence = Some(error.raw_os_error() as u32);
            }
        }
        self.trace.record(std::time::Instant::now(), record);
        let _ = rustix::net::sockopt::set_socket_send_buffer_size(&udp, 4 * 1024 * 1024);
        udp.connect(udp_address)?;
        udp.set_read_timeout(Some(HEARTBEAT_INTERVAL * 3))?;
        self.udp_registered.store(false, Ordering::Relaxed);
        self.registration_attempts.store(0, Ordering::Relaxed);
        let reg_header = PacketHeader::new(PacketType::Ping, 0, current_unix_ms() as u32, 0);
        let reg_packet = udp_send.seal_datagram(&reg_header, &[])?;
        let _ = udp.send(&reg_packet);
        {
            let mut tcp = self.tcp.lock().map_err(|_| SessionError::Poisoned)?;
            if let Some(tcp) = tcp.as_mut() {
                tcp.ssl_stream_mut().get_ref().set_read_timeout(None)?;
            }
        }
        let ready = ReadySession {
            pairing: pairing.clone(),
            server: server.clone(),
            session_salt: salt,
        };
        *self.udp.lock().map_err(|_| SessionError::Poisoned)? =
            Some(Arc::new(UdpTransport::new(udp)?));
        *self.udp_send.lock().map_err(|_| SessionError::Poisoned)? = Some(udp_send);
        *self
            .udp_receive
            .lock()
            .map_err(|_| SessionError::Poisoned)? = Some(udp_receive);
        let mut state = self.state.lock().map_err(|_| SessionError::Poisoned)?;
        state.server = Some(server);
        state.state = SessionState::Ready;
        Ok(ready)
    }

    fn send_ready_packet(
        &self,
        packet_type: PacketType,
        payload: &[u8],
    ) -> Result<(), SessionError> {
        if self.state()? != SessionState::Ready {
            return Err(SessionError::NotReady);
        }
        self.send_packet(packet_type, payload)
    }

    fn send_packet(&self, packet_type: PacketType, payload: &[u8]) -> Result<(), SessionError> {
        let owner = self
            .tcp_runtime
            .lock()
            .map_err(|_| SessionError::Poisoned)?;
        let mut state = self.state.lock().map_err(|_| SessionError::Poisoned)?;
        let sequence = state.next_tcp_sequence.wrapping_add(1);
        let header = PacketHeader::new(packet_type, sequence, current_unix_ms() as u32, 0);
        let mut packet = header.encode()?;
        packet.extend_from_slice(payload);
        let mut tcp = self.tcp.lock().map_err(|_| SessionError::Poisoned)?;
        let stream = tcp.as_mut().ok_or(SessionError::NotReady)?;
        if let Some(wake) = owner.as_ref() {
            stream.queue_frame(&packet)?;
            // Preserve immediate delivery when writable (notably Disconnect
            // immediately followed by stop). This never waits: the socket stays
            // nonblocking and only retained, hard-bounded data is attempted.
            let result = stream.flush_pending_frames();
            wake.notify_one();
            match result {
                Ok(()) => {}
                Err(TlsPskError::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
        } else {
            stream.write_frame(&packet)?;
        }
        state.next_tcp_sequence = sequence;
        Ok(())
    }

    fn read_tcp_packet(&self) -> Result<(PacketHeader, Vec<u8>), SessionError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(SessionError::Cancelled);
        }
        let read_result = {
            let mut tcp = self.tcp.lock().map_err(|_| SessionError::Poisoned)?;
            tcp.as_mut().ok_or(SessionError::NotReady)?.read_frame()
        };
        let frame = match read_result {
            Ok(f) => f,
            Err(e) => {
                if self.cancelled.load(Ordering::SeqCst) {
                    return Err(SessionError::Cancelled);
                }
                return Err(e.into());
            }
        };
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(SessionError::Cancelled);
        }
        if frame.len() < PacketHeader::SIZE {
            return Err(SessionError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "TCP packet is shorter than the v3 header",
            )));
        }
        Ok((
            PacketHeader::decode(&frame[..PacketHeader::SIZE])?,
            frame[PacketHeader::SIZE..].to_vec(),
        ))
    }

    fn handle_packet(
        &self,
        header: PacketHeader,
        payload: Vec<u8>,
        udp_observation: Option<(std::time::Instant, usize)>,
    ) -> Result<SessionEvent, SessionError> {
        let mut state = self.state.lock().map_err(|_| SessionError::Poisoned)?;
        if state.state != SessionState::Ready {
            return Err(SessionError::NotReady);
        }
        let from_udp = udp_observation.is_some();
        let received_at = if let Some((at, bytes)) = udp_observation {
            // Only receive_udp_event supplies this after successful authentication.
            // Count transport bytes even if the authenticated media payload is invalid.
            state.receiver.observe_datagram(header.sequence, bytes, at);
            at
        } else {
            std::time::Instant::now()
        };
        match header.packet_type {
            PacketType::FrameHeader | PacketType::FrameChunk if from_udp => {
                let frame = if header.packet_type == PacketType::FrameHeader {
                    state.frames.push_header(
                        FrameHeader::decode(&payload)?,
                        header.timestamp_ms,
                        received_at,
                    )?
                } else {
                    state
                        .frames
                        .push_chunk(FrameChunk::decode(&payload)?, received_at)?
                };
                if let Some(started_at) = state.frames.take_completed_started_at() {
                    // Completion time comes from the same receive clock that
                    // started the assembly: mixing a supplied logical start
                    // with a wall-clock end would invert or skew the interval
                    // under explicit-clock replay ingress.
                    state.receiver.record_assembly(started_at, received_at);
                }
                Ok(frame.map_or(SessionEvent::Ignored, SessionEvent::Frame))
            }
            PacketType::AudioFrame if from_udp => Ok(state
                .audio
                .push(AudioFragment::decode(&payload)?)
                .map_or(SessionEvent::Ignored, SessionEvent::Audio)),
            PacketType::CursorUpdate if from_udp => {
                state.cursor.update(CursorUpdate::decode(&payload)?);
                Ok(SessionEvent::Cursor(state.cursor))
            }
            PacketType::Ping if from_udp => {
                if let Some(stats) = maho_proto::TimestampStats::decode(&payload) {
                    state.receiver.record_host(stats, received_at);
                }
                Ok(SessionEvent::Ping)
            }
            PacketType::Ping => {
                drop(state);
                self.send_control(ControlMessage::Pong)?;
                Ok(SessionEvent::Ping)
            }
            PacketType::Control => match ControlMessage::decode(&payload)? {
                ControlMessage::Ping => {
                    drop(state);
                    self.send_control(ControlMessage::Pong)?;
                    Ok(SessionEvent::Ping)
                }
                ControlMessage::ClipboardSyncUpdate(update) => {
                    Ok(SessionEvent::Clipboard(update.text))
                }
                ControlMessage::InputAck(ack) => {
                    if let Ok(mut lock) = self.last_input_ack.lock() {
                        *lock = Some((ack.sequence, ack.success, ack.error_code));
                    }
                    Ok(SessionEvent::InputAck {
                        sequence: ack.sequence,
                        success: ack.success,
                        error_code: ack.error_code,
                    })
                }
                msg @ (ControlMessage::StreamConfigResponse(_)
                | ControlMessage::StreamConfigReject(_)
                | ControlMessage::StreamConfigError(_)) => Ok(SessionEvent::StreamConfig(msg)),
                _ => Ok(SessionEvent::Ignored),
            },
            _ => Ok(SessionEvent::Ignored),
        }
    }
}

struct UdpTransport {
    socket: UdpSocket,
    runtime: tokio::runtime::Runtime,
    reader: tokio::net::UdpSocket,
    cancel: tokio::sync::watch::Sender<bool>,
    timeout: Mutex<Option<Duration>>,
    buffer: Mutex<Vec<u8>>,
    #[cfg(test)]
    alloc_count: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    last_buffer_ptr: Mutex<Option<usize>>,
    #[cfg(test)]
    entered: Mutex<Option<mpsc::Sender<()>>>,
}

impl UdpTransport {
    fn new(socket: UdpSocket) -> Result<Self, SessionError> {
        let timeout = socket.read_timeout()?;
        socket.set_nonblocking(true)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let reader = {
            let _entered = runtime.enter();
            tokio::net::UdpSocket::from_std(socket.try_clone()?)?
        };
        let (cancel, _) = tokio::sync::watch::channel(false);
        Ok(Self {
            socket,
            runtime,
            reader,
            cancel,
            timeout: Mutex::new(timeout),
            buffer: Mutex::new(Vec::new()),
            #[cfg(test)]
            alloc_count: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            last_buffer_ptr: Mutex::new(None),
            #[cfg(test)]
            entered: Mutex::new(None),
        })
    }

    fn receive(&self) -> Result<(std::sync::MutexGuard<'_, Vec<u8>>, usize), SessionError> {
        let mut datagram = self.buffer.lock().map_err(|_| SessionError::Poisoned)?;
        if datagram.is_empty() {
            datagram.resize(65_536, 0);
            #[cfg(test)]
            self.alloc_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let mut cancelled = self.cancel.subscribe();
        let timeout = *self.timeout.lock().map_err(|_| SessionError::Poisoned)?;
        self.runtime
            .block_on(async {
                if *cancelled.borrow() {
                    return Err(SessionError::NotReady);
                }
                let receive = async {
                    #[cfg(test)]
                    {
                        *self.last_buffer_ptr.lock().unwrap() = Some(datagram.as_ptr() as usize);
                    }
                    let count = self.reader.recv(&mut datagram).await?;
                    Ok::<_, SessionError>(count)
                };
                tokio::pin!(receive);
                #[cfg(test)]
                let entered_opt = self.entered.lock().unwrap().take();
                #[cfg(test)]
                if let Some(entered) = entered_opt {
                    std::future::poll_fn(|cx| {
                        use std::future::Future;
                        assert!(receive.as_mut().poll(cx).is_pending());
                        std::task::Poll::Ready(())
                    })
                    .await;
                    entered
                        .send(())
                        .map_err(|_| SessionError::TcpRuntimeStopped)?;
                }
                let deadline = async {
                    match timeout {
                        Some(timeout) => tokio::time::sleep(timeout).await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    biased;
                    _ = cancelled.changed() => Err(SessionError::NotReady),
                    result = &mut receive => result,
                    _ = deadline => Err(SessionError::Io(io::Error::from(io::ErrorKind::TimedOut))),
                }
            })
            .map(|count| (datagram, count))
    }
}

#[derive(Default)]
struct EventSlots {
    clipboard: Option<String>,
    ping: bool,
    /// Stream-configuration responses/rejects/errors, kept in arrival order so
    /// a caller of `request_stream_config` can correlate them by request id.
    stream_config: VecDeque<ControlMessage>,
    error: Option<SessionError>,
    closed: bool,
}

/// Retained stream-configuration results, oldest dropped past this bound.
const STREAM_CONFIG_SLOTS: usize = 16;

#[derive(Clone, Default)]
pub struct RuntimeEvents {
    shared: Arc<(Mutex<EventSlots>, Condvar)>,
}

impl RuntimeEvents {
    fn close(&self) {
        let (lock, ready) = &*self.shared;
        let mut slots = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slots.closed = true;
        ready.notify_all();
    }
    fn push(&self, event: Result<SessionEvent, SessionError>) {
        let (lock, ready) = &*self.shared;
        let mut slots = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match event {
            Ok(SessionEvent::Clipboard(text)) => slots.clipboard = Some(text),
            Ok(SessionEvent::Ping) => slots.ping = true,
            Ok(SessionEvent::StreamConfig(message)) => {
                if slots.stream_config.len() >= STREAM_CONFIG_SLOTS {
                    slots.stream_config.pop_front();
                }
                slots.stream_config.push_back(message);
            }
            Err(error) => slots.error = Some(error),
            Ok(
                SessionEvent::Ignored
                | SessionEvent::Frame(_)
                | SessionEvent::Audio(_)
                | SessionEvent::Cursor(_)
                | SessionEvent::InputAck { .. },
            ) => {}
        }
        ready.notify_one();
    }

    fn take(slots: &mut EventSlots) -> Option<Result<SessionEvent, SessionError>> {
        if let Some(text) = slots.clipboard.take() {
            return Some(Ok(SessionEvent::Clipboard(text)));
        }
        if std::mem::take(&mut slots.ping) {
            return Some(Ok(SessionEvent::Ping));
        }
        if let Some(message) = slots.stream_config.pop_front() {
            return Some(Ok(SessionEvent::StreamConfig(message)));
        }
        slots.error.take().map(Err)
    }

    fn pending(slots: &EventSlots) -> bool {
        slots.clipboard.is_some()
            || slots.ping
            || !slots.stream_config.is_empty()
            || slots.error.is_some()
    }

    pub fn try_recv(&self) -> Result<Result<SessionEvent, SessionError>, mpsc::TryRecvError> {
        let mut slots = self
            .shared
            .0
            .lock()
            .map_err(|_| mpsc::TryRecvError::Disconnected)?;
        Self::take(&mut slots).ok_or(if slots.closed {
            mpsc::TryRecvError::Disconnected
        } else {
            mpsc::TryRecvError::Empty
        })
    }

    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Result<SessionEvent, SessionError>, mpsc::RecvTimeoutError> {
        let (lock, ready) = &*self.shared;
        let slots = lock
            .lock()
            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)?;
        let (mut slots, _) = ready
            .wait_timeout_while(slots, timeout, |slots| {
                !Self::pending(slots) && !slots.closed
            })
            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)?;
        Self::take(&mut slots).ok_or(if slots.closed {
            mpsc::RecvTimeoutError::Disconnected
        } else {
            mpsc::RecvTimeoutError::Timeout
        })
    }

    pub fn recv(&self) -> Result<Result<SessionEvent, SessionError>, mpsc::RecvError> {
        let (lock, ready) = &*self.shared;
        let slots = lock.lock().map_err(|_| mpsc::RecvError)?;
        let mut slots = ready
            .wait_while(slots, |slots| !Self::pending(slots) && !slots.closed)
            .map_err(|_| mpsc::RecvError)?;
        Self::take(&mut slots).ok_or(mpsc::RecvError)
    }
}

pub struct SessionRuntime {
    events: RuntimeEvents,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl SessionRuntime {
    pub fn events(&self) -> &RuntimeEvents {
        &self.events
    }

    #[cfg(test)]
    fn from_worker_for_test(
        worker: thread::JoinHandle<()>,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Self {
        Self {
            events: RuntimeEvents::default(),
            stop,
            worker: Some(worker),
        }
    }

    pub fn stop(&mut self) -> Result<(), SessionError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            if let Err(payload) = worker.join() {
                self.events.close();
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| {
                        payload
                            .downcast_ref::<&str>()
                            .map(|message| (*message).to_owned())
                    })
                    .unwrap_or_else(|| "non-string panic payload".to_owned());
                return Err(SessionError::TcpRuntimePanicked(message));
            }
        }
        Ok(())
    }
}

impl Drop for SessionRuntime {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn resolve_one(address: impl ToSocketAddrs) -> Result<SocketAddr, SessionError> {
    address
        .to_socket_addrs()?
        .next()
        .ok_or(SessionError::NoAddress)
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl From<crate::MediaAssemblyError> for SessionError {
    fn from(error: crate::MediaAssemblyError) -> Self {
        SessionError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            error.to_string(),
        ))
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;

    use maho_net::TlsPskServer;
    fn packet(packet_type: PacketType, payload: &[u8]) -> Vec<u8> {
        let mut packet = PacketHeader::new(packet_type, 0, 0, 0).encode().unwrap();
        packet.extend_from_slice(payload);
        packet
    }
    fn split_packet(packet: &[u8]) -> (PacketHeader, &[u8]) {
        (
            PacketHeader::decode(&packet[..PacketHeader::SIZE]).unwrap(),
            &packet[PacketHeader::SIZE..],
        )
    }
    #[test]
    fn session_runtime_stop_propagates_worker_panic() {
        let (started_tx, started_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            started_tx.send(()).unwrap();
            panic!("simulated worker panic in session runtime");
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut runtime = crate::SessionRuntime::from_worker_for_test(worker, None);
        let error = runtime.stop().expect_err("worker panic must be returned");
        assert!(matches!(error, SessionError::TcpRuntimePanicked(message)
        if message == "simulated worker panic in session runtime"));
        runtime.stop().unwrap();
        assert!(matches!(
            runtime.events().try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn runtime_events_retain_stream_configuration_results() {
        let events = RuntimeEvents::default();
        let response =
            ControlMessage::StreamConfigResponse(maho_proto::StreamConfigurationResponse {
                request_id: 7,
                active: maho_proto::StreamConfiguration {
                    width: 1920,
                    height: 1080,
                    bitrate: 8_000_000,
                    frames_per_second: 60,
                },
            });
        let reject = ControlMessage::StreamConfigReject(maho_proto::StreamConfigurationReject {
            request_id: 8,
            reason: maho_proto::StreamConfigurationErrorCode::UnsupportedDimensions,
            message: "nope".to_owned(),
        });
        events.push(Ok(SessionEvent::StreamConfig(response.clone())));
        events.push(Ok(SessionEvent::StreamConfig(reject.clone())));

        assert_eq!(
            events
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap(),
            SessionEvent::StreamConfig(response)
        );
        assert_eq!(
            events.try_recv().unwrap().unwrap(),
            SessionEvent::StreamConfig(reject)
        );
        assert!(matches!(events.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }

    #[test]
    fn client_session_udp_receive_reuses_buffer_across_cancellations() {
        let pairing_id = "udp-reuse-pairing-id";
        let key = [0x55; 32];
        let psk = PskIdentity::pairing(pairing_id, &key).unwrap();
        let listener = TlsPskServer::new([psk])
            .unwrap()
            .bind("127.0.0.1:0")
            .unwrap();
        let tcp_address = listener.local_addr().unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp_port = udp.local_addr().unwrap().port();

        let server = thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            let handshake_packet = stream.read_frame().unwrap();
            let (header, payload) = split_packet(&handshake_packet);
            assert_eq!(header.packet_type, PacketType::Handshake);
            let handshake = Handshake::decode(payload).unwrap();
            let salt = handshake.session_salt;

            let acknowledgement = Handshake {
                name: "mock-host".to_owned(),
                width: 1920,
                height: 1080,
                scale: 1.0,
                version: PROTOCOL_VERSION,
                capabilities: Capabilities::AUTHENTICATED_UDP_REGISTRATION,
                pairing_id: String::new(),
                session_salt: [0; 16],
            };
            stream
                .write_frame(&packet(
                    PacketType::HandshakeAck,
                    &acknowledgement.encode().unwrap(),
                ))
                .unwrap();

            let mut ping = [0_u8; 1024];
            let (reg_len, client_udp_addr) = udp.recv_from(&mut ping).unwrap();
            let mut c2h = DatagramCipher::derive(&key, &salt, Direction::ClientToHost).unwrap();
            let (reg_hdr, _) = c2h.open_datagram(&ping[..reg_len]).unwrap();
            assert_eq!(reg_hdr.packet_type, PacketType::Ping);

            (udp, client_udp_addr, key, salt, stream)
        });

        let config = SessionConfig {
            host: "127.0.0.1".to_owned(),
            tcp_port: tcp_address.port(),
            udp_port,
            client_name: "rust-client".to_owned(),
            capabilities: Capabilities::AUTHENTICATED_UDP_REGISTRATION,
            pairing_store_path: None,
            connect_timeout: Duration::from_secs(2),
            handshake_ack_timeout: Duration::from_secs(2),
        };
        let session = ClientSession::new(config).unwrap();
        let record = crate::PairingRecord {
            id: pairing_id.to_string(),
            name: "mock-host".to_string(),
            key: key.to_vec(),
            added_at_unix_ms: 0,
            last_endpoint: None,
            endpoint_aliases: Vec::new(),
            relay_url: None,
            relay_host_id: None,
        };
        let ready = session.connect_with_pairing(record).unwrap();
        assert_eq!(ready.server.name, "mock-host");

        // Attempt 1: Cancelled receive
        let (entered_tx1, entered_rx1) = mpsc::channel();
        session.register_udp_entered_hook_for_test(entered_tx1);
        let s1 = session.clone();
        let (res_tx1, res_rx1) = mpsc::channel();
        let t1 = thread::spawn(move || {
            let res = s1.receive_udp_event();
            res_tx1.send(res).unwrap();
        });
        entered_rx1.recv_timeout(Duration::from_secs(5)).unwrap();
        let ptr1 = session
            .udp_recv_buffer_ptr()
            .expect("buffer pointer must exist");
        session.cancel_udp_receive_for_test();
        let res1 = res_rx1.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(res1, Err(SessionError::NotReady)));
        t1.join().unwrap();
        session.reset_udp_cancel_for_test();

        // Attempt 2: Cancelled receive
        let (entered_tx2, entered_rx2) = mpsc::channel();
        session.register_udp_entered_hook_for_test(entered_tx2);
        let s2 = session.clone();
        let (res_tx2, res_rx2) = mpsc::channel();
        let t2 = thread::spawn(move || {
            let res = s2.receive_udp_event();
            res_tx2.send(res).unwrap();
        });
        entered_rx2.recv_timeout(Duration::from_secs(5)).unwrap();
        let ptr2 = session
            .udp_recv_buffer_ptr()
            .expect("buffer pointer must exist");
        session.cancel_udp_receive_for_test();
        let res2 = res_rx2.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(res2, Err(SessionError::NotReady)));
        t2.join().unwrap();
        session.reset_udp_cancel_for_test();

        // Attempt 3: Successful datagram receive
        let (server_udp, client_udp_addr, server_key, session_salt, _tcp_stream) =
            server.join().unwrap();
        let mut server_cipher = maho_net::DatagramCipher::derive(
            &server_key,
            &session_salt,
            maho_net::Direction::HostToClient,
        )
        .unwrap();
        let ping_header = PacketHeader::new(PacketType::Ping, 1, 0, 0);
        let ping_datagram = server_cipher.seal_datagram(&ping_header, &[]).unwrap();

        let (entered_tx3, entered_rx3) = mpsc::channel();
        session.register_udp_entered_hook_for_test(entered_tx3);
        let s3 = session.clone();
        let (res_tx3, res_rx3) = mpsc::channel();
        let t3 = thread::spawn(move || {
            let res = s3.receive_udp_event();
            res_tx3.send(res).unwrap();
        });
        entered_rx3.recv_timeout(Duration::from_secs(5)).unwrap();
        let ptr3 = session
            .udp_recv_buffer_ptr()
            .expect("buffer pointer must exist");
        server_udp.send_to(&ping_datagram, client_udp_addr).unwrap();
        let res3 = res_rx3.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(res3.unwrap(), crate::SessionEvent::Ping);
        t3.join().unwrap();

        // A longer packet after a short packet must still have the full receive capacity.
        let header = PacketHeader::new(PacketType::Ping, 2, 0, 0);
        let datagram = server_cipher.seal_datagram(&header, &[7; 4096]).unwrap();
        server_udp.send_to(&datagram, client_udp_addr).unwrap();
        assert_eq!(
            session.receive_udp_event().unwrap(),
            crate::SessionEvent::Ping
        );

        // Deterministic assertions:
        let alloc_count = session.udp_recv_alloc_count().unwrap();
        assert_eq!(
            alloc_count, 1,
            "expected single 65536-byte receive allocation across attempts, but got {}",
            alloc_count
        );
        assert_eq!(
            ptr1, ptr2,
            "buffer pointer must be identical across cancellation attempts"
        );
        assert_eq!(
            ptr2, ptr3,
            "buffer pointer must be identical between cancellation and success"
        );

        session.disconnect().unwrap();
    }

    #[test]
    fn udp_cancel_wakes_registered_receive() {
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(peer.local_addr().unwrap()).unwrap();
        let transport = Arc::new(UdpTransport::new(socket).unwrap());
        let temporary = tempfile::tempdir().unwrap();
        let mut config = SessionConfig::direct("127.0.0.1", "udp");
        config.pairing_store_path = Some(temporary.path().join("pairings.json"));
        let session = ClientSession::new(config).unwrap();
        *session.udp.lock().unwrap() = Some(transport.clone());
        session.state.lock().unwrap().state = SessionState::Ready;
        let receiver = session.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        *transport.entered.lock().unwrap() = Some(entered_tx);
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = receiver.receive_udp_event();
            done_tx
                .send(matches!(result, Err(SessionError::NotReady)))
                .unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        session.disconnect().unwrap();
        assert!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn tls_read_step_returns_with_partial_frame() {
        use std::io::Write;
        let psk = PskIdentity::pairing("step", &[9; 32]).unwrap();
        let listener = maho_net::TlsPskServer::new([psk.clone()])
            .unwrap()
            .bind("127.0.0.1:0")
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (partial_tx, partial_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let host = thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            stream.ssl_stream_mut().write_all(&[20, 0]).unwrap();
            partial_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        let mut stream = TlsPskClient::new(psk).unwrap().connect(address).unwrap();
        partial_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        stream
            .ssl_stream()
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let result = stream.read_frame_step();
        release_tx.send(()).unwrap();
        host.join().unwrap();
        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn runtime_input_and_stop_progress_with_partial_frame() {
        use std::io::Write;
        let psk = PskIdentity::pairing("runtime", &[9; 32]).unwrap();
        let listener = maho_net::TlsPskServer::new([psk.clone()])
            .unwrap()
            .bind("127.0.0.1:0")
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (partial_tx, partial_rx) = mpsc::channel();
        let (input_tx, input_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let host = thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            stream
                .ssl_stream()
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream.ssl_stream_mut().write_all(&[20, 0]).unwrap();
            partial_tx.send(()).unwrap();
            let input = stream.read_frame().unwrap();
            assert_eq!(
                PacketHeader::decode(&input[..PacketHeader::SIZE])
                    .unwrap()
                    .packet_type,
                PacketType::InputEvent
            );
            input_tx
                .send(InputEvent::decode(&input[PacketHeader::SIZE..]).unwrap())
                .unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        let stream = TlsPskClient::new(psk).unwrap().connect(address).unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let mut config = SessionConfig::direct("127.0.0.1", "runtime");
        config.pairing_store_path = Some(temporary.path().join("pairings.json"));
        let session = ClientSession::new(config).unwrap();
        *session.tcp.lock().unwrap() = Some(stream);
        session.state.lock().unwrap().state = SessionState::Ready;
        let (wait_tx, wait_rx) = mpsc::channel();
        *session.tcp_wait.lock().unwrap() = Some(wait_tx);
        partial_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut runtime = session.spawn_tcp_runtime().unwrap();
        wait_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let input = InputEvent {
            event_type: maho_proto::InputEventType::MouseMove,
            x: 0.25,
            y: 0.75,
            key_code: 0,
            modifiers: maho_proto::Modifiers::empty(),
            scroll_dx: 0.0,
            scroll_dy: 0.0,
        };
        session.send_input(input).unwrap();
        assert_eq!(
            input_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            input
        );
        let (stop_tx, stop_rx) = mpsc::channel();
        let stop = thread::spawn(move || {
            runtime.stop().unwrap();
            stop_tx.send(()).unwrap();
        });
        stop_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        release_tx.send(()).unwrap();
        stop.join().unwrap();
        host.join().unwrap();
        session.disconnect().unwrap();
    }

    #[test]
    fn client_session_tracks_input_ack_events() {
        let config = SessionConfig::direct("127.0.0.1", "test-client");
        let session = ClientSession::new(config).unwrap();
        assert!(session.last_input_ack().is_none());

        // Simulate recording an InputAck via internal lock
        if let Ok(mut lock) = session.last_input_ack.lock() {
            *lock = Some((101, true, 0));
        }

        assert_eq!(session.last_input_ack(), Some((101, true, 0)));
    }

    #[test]
    fn worker_loss_is_retryable_but_real_failures_are_not() {
        // Given: the host service replaced its session worker, tearing down the
        // TCP control channel and the UDP media session under a connected
        // client. That is the Windows secure-desktop switch, and re-handshaking
        // with the stored pairing recovers it.
        for error in [
            SessionError::NotReady,
            SessionError::TcpRuntimeStopped,
            SessionError::TcpRuntimePanicked("worker exited".into()),
            SessionError::HandshakeAckTimeout,
        ] {
            assert!(
                should_reconnect(&error),
                "expected a reconnect for {error:?}"
            );
        }

        // Given: failures reconnecting cannot fix.
        for error in [
            SessionError::PairingNotFound("missing".into()),
            SessionError::NoAddress,
        ] {
            assert!(
                !should_reconnect(&error),
                "expected no reconnect for {error:?}"
            );
        }
    }
}

#[cfg(test)]
mod receiver_clock_tests {
    use super::*;
    use maho_net::TlsPskServer;
    use std::time::Instant;

    #[test]
    fn explicit_clock_ingress_records_assembly_interval_on_that_clock() {
        let key = [0x39; 32];
        let listener = TlsPskServer::new([PskIdentity::pairing("clock", &key).unwrap()])
            .unwrap()
            .bind("127.0.0.1:0")
            .unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let udp_peer = udp.try_clone().unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let mut config = SessionConfig::direct("127.0.0.1", "clock");
        config.tcp_port = listener.local_addr().unwrap().port();
        config.udp_port = udp.local_addr().unwrap().port();
        config.connect_timeout = Duration::from_secs(5);
        config.handshake_ack_timeout = Duration::from_secs(5);
        config.pairing_store_path = Some(temporary.path().join("pairings.json"));
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            stream
                .ssl_stream()
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let frame = stream.read_frame().unwrap();
            let handshake = Handshake::decode(&frame[PacketHeader::SIZE..]).unwrap();
            let mut ack = PacketHeader::new(PacketType::HandshakeAck, 0, 0, 0)
                .encode()
                .unwrap();
            ack.extend_from_slice(&handshake.encode().unwrap());
            stream.write_frame(&ack).unwrap();
            let mut probe = [0; 1024];
            let (probe_len, peer) = udp.recv_from(&mut probe).unwrap();
            let c2h =
                DatagramCipher::derive(&key, &handshake.session_salt, Direction::ClientToHost)
                    .unwrap();
            let mut c2h = c2h;
            let (probe_hdr, _) = c2h.open_datagram(&probe[..probe_len]).unwrap();
            assert_eq!(probe_hdr.packet_type, PacketType::Ping);
            ready_tx.send((peer, handshake.session_salt)).unwrap();
            // Stay readable until the client disconnects at teardown.
            let _ = stream.read_frame();
        });
        let session = ClientSession::new(config).unwrap();
        session
            .connect_with_pairing(PairingRecord {
                id: "clock".into(),
                name: "fixture".into(),
                key: key.to_vec(),
                added_at_unix_ms: 0,
                last_endpoint: None,
                endpoint_aliases: Vec::new(),
                relay_url: None,
                relay_host_id: None,
            })
            .unwrap();
        session
            .set_udp_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (peer, salt) = ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut cipher = DatagramCipher::derive(&key, &salt, Direction::HostToClient).unwrap();
        let header_datagram = cipher
            .seal_datagram(
                &PacketHeader::new(PacketType::FrameHeader, 1, 0, 0),
                &FrameHeader {
                    frame_id: 7,
                    width: 1,
                    height: 1,
                    is_key_frame: true,
                    total_chunks: 1,
                    total_size: 1,
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
        let chunk_datagram = cipher
            .seal_datagram(
                &PacketHeader::new(PacketType::FrameChunk, 2, 0, 0),
                &FrameChunk {
                    frame_id: 7,
                    chunk_index: 0,
                    data: vec![1],
                }
                .encode()
                .unwrap(),
            )
            .unwrap();

        // Future logical instants: wall-clock completion sampling would produce
        // a reversed interval and silently drop the assembly sample.
        let start = Instant::now() + Duration::from_secs(3600);
        udp_peer.send_to(&header_datagram, peer).unwrap();
        assert_eq!(
            session.receive_udp_event_at(start).unwrap(),
            SessionEvent::Ignored
        );
        udp_peer.send_to(&chunk_datagram, peer).unwrap();
        assert!(matches!(
            session
                .receive_udp_event_at(start + Duration::from_millis(5))
                .unwrap(),
            SessionEvent::Frame(_)
        ));
        let snapshot = session
            .receiver_snapshot_at(start + Duration::from_millis(6))
            .unwrap();
        let assembly = snapshot
            .receive_assembly_us
            .expect("explicit-clock assembly interval must be recorded");
        assert!(
            assembly.max_us <= 5_000,
            "assembly interval must come from the supplied clock, got {assembly:?}"
        );
        session.disconnect().unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn send_udp_registration_emits_authenticated_ping_and_retry() {
        let key = [0x5a; 32];
        let listener = TlsPskServer::new([PskIdentity::pairing("burst", &key).unwrap()])
            .unwrap()
            .bind("127.0.0.1:0")
            .unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let mut config = SessionConfig::direct("127.0.0.1", "burst");
        config.tcp_port = listener.local_addr().unwrap().port();
        config.udp_port = udp.local_addr().unwrap().port();
        config.connect_timeout = Duration::from_secs(5);
        config.handshake_ack_timeout = Duration::from_secs(5);
        config.pairing_store_path = Some(temporary.path().join("pairings.json"));

        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            stream
                .ssl_stream()
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let frame = stream.read_frame().unwrap();
            let handshake = Handshake::decode(&frame[PacketHeader::SIZE..]).unwrap();
            let mut ack = PacketHeader::new(PacketType::HandshakeAck, 0, 0, 0)
                .encode()
                .unwrap();
            ack.extend_from_slice(&handshake.encode().unwrap());
            stream.write_frame(&ack).unwrap();
            ready_tx.send(handshake.session_salt).unwrap();
            let _ = stream.read_frame();
        });

        let session = ClientSession::new(config).unwrap();
        session
            .connect_with_pairing(PairingRecord {
                id: "burst".into(),
                name: "burst_fixture".into(),
                key: key.to_vec(),
                added_at_unix_ms: 0,
                last_endpoint: None,
                endpoint_aliases: Vec::new(),
                relay_url: None,
                relay_host_id: None,
            })
            .unwrap();
        let salt = ready_rx.recv().unwrap();
        let mut c2h = DatagramCipher::derive(&key, &salt, Direction::ClientToHost).unwrap();

        // Initial registration packet was sent during connect
        let mut probe = [0u8; 1024];
        let (len, _) = udp.recv_from(&mut probe).unwrap();
        let (hdr, payload) = c2h.open_datagram(&probe[..len]).unwrap();
        assert_eq!(hdr.packet_type, PacketType::Ping);
        assert!(payload.is_empty());

        // Resending sends an additional valid registration ping with fresh nonce
        session.send_udp_registration().unwrap();
        let (len, _) = udp.recv_from(&mut probe).unwrap();
        let (hdr, payload) = c2h.open_datagram(&probe[..len]).unwrap();
        assert_eq!(hdr.packet_type, PacketType::Ping);
        assert!(payload.is_empty());

        session.disconnect().unwrap();
        worker.join().unwrap();
    }
}
