# Implementation Contracts: Remote Connection and Pairing Improvement

## Lead review amendments (binding over examples below)

The Flash draft below supplies the plan. These corrections follow direct source review and take precedence over illustrative pseudocode:

1. Keep the existing separately namespaced `erd_app::PairingRecord` and `erd_host::PairingRecord` names and compatible serialized key/time fields. Do not rename every consumer to new record types. Separate default filenames and temporary files; add only the endpoint metadata and public DTO actually needed. Parent directories may be shared; inbound/outbound credential files may not.
2. Preserve the existing flat Tauri `connect(host, tcp_port, udp_port, pin, ...)` invoke shape; add optional `credential_id` rather than nesting all callers under a new `request` parameter. Normalize a verified endpoint as host PLUS TCP port, not `last_endpoint == request.host`. Match exact stored endpoints; preserve existing keys on migration but never auto-import ambiguous legacy records into the host.
3. Use the existing `random_pin()` implementation and rand version. Never assert that a random sample differs from `"12345678"`: that assertion is probabilistic. Inject the PIN generation seam to prove the default branch invokes random generation and explicit PIN bypasses it; validate format separately.
4. Paired TLS success is not application `Authenticated` yet. Keep application handshake validation before accepting input or starting media. Bind bootstrap consent to its connection-local granted ID and reject duplicate handshakes.
5. UDP endpoint is fixed after the first valid current-session registration. No new RwLock dependency, polling sender, or endpoint migration in this change. Reuse `PacketType::Ping`, empty payload, existing seal/open datagram APIs and nonce ownership. Introduce an authenticated-registration capability bit in existing handshake capabilities, advertise it on both ends and fail explicitly on missing support before Ready; no plaintext compatibility fallback. Validate actual header/payload after authentication. This preserves framing v3 but deliberately requires capability negotiation.
6. `SessionEvent::Disconnected` and `SessionRuntimeReceiver` in the draft are illustrative, not existing APIs. Read the actual `RuntimeEvents` and terminal-error/closed behavior. Native cleanup must not self-join or block while holding `inner`; only iOS needs the new terminal consumer unless a demonstrated desktop regression requires more.
7. Tests are behavioral: explicit credential selection must authenticate to a real loopback peer with that ID/key, not only assert a mocked load call. Modal tests assert visibility/action/focus, not a pinned DOM parent. Lifecycle uses events with a bounded timeout, not an arbitrary 100ms performance deadline. Test paths/names below are planned targets, not existing evidence.
8. Preserve existing timeout and lockout policy; do not invent fixed five-minute lockout or 30-second consent values from draft descriptions. Errors should classify actual typed sources; unknown TLS failures must not all be labeled revoked credentials.

- Document Version: 1.0.0
- Date: 2026-09-10
- Reference Plan: `docs/remote-connection-pairing-review-plan-20260910.md`
- Base Commit: `71f8b05f5a0e9d53ce0249beb46ca864b7a836f8`
- Deliverable: Decision-complete implementation specification and regression test matrix covering findings R1 through R11.
- Execution Policy: Disjoint file ownership per phase, zero unauthenticated fallback, typed error domain, deterministic tests.

---

## 1. Architectural Principles and Invariants

1. **Directional Separation of Trust (R1):** Host inbound authorization records and client outbound credentials must never share a storage file, directory path, or identity namespace. An outbound client credential must never grant inbound access to a local host.
2. **Cryptographic Binding of Bootstrap Consent (R2):** Bootstrap TLS (`erd-b1`) is restricted to the pairing handshake. It cannot transition to `Authenticated` without active host operator approval, and the subsequent application `Handshake` must be strictly bound to the freshly granted `pairing_id`.
3. **Authenticated Media Transport Endpoint (R3):** UDP reception endpoint on the host is determined exclusively by a datagram authenticated with the session's directional `ClientToHost` cipher. Plaintext probes (`0xff`) and unauthenticated datagrams are discarded without state modification.
4. **Zero Secrets in Public APIs (R4):** IPC commands, Tauri invoke handlers, and logging channels must never expose long-term 256-bit symmetric keys. UI consumes only public summary metadata (`PairingSummary`).
5. **Unified Session Lifecycle Ownership (R5, R6):** Native session owners supervise both TCP and UDP transports. TCP connection termination is an immediate terminal event that tears down workers and transitions state to disconnected/error. Cleanup failures preserve error state and prevent new connections until settled.
6. **Accessible Inflight Cancellation (R7):** Connecting modals are positioned at the root viewport layer, allowing users to abort in-flight connection attempts at any stage.
7. **Explicit Credential Selection (R8, R9):** Connections use explicit credential identifiers (`credential_id`) and verified network endpoints. Discovery names, mDNS prefixes, and unverified IPs are display-only hints and cannot bind credentials.
8. **Preservation of Scoped IPv6 Link-Local Endpoints (R10):** Link-local IPv6 addresses retain their interface scope IDs across parsing, filtering, and connection resolution.
9. **Secure Random Default Bootstrap PIN (R11):** Host defaults to a cryptographically secure, random 8-digit PIN. Automatic fallback to bootstrap pairing upon reconnection failure is prohibited.

---

## 2. Storage and Migration Contracts (R1, R8, R9)

### 2.1 Storage Paths and File Names

| Component | Target Platform | Storage Backend | File / Service Name | Path Resolution |
|---|---|---|---|---|
| **Host Authorization** | macOS | File (JSON) | `host-authorizations.json` | `~/Library/Application Support/EclipticRD/host-authorizations.json` |
| **Host Authorization** | Linux | File (JSON) | `host-authorizations.json` | `$XDG_DATA_HOME/EclipticRD/host-authorizations.json` (fallback: `~/.local/share/EclipticRD/host-authorizations.json`) |
| **Host Authorization** | Windows | File (JSON) | `host-authorizations.json` | `%APPDATA%\EclipticRD\host-authorizations.json` |
| **Client Credentials** | macOS | File (JSON) | `client-pairings.json` | `~/Library/Application Support/EclipticRD/client-pairings.json` |
| **Client Credentials** | Linux | File (JSON) | `client-pairings.json` | `$XDG_DATA_HOME/EclipticRD/client-pairings.json` (fallback: `~/.local/share/EclipticRD/client-pairings.json`) |
| **Client Credentials** | Windows | File (JSON) | `client-pairings.json` | `%APPDATA%\EclipticRD\client-pairings.json` |
| **Client Credentials** | iOS | Keychain | Service: `com.eclipticrd.ios.pairing` | iOS Secure Keychain |

### 2.2 Data Schemas

#### Host Authorization Store (`host-authorizations.json`)
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostAuthorizationRecord {
    /// Unique pairing ID generated by host (UUIDv4 uppercase string)
    pub id: String,
    /// Human-readable client name supplied in PairingRequest
    pub client_name: String,
    /// 32-byte shared symmetric key (Base64 encoded on disk)
    #[serde(deserialize_with = "deserialize_key", serialize_with = "serialize_key")]
    pub key: Vec<u8>,
    /// Timestamp when authorization was granted (Unix milliseconds)
    pub authorized_at_unix_ms: u64,
}
```

#### Client Pairing Store (`client-pairings.json`)
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientPairingRecord {
    /// Pairing ID issued by the remote host
    pub id: String,
    /// Host name reported by remote host in PairingGrant
    pub host_name: String,
    /// 32-byte shared symmetric key (Base64 encoded on disk)
    #[serde(deserialize_with = "deserialize_key", serialize_with = "serialize_key")]
    pub key: Vec<u8>,
    /// Timestamp when pairing occurred (Unix milliseconds)
    pub added_at_unix_ms: u64,
    /// Last verified endpoint (e.g. "192.168.1.100:19730" or "[fe80::1%en0]:19730")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_endpoint: Option<String>,
}
```

### 2.3 Migration Policy

1. **Legacy File:** `pairing-keys.json` in user application directories is treated as **legacy read-only**.
2. **Host Migration (Strict Isolation):**
   - The host authorization engine **must not** auto-import records from `pairing-keys.json`.
   - Rationale: Because legacy records mixed client credentials and host grants, auto-importing risks exposing the host to remote machines the user connected to as a client.
   - All inbound peers must obtain explicit operator authorization on first connection under the new system.
3. **Client Migration:**
   - If `client-pairings.json` does not exist and `pairing-keys.json` is present:
     - The client reads `pairing-keys.json`, converts valid entries into `ClientPairingRecord` (setting `last_endpoint = None`), and writes `client-pairings.json`.
     - The legacy `pairing-keys.json` file is preserved intact as backup; it is never overwritten or deleted by ERD.
4. **Keychain (iOS):**
   - Keys in iOS Keychain service `com.eclipticrd.ios.pairing` are strictly client outbound records. Schema updates maintain backwards compatibility with existing Keychain items.

---

## 3. Public IPC and DTO Contracts (R4, R8, R9)

### 3.1 Public Pairing Summary DTO
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingSummary {
    pub id: String,
    pub host_name: String,
    pub added_at_unix_ms: u64,
    pub last_endpoint: Option<String>,
}
```

### 3.2 Tauri & Native IPC Commands

```rust
// Lists stored client pairings WITHOUT exposing secret keys
#[tauri::command]
pub fn list_pairings() -> Result<Vec<PairingSummary>, IpcError>;

// Deletes a pairing from the client store
#[tauri::command]
pub fn forget_pairing(id: String) -> Result<(), IpcError>;

// Connect request structure
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRequest {
    pub host: String,
    pub tcp_port: Option<u16>,
    pub udp_port: Option<u16>,
    pub pin: Option<String>,
    pub credential_id: Option<String>,
}

#[tauri::command]
pub async fn connect(
    state: State<'_, AppState>,
    request: ConnectRequest,
) -> Result<ConnectResponse, IpcError>;
```

### 3.3 Credential Selection Logic (R8, R9)
- If `request.pin` is provided (8 digits): Initiates bootstrap TLS (`erd-b1`) pairing flow.
- If `request.pin` is absent and `request.credential_id` is provided: Looks up the record strictly by `id == credential_id`.
- If both `pin` and `credential_id` are absent:
  - Client attempts lookup in `ClientPairingStore` matching `last_endpoint == request.host` or verified alias.
  - If no unambiguous record is found, returns typed error `pairing-required`.
  - **Prohibited:** Fuzzy string matching, prefix comparisons (`host.starts_with(&r.name)`), case-insensitive name collisions, or hardcoded IP exceptions (`100.91.254.71`).

---

## 4. Bootstrap and TLS Authentication Contract (R2, R11)

### 4.1 Host PIN Configuration & Generation (R11)
- Default CLI invocation (`erd-host` without `--pin` or `--bootstrap-pin`): Generates a cryptographically secure random 8-digit numeric string via `rand::rng().random_range(10_000_000..=99_999_999)`.
- The PIN is logged to `stdout` with clear formatting.
- Explicit `--pin generate` continues to generate a random 8-digit PIN.
- `--bootstrap-pin <8-digits>` accepts explicit PIN for automated testing/headless environments.
- Hardcoded default `"12345678"` is deleted.

### 4.2 Bootstrap TLS Consent Binding State Machine (R2)

```text
[Incoming TCP Stream]
        │
        ▼
   [TLS Handshake]
        │
   Identity Negotiated:
   ├── "erd-p1.{id}" ───────────► [Verify id in HostAuthorizationStore]
   │                                     │
   │                               Valid │ Invalid
   │                                     ▼         ▼
   │                           [State: Authenticated] [Close Stream]
   │
   └── "erd-b1" (Bootstrap) ────► [State: BootstrapConnected]
                                         │
                                   Receive Packet:
                                   ├── HandshakeAck / Input / Control ──► [Reject: PreAuth / Close]
                                   ├── Handshake ───────────────────────► [Reject: ConsentRequired / Close]
                                   └── PairingRequest(name) ────────────► [Prompt Operator Consent]
                                                                                │
                                                                   Approved ────┴──── Denied / Timeout
                                                                      │                     │
                                                          [Generate PairingRecord]     [PairingReject]
                                                          [Send PairingGrant(id, key)]      │
                                                          [State: PairingGranted(id)]  [Close Stream]
                                                                      │
                                                          Receive Packet:
                                                          ├── Handshake(id2 != id) ──► [Reject: IdentityMismatch / Close]
                                                          └── Handshake(id) ─────────► [State: Authenticated(id)]
                                                                                             │
                                                                                    Subsequent Handshake:
                                                                                    └── [Reject: AlreadyAuthenticated / Close]
```

### 4.3 Host Session Implementation Rules
1. In `SessionState::BootstrapConnected`, `PacketType::Handshake` is **strictly disallowed**. Any handshake sent prior to operator consent triggers immediate connection termination.
2. When operator grants consent, host generates:
   `pairing_id = Uuid::new_v4().to_string().to_uppercase()`
   `key = 32 random bytes`
   Saves to `HostAuthorizationStore`, sends `PairingGrant`, and sets state to `SessionState::PairingGranted { granted_id: pairing_id }`.
3. In `SessionState::PairingGranted { granted_id }`, the incoming `Handshake` packet's `pairing_id` must match `granted_id` exactly. Mismatch terminates the connection.
4. Once in `SessionState::Authenticated`, subsequent `Handshake` packets are rejected with an error; capture pipelines and cipher contexts are never re-initialized on an existing connection.

---

## 5. Authenticated UDP Registration Protocol Contract (R3)

### 5.1 Registration Protocol Decision
- **Protocol Version:** ERD v3.
- **Wire Format:** Standard ERD encrypted datagram (`erd-net/src/udp_gcm.rs`).
- **Plaintext Probe Disablement:** The unauthenticated `0xff` probe is abolished. No unencrypted UDP packet is accepted by the host or sent by the client.
- **Registration Packet Shape:**
  - Client constructs a standard `PacketHeader`:
    - `packet_type`: `PacketType::Ping`
    - `sequence`: `0` (or `1`)
    - `timestamp_ms`: Current monotonic timestamp
    - `flags`: `0`
  - Encrypted with `DatagramCipher::derive(&key, &salt, Direction::ClientToHost)`.
  - Wire bytes: `[Header (12B)][Nonce (12B)][Encrypted Payload (0B)][GCM Tag (16B)]` = 40 bytes minimum.

### 5.2 Host Endpoint Registration Flow
```rust
fn discover_udp_peer(
    &self,
    tcp_peer: SocketAddr,
    udp_peer: &mut Option<SocketAddr>,
    receive_cipher: &mut DatagramCipher,
) -> Result<(), SessionError> {
    let mut buffer = [0_u8; 2048];
    loop {
        match self.udp_socket.recv_from(&mut buffer) {
            Ok((length, peer)) if peer.ip() == tcp_peer.ip() => {
                // Reject all plaintext packets (length < 40 or missing header)
                if length < 40 {
                    continue;
                }
                // Cryptographic verification: MUST open cleanly with session cipher
                match receive_cipher.open_datagram(&buffer[..length]) {
                    Ok((header, _payload)) => {
                        if header.packet_type == PacketType::Ping {
                            *udp_peer = Some(peer);
                            return Ok(());
                        }
                    }
                    Err(DatagramError::Authentication) | Err(_) => {
                        // Forged, tampered, or wrong-session packet: DISCARD without state change
                        continue;
                    }
                }
            }
            Ok(_) => continue, // Ignore packets from non-matching IP
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(SessionError::Io(e)),
        }
    }
}
```

### 5.3 Host Media Sender Thread Binding
- The host media sender thread must not capture a stale or uninitialized `udp_peer` address.
- `udp_peer` is stored in an `Arc<parking_lot::RwLock<Option<SocketAddr>>>` (or synchronized channel).
- Media sender thread polls or awaits the authenticated registration event before transmitting video/audio frames.
- Once registered, changes to `udp_peer` require a valid authenticated datagram from the new port.

---

## 6. Typed Error Domain Contract (Cross-Platform)

### 6.1 Standard Error Codes
All errors crossing the native-to-UI boundary must serialize as structured JSON containing a standardized machine-readable `code`:

| Code | Stage | Description | User / Client Action |
|---|---|---|---|
| `pairing-required` | `preauth` | No pairing PIN provided and no saved credential found | Prompt user for 8-digit PIN |
| `pairing-denied` | `preauth` | Host operator explicitly clicked "Deny" | Show "Connection rejected by host" |
| `pairing-locked-out` | `preauth` | Host locked out pairing due to excessive attempts | Wait lockout duration (5 minutes) |
| `pairing-disabled` | `preauth` | Host pairing window expired or pairing disabled | Prompt user to start pairing on host |
| `credential-rejected` | `tls-psk` | Host rejected saved pairing key (revoked or unknown ID) | **Do not auto-retry with PIN**. Mark key invalid, prompt for PIN |
| `invalid-pin` | `client` | PIN is not 8 numeric digits | Inline validation warning |
| `consent-timeout` | `preauth` | Operator did not respond within 30 seconds | Show timeout error, allow manual retry |
| `handshake-timeout` | `handshake` | Host did not send `HandshakeAck` within deadline | Network error retry |
| `remote-closed` | `runtime` | Host closed TCP connection cleanly or crashed | Session terminated, return to host list |
| `network-unreachable` | `connect` | TCP connect refused or host IP unreachable | Check IP/network connection |
| `cleanup-failed` | `cleanup` | Native session release failed | Disable connect until resolved |
| `cancelled` | `connect` | User clicked Cancel or connection superseded | Return to idle cleanly |

### 6.2 Error DTO Definition

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: String,
    pub message: String,
    pub stage: String,
    pub retryable: bool,
}
```

```javascript
// TypeScript / JavaScript shape
interface IpcError {
  code:
    | 'pairing-required'
    | 'pairing-denied'
    | 'pairing-locked-out'
    | 'pairing-disabled'
    | 'credential-rejected'
    | 'invalid-pin'
    | 'consent-timeout'
    | 'handshake-timeout'
    | 'remote-closed'
    | 'network-unreachable'
    | 'cleanup-failed'
    | 'cancelled';
  message: string;
  stage: 'client' | 'connect' | 'preauth' | 'tls-psk' | 'handshake' | 'runtime' | 'cleanup';
  retryable: boolean;
}
```

---

## 7. Lifecycle, Cancellation, and UI Contracts (R5, R6, R7)

### 7.1 Unified Session Supervisor (iOS & Desktop) (R5)
In `clients/rust/ios-shell/src/state.rs`:
1. `SessionRuntime` returned by `ClientSession::spawn_tcp_runtime()` produces `events: SessionRuntimeReceiver`.
2. A dedicated supervisor thread (`erd-ios-supervisor`) is spawned alongside worker threads:
   ```rust
   let supervisor = thread::Builder::new()
       .name("erd-ios-supervisor".into())
       .spawn(move || {
           while let Ok(event_result) = event_rx.recv() {
               match event_result {
                   Ok(SessionEvent::Ignored) => {}
                   Ok(SessionEvent::Disconnected) | Err(_) => {
                       // Host closed connection or fatal TCP error occurred
                       state.handle_remote_disconnect(current_generation, "remote-closed");
                       break;
                   }
                   _ => {}
               }
           }
           // Channel closed indicates runtime cleanup finished
           state.handle_remote_disconnect(current_generation, "remote-closed");
       });
   ```
3. `handle_remote_disconnect` sets `inner.state = ConnectionState::Disconnected`, records `last_error = Some("remote-closed")`, signals `stop_flag.store(true)`, and shuts down audio/media workers immediately.
4. `stats()` inspects the active `ClientSession` state; if the session is `Disconnected`, `stats().state` reports `"disconnected"` or `"error"`, never `"ready"`.

### 7.2 Mobile Connection State Manager Cleanup Guarantees (R6)
In `clients/rust/ios-shell/ui/connection-state.js`:
1. **Single Pending Cleanup Promise:** All calls to `disconnect()` share a single in-flight `pendingCleanup` promise. Concurrent disconnect invocations return this same promise.
2. **Cleanup on Error:** `disconnect()` executes `invoke('disconnect')` whenever native resources are held, including when `state === 'error'`.
3. **No False Idle on Failure:** If `invoke('disconnect')` throws, state transitions to `cleanup-failed`, preserving `lastError`. State **never** transitions to `idle` when cleanup fails.
4. **Guarded Dismiss:** `dismissError()` only transitions to `idle` if `state === 'error'` AND native resources are completely released (`!hasNative || isCleanedUp`).

### 7.3 Modal Dialog Layering & Accessibility (R7)
In `clients/rust/ios-shell/ui/index.html`:
- Move `<div id="modal-connecting">` out of `<section id="view-session" hidden>` to the top-level `<body>` container as a sibling of `<main id="view-connect">` and `<section id="view-session">`.
- When connecting, `modal-connecting` is visible and centered over the background.
- Cancel button `#btn-cancel-connect` receives focus and remains interactive at all times during `connecting` and `disconnecting` phases.
- Clicking `#btn-cancel-connect` invokes `disconnect()`, aborting in-flight operations and returning to `idle` upon cleanup completion.

---

## 8. Network Discovery and Scoped IPv6 Contract (R10)

### 8.1 Scoped Address Type and Preservation
In `clients/rust/erd-net/src/discovery.rs` and `clients/rust/erd-net/src/discovery/apple.rs`:
1. Link-local IPv6 addresses (`fe80::/10`) require a valid nonzero interface scope ID (`scope_id > 0`) to be routable.
2. Endpoint representation preserves scope:
   ```rust
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub struct DiscoveredEndpoint {
       pub ip: IpAddr,
       pub scope_id: Option<u32>,
       pub formatted: String, // e.g. "fe80::1%5" or "192.168.1.50"
   }
   ```
3. `is_usable_ipv6`:
   - Global unicast and ULA IPv6 are accepted without scope.
   - Link-local IPv6 (`fe80::/10`) is accepted **if and only if** `scope_id.is_some_and(|s| s > 0)`.
   - Unscoped link-local IPv6 (`fe80::/10` with `scope == 0` or `None`) is rejected.
4. `parse_service_metadata` receives scoped endpoints; when a scoped IPv6 address is selected, `DiscoveredHost.ip` retains the format `<ipv6>%<scope_id>`.
5. `decide_service_state_action` returns `ServiceStateAction::Publish(host)` with the scoped address intact, ensuring it is not retracted by common validation.

---

## 9. Disjoint Per-Phase File Ownership Matrix

To ensure deterministic, conflict-free implementation across tasks and phases, code ownership is partitioned into strictly disjoint file sets:

```
┌────────────────────────────────────────────────────────────────────────┐
│ Phase A: Trust & Authorization Boundaries                              │
│ - clients/rust/erd-proto/src/pairing.rs                                │
│ - clients/rust/erd-app/src/pairing.rs                                  │
│ - clients/rust/erd-app/src/session.rs (pairing store & DTOs)           │
│ - clients/rust/erd-host/src/session.rs (consent binding & PreAuth TLS) │
│ - clients/rust/erd-host/src/main.rs (random PIN & store setup)         │
│ - clients/rust/tauri-shell/src-tauri/src/lib.rs (commands: pairings)   │
└────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ Phase B: Authenticated UDP Registration                                │
│ - clients/rust/erd-net/src/udp_gcm.rs (registration frame helpers)    │
│ - clients/rust/erd-app/src/session.rs (send authenticated UDP probe)   │
│ - clients/rust/erd-host/src/session.rs (discover_udp_peer & sender)    │
└────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ Phase C: Discovery, Identity & Credential Selection                    │
│ - clients/rust/erd-net/src/discovery.rs (scoped IPv6 validation)       │
│ - clients/rust/erd-net/src/discovery/apple.rs (scope preservation)     │
│ - clients/rust/tauri-shell/src-tauri/src/lib.rs (credential_id lookup) │
│ - clients/rust/tauri-shell/ui/index.html (pass credential_id)          │
│ - clients/rust/ios-shell/src/state.rs (keychain credential_id lookup)  │
│ - clients/rust/ios-shell/ui/connection-state.js (card pairing state)   │
└────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ Phase D: Mobile Lifecycle, Error Handling & UI Modals                  │
│ - clients/rust/ios-shell/src/state.rs (TCP runtime supervisor thread)  │
│ - clients/rust/ios-shell/ui/index.html (modal DOM root repositioning)  │
│ - clients/rust/ios-shell/ui/app.js (modal visibility & focus trap)     │
│ - clients/rust/ios-shell/ui/styles.css (modal layer styling)           │
│ - clients/rust/ios-shell/ui/connection-state.js (pending cleanup)      │
└────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ Phase E: Integration & Regression Verification                         │
│ - Full automated test suites across Rust workspace and Bun harnesses   │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 10. Targeted Regression Test Specifications

The following deterministic regression tests must be executed to verify compliance with contracts R1 through R11.

### 10.1 Phase A: Trust and Authorization (R1, R2, R4, R11)

#### Test A.1: Outbound Pairing Key Rejected by Host Inbound Authorizations (R1)
- **Target:** `erd-app/src/pairing.rs`, `erd-host/src/session.rs`
- **Location:** `clients/rust/erd-host/tests/pairing_isolation.rs`
- **Test Name:** `test_outbound_client_pairing_rejected_as_host_authorization`
- **Command:** `cargo test --test pairing_isolation test_outbound_client_pairing_rejected_as_host_authorization`
- **Assertion:** A key saved via `ClientPairingStore::save` under `client-pairings.json` cannot be loaded or used by `HostAuthorizationStore::load_all`. Host TLS handshake fails with identity unknown.

#### Test A.2: Bootstrap TLS Rejects Application Handshake Without Prior Consent (R2)
- **Target:** `erd-host/src/session.rs`
- **Location:** `clients/rust/erd-host/tests/bootstrap_auth.rs`
- **Test Name:** `test_bootstrap_tls_rejects_handshake_without_consent`
- **Command:** `cargo test --test bootstrap_auth test_bootstrap_tls_rejects_handshake_without_consent`
- **Assertion:** Connecting via `erd-b1` and immediately transmitting `Handshake(existing_id)` yields `SessionError::PreAuth` or connection reset; input injection and media workers are never started.

#### Test A.3: Bootstrap TLS Rejects Application Handshake with Mismatched ID (R2)
- **Target:** `erd-host/src/session.rs`
- **Location:** `clients/rust/erd-host/tests/bootstrap_auth.rs`
- **Test Name:** `test_bootstrap_tls_rejects_mismatched_pairing_id`
- **Command:** `cargo test --test bootstrap_auth test_bootstrap_tls_rejects_mismatched_pairing_id`
- **Assertion:** Client connects via `erd-b1`, requests pairing, receives `PairingGrant(id_1)`, but sends `Handshake(id_2)`. Host closes connection with `IdentityMismatch`.

#### Test A.4: Duplicate Handshake Rejected on Authenticated Session (R2)
- **Target:** `erd-host/src/session.rs`
- **Location:** `clients/rust/erd-host/tests/bootstrap_auth.rs`
- **Test Name:** `test_authenticated_session_rejects_duplicate_handshake`
- **Command:** `cargo test --test bootstrap_auth test_authenticated_session_rejects_duplicate_handshake`
- **Assertion:** Sending a second `Handshake` packet after authentication does not replace ciphers or recreate capture workers; returns error and closes connection.

#### Test A.5: List Pairings IPC Excludes Symmetric Secrets (R4)
- **Target:** `tauri-shell/src-tauri/src/lib.rs`
- **Location:** `clients/rust/tauri-shell/src-tauri/src/pairing_tests.rs`
- **Test Name:** `test_list_pairings_json_excludes_key_field`
- **Command:** `cargo test -p tauri-shell --lib test_list_pairings_json_excludes_key_field`
- **Assertion:** `serde_json::to_string(&commands::list_pairings().unwrap())` produces JSON containing `["id", "hostName", "addedAtUnixMs"]` and zero instances of `"key"`.

#### Test A.6: Host Startup Defaults to Secure Random PIN (R11)
- **Target:** `erd-host/src/main.rs`
- **Location:** `clients/rust/erd-host/tests/pin_policy.rs`
- **Test Name:** `test_default_pin_is_random_and_valid`
- **Command:** `cargo test --test pin_policy test_default_pin_is_random_and_valid`
- **Assertion:** Host started without `--pin` generates an 8-digit PIN matching `/^[0-9]{8}$/` that is not `"12345678"`.

---

### 10.2 Phase B: Authenticated UDP Registration (R3)

#### Test B.1: Host Discards Plaintext UDP Probes (R3)
- **Target:** `erd-host/src/session.rs`
- **Location:** `clients/rust/erd-host/tests/udp_registration.rs`
- **Test Name:** `test_udp_registration_ignores_plaintext_probes`
- **Command:** `cargo test --test udp_registration test_udp_registration_ignores_plaintext_probes`
- **Assertion:** Sending raw `&[0xff]` or arbitrary plaintext from a matching IP leaves `udp_peer` as `None`; no media packets are sent to that socket.

#### Test B.2: Host Discards Packets with Invalid or Tampered GCM Tags (R3)
- **Target:** `erd-host/src/session.rs`
- **Location:** `clients/rust/erd-host/tests/udp_registration.rs`
- **Test Name:** `test_udp_registration_rejects_tampered_ciphertext`
- **Command:** `cargo test --test udp_registration test_udp_registration_rejects_tampered_ciphertext`
- **Assertion:** Sending a datagram with an invalid GCM tag or wrong direction nonce does not register `udp_peer`.

#### Test B.3: Host Registers Endpoint Only Upon Valid Authenticated Ping (R3)
- **Target:** `erd-host/src/session.rs`, `erd-app/src/session.rs`
- **Location:** `clients/rust/erd-host/tests/udp_registration.rs`
- **Test Name:** `test_udp_registration_succeeds_with_session_cipher`
- **Command:** `cargo test --test udp_registration test_udp_registration_succeeds_with_session_cipher`
- **Assertion:** Sending a `Ping` datagram sealed with `c2h_cipher` registers the socket address as `udp_peer`, and subsequent media frames arrive encrypted at that socket.

---

### 10.3 Phase C: Discovery and Scoped IPv6 Endpoints (R8, R9, R10)

#### Test C.1: Apple Scoped IPv6 Link-Local Published (R10)
- **Target:** `erd-net/src/discovery.rs`, `erd-net/src/discovery/apple.rs`
- **Location:** `clients/rust/erd-net/tests/discovery_scoped_ipv6.rs`
- **Test Name:** `test_scoped_ipv6_link_local_published`
- **Command:** `cargo test -p erd-net --test discovery_scoped_ipv6 test_scoped_ipv6_link_local_published`
- **Assertion:** Passing `(IpAddr::V6("fe80::1"), Some(5))` to `decide_service_state_action` yields `ServiceStateAction::Publish(host)` with `host.ip == "fe80::1%5"`.

#### Test C.2: Unscoped IPv6 Link-Local Retracted (R10)
- **Target:** `erd-net/src/discovery.rs`, `erd-net/src/discovery/apple.rs`
- **Location:** `clients/rust/erd-net/tests/discovery_scoped_ipv6.rs`
- **Test Name:** `test_unscoped_ipv6_link_local_retracted`
- **Command:** `cargo test -p erd-net --test discovery_scoped_ipv6 test_unscoped_ipv6_link_local_retracted`
- **Assertion:** Passing `(IpAddr::V6("fe80::1"), None)` or `Some(0)` yields `ServiceStateAction::Retract`.

#### Test C.3: Connect Uses Explicit Credential ID (R8)
- **Target:** `tauri-shell/src-tauri/src/lib.rs`
- **Location:** `clients/rust/tauri-shell/src-tauri/src/connect_tests.rs`
- **Test Name:** `test_connect_selects_explicit_credential_id`
- **Command:** `cargo test -p tauri-shell --lib test_connect_selects_explicit_credential_id`
- **Assertion:** If `credential_id` is supplied, `ClientPairingStore::load(credential_id)` is invoked directly without hostname or prefix search.

---

### 10.4 Phase D: Mobile Lifecycle & UI States (R5, R6, R7)

#### Test D.1: iOS Supervisor Propagates TCP Termination to Session State (R5)
- **Target:** `ios-shell/src/state.rs`
- **Location:** `clients/rust/ios-shell/tests/lifecycle.rs`
- **Test Name:** `test_ios_tcp_disconnect_updates_session_state`
- **Command:** `cargo test -p erd-ios --test lifecycle test_ios_tcp_disconnect_updates_session_state`
- **Assertion:** Closing the remote TCP stream causes `stats().state` to transition from `"ready"` to `"disconnected"` within 100ms; media workers exit cleanly.

#### Test D.2: iOS Cleanup Executed on Error State (R6)
- **Target:** `ios-shell/ui/connection-state.js`
- **Runner:** `bun test`
- **Command:** `bun test clients/rust/ios-shell/ui/test/connection-state.test.mjs`
- **Test Names:**
  - `test('cleanup invokes native disconnect even when state is error')`
  - `test('cleanup failure transitions to cleanup-failed and does not revert to idle')`
  - `test('concurrent disconnect calls share single pending cleanup promise')`

#### Test D.3: Connecting Modal Remains Visible and Accessible in DOM (R7)
- **Target:** `ios-shell/ui/index.html`, `ios-shell/ui/app.js`
- **Runner:** `bun test`
- **Command:** `bun test clients/rust/ios-shell/ui/test/dom-modal.test.mjs`
- **Test Names:**
  - `test('connecting modal is child of body, not hidden session view')`
  - `test('cancel button is clickable and dispatches disconnect while connecting')`

---

## 11. Verification Matrix Summary

| Finding | Priority | Phase | Implementation Target | Deterministic Test Command |
|---|---|---|---|---|
| **R1** | P1 | Phase A | `erd-app/src/pairing.rs`, `erd-host/src/session.rs` | `cargo test --test pairing_isolation` |
| **R2** | P1 | Phase A | `erd-host/src/session.rs` | `cargo test --test bootstrap_auth` |
| **R3** | P1 | Phase B | `erd-host/src/session.rs`, `erd-app/src/session.rs` | `cargo test --test udp_registration` |
| **R4** | P1 | Phase A | `tauri-shell/src-tauri/src/lib.rs` | `cargo test -p tauri-shell --lib test_list_pairings` |
| **R5** | P1 | Phase D | `ios-shell/src/state.rs` | `cargo test -p erd-ios --test lifecycle` |
| **R6** | P1 | Phase D | `ios-shell/ui/connection-state.js` | `bun test clients/rust/ios-shell/ui/test/connection-state.test.mjs` |
| **R7** | P1 | Phase D | `ios-shell/ui/index.html`, `ios-shell/ui/app.js` | `bun test clients/rust/ios-shell/ui/test/dom-modal.test.mjs` |
| **R8** | P2 | Phase C | `tauri-shell/src-tauri/src/lib.rs` | `cargo test -p tauri-shell --lib test_connect_selects` |
| **R9** | P2 | Phase C | `ios-shell/src/state.rs`, `ios-shell/ui/connection-state.js` | `bun test clients/rust/ios-shell/ui/test/discovery.test.mjs` |
| **R10** | P2 | Phase C | `erd-net/src/discovery.rs`, `apple.rs` | `cargo test -p erd-net --test discovery_scoped_ipv6` |
| **R11** | P1 | Phase A | `erd-host/src/main.rs`, `tauri-shell/src-tauri/src/lib.rs` | `cargo test --test pin_policy` |
