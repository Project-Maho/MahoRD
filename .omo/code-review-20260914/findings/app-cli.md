# Lane: app-cli
## Scope reviewed
- `clients/rust/maho-app/src/pairing.rs` (1227 lines)
- `clients/rust/maho-app/src/bin/maho_client.rs` (1345 lines)
- `clients/rust/maho-app/src/bin/maho_client/continuity_codec_tests.rs` (155 lines)
- `clients/rust/maho-app/src/bin/maho_client/receiver_telemetry_tests.rs` (33 lines)
- `clients/rust/maho-app/src/bin/maho_client/recovery_characterization.rs` (45 lines)
- `clients/rust/maho-app/src/tcp_write_tests.rs` (293 lines)
- (Transitive dependencies inspected for contract verification: `clients/rust/maho-net/src/tls_psk.rs`, `clients/rust/maho-proto/src/pairing.rs`, `clients/rust/maho-app/src/session.rs`, `clients/rust/maho-app/src/frame_queue.rs`)

## Findings

### [P1] Credential-store migration path omits legacy `EclipticRD` directory and Keychain service
- **Location**: `clients/rust/maho-app/src/pairing.rs:360-376` (secondary: `clients/rust/maho-app/src/pairing.rs:307-333`, `clients/rust/maho-app/src/pairing.rs:342-353`)
- **Evidence**:
```rust
    pub fn load_all(&self) -> Result<Vec<PairingRecord>, PairingStoreError> {
        match &self.backend {
            StoreBackend::File(path) => {
                if !path.exists() {
                    // Safe legacy client migration: if client-pairings.json does not exist
                    // and pairing-keys.json is present, migrate valid entries into client-pairings.json.
                    // The legacy file is preserved intact and never overwritten or deleted.
                    let legacy_path = path.with_file_name("pairing-keys.json");
                    if legacy_path.exists() && legacy_path != *path {
                        let legacy_records = Self::read_file_records(&legacy_path)?;
                        if !legacy_records.is_empty() {
                            self.write_records(path, &legacy_records)?;
                            return Ok(legacy_records);
                        }
                    }
                }
                Self::read_file_records(path)
            }
```
- **Impact**: In commit `60b02ca`, the rebrand mechanically renamed `EclipticRD` application data directories to `MahoRD` and Keychain service `com.eclipticrd.ios.pairing` to `com.projectmaho.mahord.pairing`. However, the migration logic in `load_all` only checks `path.with_file_name("pairing-keys.json")` inside the *new* `MahoRD` directory (`path.parent()`). It never inspects the previous `EclipticRD` directory (`~/Library/Application Support/EclipticRD`, `%APPDATA%\EclipticRD`, `~/.local/share/EclipticRD`) nor the previous Keychain service. Upgrading users lose access to all previously paired host credentials and are forced into an uncoordinated 1x PIN re-pairing.
- **Fix**: Extend the migration check: if neither `path` (`MahoRD/client-pairings.json`) nor `MahoRD/pairing-keys.json` exists, check for `EclipticRD/client-pairings.json` and `EclipticRD/pairing-keys.json` under the platform application data parent directory. For Keychain on iOS/macOS, if no entries match `com.projectmaho.mahord.pairing`, query `com.eclipticrd.ios.pairing` (and accounts with prefix `erd_pairing_`) and import valid records into the new store.
- **Confidence**: high

### [P1] Fatal TCP signaling disconnection is ignored by CLI decode loop, causing infinite hang
- **Location**: `clients/rust/maho-app/src/bin/maho_client.rs:731-746` (secondary: `clients/rust/maho-app/src/bin/maho_client.rs:120-128`)
- **Evidence**:
```rust
        // Check if TCP runtime emitted an error
        while let Ok(event_res) = tcp_runtime.events().try_recv() {
            match event_res {
                Ok(SessionEvent::Ping) => {
                    debug!("TCP heartbeat ping acknowledged");
                }
                Ok(SessionEvent::Clipboard(_text)) => {
                    debug!("Received clipboard update");
                }
                Ok(_) => {}
                Err(err) => {
                    // Non-fatal or timeout errors in polling shouldn't abort immediately unless fatal
                    warn!(%err, "TCP runtime poll error");
                }
            }
        }
```
- **Impact**: When the TCP signaling channel encounters a connection reset, broken pipe, or fatal error, the TCP worker thread pushes `Err(error)` into its event channel and closes the channel (`event_tx.close()`). In `maho_client.rs`, the polling loop drains `try_recv()`, logs `warn!(%err, "TCP runtime poll error")`, and does NOT set `running.store(false)` or break out of the main loop. On subsequent iterations, `try_recv()` returns `Err(TryRecvError::Disconnected)`, which `while let Ok` silently skips. When running in `--mcp` or `--agent-server` mode (where `timeout_secs` defaults to `None`), `cli.deadline_expired` returns `false` forever. The client enters an unbounded hang in an infinite loop while the signaling transport is dead and incapable of sending keyframe requests, heartbeat responses, or inputs.
- **Fix**: When `event_res` is `Err(err)` or when `tcp_runtime.events().try_recv()` returns `Err(mpsc::TryRecvError::Disconnected)` after the session was established, flag the signaling transport as terminated, trigger session teardown by setting `running.store(false, Ordering::SeqCst)`, and break the decode loop.
- **Confidence**: high

### [P1] Single invalid key length permanently bricks and wedges file-based pairing store
- **Location**: `clients/rust/maho-app/src/pairing.rs:384-398`
- **Evidence**:
```rust
    fn read_file_records(path: &Path) -> Result<Vec<PairingRecord>, PairingStoreError> {
        match fs::read(path) {
            Ok(bytes) => {
                let records: Vec<PairingRecord> = serde_json::from_slice(&bytes)?;
                for record in &records {
                    record.key_array()?;
                }
                Ok(records)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        }
    }
```
- **Impact**: If even a single pairing record in `client-pairings.json` has an invalid key length (e.g. truncated base64 string or manual edit), `record.key_array()?` returns `Err(PairingStoreError::InvalidKeyLength)`. Because `read_file_records` returns an error, `load_all()` fails. Crucially, `PairingStore::save`, `PairingStore::delete`, `PairingStore::load`, and `PairingStore::remember_endpoint` all begin by calling `self.load_all()?`. Consequently, the user cannot delete the bad record, cannot save a new record, cannot reconnect, and cannot use the store at all. The entire credential store is permanently wedged. (By comparison, `load_all_keychain` at line 822 correctly checks `if record.key_array().is_ok()` and ignores invalid entries).
- **Fix**: In `read_file_records`, warn on and filter out corrupt or invalid entries (or retain them in raw form so `delete(bad_id)` can prune them) instead of failing the entire read operation with `?`.
- **Confidence**: high

### [P1] Hardcoded developer IP/username and broken prefix matching in CLI host reconnect
- **Location**: `clients/rust/maho-app/src/bin/maho_client.rs:414-427`
- **Evidence**:
```rust
        let matched = match store.find_by_host(&cli.host) {
            Ok(Some(record)) => Some(record),
            _ => {
                if let Ok(records) = store.load_all() {
                    records.into_iter().rev().find(|r| {
                        cli.host.eq_ignore_ascii_case(&r.name)
                            || cli.host.starts_with(&r.name)
                            || (cli.host == "100.91.254.71" && r.name == "indo")
                    })
                } else {
                    None
                }
            }
        };
```
- **Impact**: When reconnecting with `maho-client --host <target>`, the CLI attempts to match `cli.host` against `r.name`. When a client pairs via `--pin`, `r.name` is populated from `grant.host_name` (the host's computer name, e.g. "MacBook-Pro"). If the user connects by IP address (e.g. `192.168.1.50`), `r.name` never matches `cli.host`. The fallback search contains a hardcoded developer backdoor/exception for IP `100.91.254.71` and name `indo`. For all other users, reconnecting by IP fails with `no stored pairing for host '<ip>'`. Furthermore, `cli.host.starts_with(&r.name)` causes false positive matches: if `r.name` is empty `""`, it matches the first record in the store; if `cli.host` is `192.168.1.100` and `r.name` is `192.168.1.1`, it matches and connects with the wrong host's PSK identity.
- **Fix**: Remove the hardcoded `100.91.254.71`/`indo` residue and replace `starts_with` with exact matching. Check `record.last_endpoint` host and `record.endpoint_aliases` when `cli.host` is an IP address or hostname.
- **Confidence**: high

### [P2] Secret pairing keys and tokens held in plain memory, exposed in `Debug`, and never zeroized
- **Location**: `clients/rust/maho-app/src/pairing.rs:32-38` (secondary: `clients/rust/maho-app/src/bin/maho_client.rs:32-60`)
- **Evidence**:
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingRecord {
    pub id: String,
    pub name: String,
    #[serde(deserialize_with = "deserialize_key", serialize_with = "serialize_key")]
    pub key: Vec<u8>,
```
and `clients/rust/maho-app/src/bin/maho_client.rs:32-52`:
```rust
#[derive(Debug, Parser)]
#[command(
    name = "maho-client",
    about = "MahoRD headless testing client for VM E2E driving",
    version
)]
struct Cli {
    /// Remote host address (IPv4, IPv6, or hostname).
    #[arg(long)]
    host: String,

    /// Remote host TCP TLS-PSK signaling port.
    #[arg(long, default_value_t = DEFAULT_TCP_PORT)]
    tcp_port: u16,

    /// Remote host UDP media port.
    #[arg(long)]
    udp_port: Option<u16>,

    /// 8-digit bootstrap PIN for pairing with the host.
    #[arg(long, conflicts_with = "psk_hex")]
    pin: Option<String>,

    /// 64-character hexadecimal pre-shared key (32 bytes) if pairing store is unavailable.
    #[arg(long, conflicts_with = "pin")]
    psk_hex: Option<String>,
```
- **Impact**: `PairingRecord` derives `Debug` and stores 32-byte TLS-PSK pre-shared keys in a standard heap `Vec<u8>`. `Cli` in `maho_client.rs` derives `Debug` and stores the 8-digit bootstrap PIN, `--psk-hex`, and `--agent-token` in plain `Option<String>`. No types implement `zeroize::Zeroize` or `ZeroizeOnDrop`. In the event of a crash, core dump, debug logging (`{:?}`), or heap inspection, raw cryptographic credentials and bearer tokens remain readable in memory.
- **Fix**: Implement custom `Debug` for `PairingRecord` and `Cli` that redacts secret fields (`[REDACTED]`). Use `zeroize::Zeroizing<Vec<u8>>` or `zeroize::ZeroizeOnDrop` for secret key material.
- **Confidence**: high

### [P2] Insecure static temporary file and missing Windows permissions in `write_records`
- **Location**: `clients/rust/maho-app/src/pairing.rs:520-525` (secondary: `clients/rust/maho-app/src/pairing.rs:858-887`)
- **Evidence**:
```rust
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(records)?;
        write_private(&temporary, &bytes)?;
        fs::rename(&temporary, path)?;
        set_private_permissions(path)?;
```
and `write_private` / `set_private_permissions` at lines 858-887:
```rust
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes)
    }
}

fn set_private_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}
```
- **Impact**: 
  1. `temporary` uses a fixed static path (`path.with_extension("json.tmp")`). Concurrent invocations (e.g. headless CLI client running alongside GUI client) race on the same temporary file, causing write truncation and atomic rename collisions.
  2. On Unix, `OpenOptions::mode(0o600)` only sets permissions if the file is created anew by `open(O_CREAT)`. If `client-pairings.json.tmp` already existed from a previous run or with default umask (`0o644`), `open` does not alter its permissions, writing secret pairing keys into a world-readable file before `fs::rename`.
  3. On Windows, `write_private` falls back to `fs::write` and `set_private_permissions` is a complete no-op (`let _ = path; Ok(())`), failing to set private ACLs on Windows multi-user systems.
- **Fix**: Use unique randomized temporary file names in the target directory (e.g. `tempfile::Builder::new().tempfile_in(parent)`). Explicitly call `set_permissions` immediately on the newly opened temporary file handle before writing. On Windows, apply an explicit DACL granting access solely to current user SID.
- **Confidence**: high

### [P2] Fake `ctrlc_handler` silently drops signal callback, preventing graceful teardown and stats generation
- **Location**: `clients/rust/maho-app/src/bin/maho_client.rs:983-999` (secondary: `clients/rust/maho-app/src/bin/maho_client.rs:506-511`)
- **Evidence**:
```rust
fn ctrlc_handler<F>(f: F) -> Result<()>
where
    F: Fn() + Send + Sync + 'static,
{
    // Best-effort ctrl-c handler without extra crate
    // On Unix, standard signal hooks can be set or ignored gracefully.
    #[cfg(unix)]
    {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            // Nothing required if signal handler isn't needed, standard SIGINT exits process
        });
    }
    let _ = f;
    Ok(())
}
```
and registration at lines 506-511:
```rust
    let running = Arc::new(AtomicBool::new(true));
    let r_ctrl = running.clone();
    let _ = ctrlc_handler(move || {
        r_ctrl.store(false, Ordering::SeqCst);
    });
```
- **Impact**: The caller registers a closure intended to toggle `running` to `false` on SIGINT, so the main loop can perform graceful teardown (`teardown(&session, ...)` sends `ControlMessage::Disconnect` to remote host) and output `--stats-json` via `write_stats_file`. But `ctrlc_handler` is a dummy stub that executes `let _ = f;` and registers nothing. Pressing Ctrl-C sends SIGINT, abruptly terminating the process. The server receives no disconnect signal and holds session state until ping timeout, while all captured decode latency statistics are discarded without being written to disk.
- **Fix**: Register an actual signal handler using `tokio::signal::ctrl_c()` or a standard POSIX sigaction hook via `rustix`, setting `running.store(false, Ordering::SeqCst)`.
- **Confidence**: high

### [P2] Missing validation for `--pin` produces misleading error message
- **Location**: `clients/rust/maho-app/src/bin/maho_client.rs:48-50` (secondary: `clients/rust/maho-net/src/tls_psk.rs:622-626`)
- **Evidence**:
```rust
    /// 8-digit bootstrap PIN for pairing with the host.
    #[arg(long, conflicts_with = "psk_hex")]
    pin: Option<String>,
```
and `clients/rust/maho-net/src/tls_psk.rs:622-626`:
```rust
pub fn bootstrap_psk(pin: &str) -> Result<[u8; 32], TlsPskError> {
    if pin.len() != 8 || !pin.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TlsPskError::InvalidIdentity);
    }
```
- **Impact**: If a user enters a PIN that is not exactly 8 ASCII digits (e.g. 4 or 6 digits, accidental whitespace, or typo), `bootstrap_psk` returns `TlsPskError::InvalidIdentity`. The error message presented to the user is `Error: bootstrap pairing failed: PSK identity must be non-empty UTF-8 without NUL bytes`, which is completely misleading and fails to inform the user that the PIN must be exactly 8 digits.
- **Fix**: Add a clap value parser or upfront validator on `--pin` in `maho_client.rs` that checks `pin.len() == 8 && pin.chars().all(|c| c.is_ascii_digit())`, returning `error: --pin must be exactly 8 digits`.
- **Confidence**: high

### [P2] Dead code and contract violation in `--nudge-ms` driver loop
- **Location**: `clients/rust/maho-app/src/bin/maho_client.rs:524-539`
- **Evidence**:
```rust
    if let Some(nudge_ms) = cli.nudge_ms.filter(|value| *value > 0) {
        let session_nudge = session.clone();
        let r_nudge = running.clone();
        let _first = Arc::clone(&first_frame_seen);
        std::thread::Builder::new()
            .name("maho-client-nudge".into())
            .spawn(move || {
                let mut flip = false;
                while r_nudge.load(Ordering::Relaxed) {
                    let x = if flip { 0.501_5 } else { 0.5 };
                    flip = !flip;
                    let event = InputEvent {
                        event_type: InputEventType::MouseMove,
                        x,
                        y: 0.5,
                        key_code: 0,
                        modifiers: Modifiers::empty(),
                        scroll_dx: 0.0,
                        scroll_dy: 0.0,
                    };
```
- **Impact**: The CLI documentation explicitly states: "Send a tiny alternating mouse-move every N ms once streaming starts." The code binds `let _first = Arc::clone(&first_frame_seen);` (where `first_frame_seen` is set to `true` on line 757 when the first video frame is decoded), but `_first` is never read in the thread loop. The thread begins injecting mouse-move input events immediately before any video frame has arrived, before decoding begins, and while the handshake/session is still initializing.
- **Fix**: In the nudge thread loop, wait until `_first.load(Ordering::Relaxed)` is true before sending input events.
- **Confidence**: high

### [P2] Unbounded memory and disk growth in `PairingRecord::endpoint_aliases`
- **Location**: `clients/rust/maho-app/src/pairing.rs:529-537`
- **Evidence**:
```rust
fn update_record_endpoint(record: &mut PairingRecord, endpoint: PairingEndpoint) {
    record.endpoint_aliases.retain(|a| a != &endpoint);
    if let Some(prev) = record.last_endpoint.take() {
        if prev != endpoint && !record.endpoint_aliases.contains(&prev) {
            record.endpoint_aliases.push(prev);
        }
    }
    record.last_endpoint = Some(endpoint);
}
```
- **Impact**: Every time `remember_endpoint` is invoked with a different endpoint (common on mobile or laptop clients roaming across Wi-Fi, cellular, VPN, and LAN subnets), the previous endpoint is pushed to `record.endpoint_aliases`. There is no maximum cap or LRU truncation. Over time, `endpoint_aliases` accumulates unbounded entries in the on-disk JSON file and in memory.
- **Fix**: Bound `endpoint_aliases` to a reasonable maximum (e.g. 8 or 16 most recent endpoints), truncating older entries when the cap is reached.
- **Confidence**: high

## Non-findings checked
- `pairing.rs:247`: `key_array` strictly validates `PAIRING_KEY_SIZE == 32`, rejecting malformed keys.
- `pairing.rs:67-90`: `PairingSummary` explicitly omits the secret `key` field, preventing credential leakage in IPC or web summaries.
- `pairing.rs:470-474`: `remember_endpoint` verifies `record.key == expected_key` before updating endpoint metadata, preventing cross-credential endpoint mutation.
- `pairing.rs:590-604`: CoreFoundation wrapper `CfWrapper` correctly pairs allocation with `CFRelease` in its `Drop` implementation.
- `pairing.rs:945-985`: `test_legacy_client_migration_preserves_legacy_and_writes_client_pairings` ensures the legacy `pairing-keys.json` file is preserved byte-for-byte during in-directory migration.
- `maho_client.rs:347-362`: `parse_hex_32` strictly validates that `--psk-hex` is exactly 64 hexadecimal characters (32 bytes).
- `maho_client.rs:325-345`: `FrameQueue` implements half-range serial ordering (`advance >= (1 << 31)`) to handle `u32` frame counter wrapping without spurious recovery requests.
- `maho_client.rs:185-215`: `store_latest_frame` synchronizes frame buffer writes with a mutex, ensuring agent screenshot endpoints do not read torn frame memory.
- `tcp_write_tests.rs:155-215`: Kernel TCP buffer saturation tests confirm backpressure (`WouldBlock`) is handled without message reordering or dropping control packets.
- `continuity_codec_tests.rs:95-155`: Characterization tests confirm the continuity queue suppresses dependent frames and prevents decoder corruption across missing chunks, buffer overflows, and inverted completions.
