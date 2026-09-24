# Phase C Shared Credential Metadata & IPC Error API Contract

- Date: 2026-09-10
- Task ID: `st_01a089dd`
- Status: Implemented & Verified GREEN on Omarchy (`indo@100.91.254.71`)
- Scope: `erd-app/src/pairing.rs`, `erd-app/src/error.rs`, `erd-app/src/lib.rs`, associated tests, and mechanical `erd_app::PairingRecord` updates across dependent callers/fixtures.

---

## 1. Exported Public API Specification

All types are exported from `erd_app` (`clients/rust/erd-app`).

### 1.1 `PairingEndpoint`

Represents a concrete validated transport endpoint for a paired host.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingEndpoint {
    pub host: String,
    pub tcp_port: u16,
    pub udp_port: u16,
}

impl PairingEndpoint {
    pub fn new(host: impl Into<String>, tcp_port: u16, udp_port: u16) -> Self;
}
```

#### JSON Representation
```json
{
  "host": "192.168.1.50",
  "tcpPort": 19730,
  "udpPort": 19731
}
```
- `host`: Raw transport IP/hostname string, preserving IPv6 interface scope if applicable (e.g. `"[fe80::1%en0]"`).
- `tcpPort` / `udpPort`: Concrete validated communication ports.

---

### 1.2 `PairingRecord` (Client Outbound Storage Record)

Preserves existing `id`, `name`, Base64 `key`, and `addedAt` fields. Adds backward-compatible optional endpoint metadata.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingRecord {
    pub id: String,
    pub name: String,
    #[serde(deserialize_with = "deserialize_key", serialize_with = "serialize_key")]
    pub key: Vec<u8>,
    #[serde(
        rename = "addedAt",
        default,
        deserialize_with = "deserialize_added_at",
        serialize_with = "serialize_added_at",
        alias = "addedAt",
        alias = "added_at",
        alias = "added_at_unix_ms",
        alias = "addedAtUnixMs"
    )]
    pub added_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_endpoint: Option<PairingEndpoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoint_aliases: Vec<PairingEndpoint>,
}

impl PairingRecord {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        key: Vec<u8>,
        added_at_unix_ms: u64,
    ) -> Self;

    pub fn with_endpoints(
        mut self,
        last_endpoint: Option<PairingEndpoint>,
        endpoint_aliases: Vec<PairingEndpoint>,
    ) -> Self;

    pub fn summary(&self) -> PairingSummary;

    pub fn key_array(&self) -> Result<[u8; PAIRING_KEY_SIZE], PairingStoreError>;
}
```

#### Invariants & Backward Compatibility:
- **Legacy Records**: Deserializing JSON records lacking `lastEndpoint` and `endpointAliases` produces `last_endpoint: None` and `endpoint_aliases: Vec::new()`.
- **Serialization**: When `last_endpoint` is `None` or `endpoint_aliases` is empty, those fields are omitted from disk serialization, preserving backwards schema compatibility.
- **Key & Time**: Preserves standard Base64 encoding for 32-byte symmetric keys and Swift reference date / Unix millisecond compatibility on `addedAt`.

---

### 1.3 `PairingSummary` (Shared Key-Free Public DTO)

Safe projection for WebView IPC crossing. Never exposes long-term 256-bit symmetric keys. Re-exported by `tauri-shell` and `ios-shell`.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingSummary {
    pub id: String,
    pub host_name: String,
    pub added_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_endpoint: Option<PairingEndpoint>,
}

impl From<PairingRecord> for PairingSummary;
impl From<&PairingRecord> for PairingSummary;
```

#### JSON Representation
```json
{
  "id": "A1B2C3D4-E5F6-7890-ABCD-EF1234567890",
  "hostName": "MyWorkstation",
  "addedAtUnixMs": 1725900000000,
  "lastEndpoint": {
    "host": "100.91.254.71",
    "tcpPort": 19730,
    "udpPort": 19731
  }
}
```
When `last_endpoint` is `None`, `lastEndpoint` is omitted, producing only `["addedAtUnixMs", "hostName", "id"]`.

---

### 1.4 Exact-ID / Key-Matched Metadata Update: `PairingStore::remember_endpoint`

Updates endpoint hints on a verified record without risk of record resurrection or credential corruption.

```rust
impl PairingStore {
    pub fn remember_endpoint(
        &self,
        id: &str,
        expected_key: &[u8],
        endpoint: PairingEndpoint,
    ) -> Result<bool, PairingStoreError>;
}
```

#### Behavioral Contract:
1. **Missing Record**: If `load(id)` is `None`, returns `Ok(false)`. Does NOT resurrect or create a record.
2. **Key Mismatch**: If `record.key != expected_key`, returns `Ok(false)`. Does NOT mutate stored record.
3. **Exact Match**:
   - Deduplicates: Removes `endpoint` from `endpoint_aliases`.
   - Shifts previous `last_endpoint` (if present and distinct from `endpoint` and not already in aliases) into `endpoint_aliases`.
   - Sets `record.last_endpoint = Some(endpoint)`.
   - Preserves `id`, `name`, `key`, and `added_at_unix_ms` intact.
   - Persists to the underlying store (`File`, `Ephemeral`, or `Keychain`).
   - Returns `Ok(true)`.
4. **No Mandatory Store Access**: Calling `ClientSession::connect_with_pairing` directly (e.g. for CLI/QA with explicit PSK) does NOT require or trigger `remember_endpoint`. Callers invoke `remember_endpoint` explicitly after successful authentication in stored-credential flows.

---

### 1.5 Shared Typed IPC Errors (`erd_app::error`)

Provides machine-readable error codes and stages across desktop and mobile shells without relying on prose string parsing.

#### `IpcErrorCode`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IpcErrorCode {
    PairingRequired,     // "pairing-required"
    PairingDenied,       // "pairing-denied"
    PairingLockedOut,    // "pairing-locked-out"
    PairingDisabled,     // "pairing-disabled"
    CredentialRejected,  // "credential-rejected"
    InvalidPin,          // "invalid-pin"
    ConsentTimeout,      // "consent-timeout"
    HandshakeTimeout,    // "handshake-timeout"
    RemoteClosed,        // "remote-closed"
    NetworkUnreachable,  // "network-unreachable"
    CleanupFailed,       // "cleanup-failed"
    Cancelled,           // "cancelled"
    ConnectionFailed,    // "connection-failed" (general fallback)
    IncompatiblePeer,    // "incompatible-peer" (missing capabilities, e.g. R3)
}
```

#### `IpcErrorStage`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IpcErrorStage {
    Client,              // "client"
    Connect,             // "connect"
    Preauth,             // "preauth"
    #[serde(rename = "tls-psk")]
    TlsPsk,              // "tls-psk"
    Handshake,           // "handshake"
    Runtime,             // "runtime"
    Cleanup,             // "cleanup"
}
```

#### `IpcError`
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: IpcErrorCode,
    pub message: String,
    pub stage: IpcErrorStage,
    pub retryable: bool,
}

impl IpcError {
    pub fn new(code: IpcErrorCode, stage: IpcErrorStage, message: impl Into<String>) -> Self;
    pub fn with_retryable(mut self, retryable: bool) -> Self;
    pub fn pairing_required(message: impl Into<String>) -> Self;
    pub fn credential_rejected(message: impl Into<String>) -> Self;
    pub fn invalid_pin(message: impl Into<String>) -> Self;
    pub fn incompatible_peer(message: impl Into<String>) -> Self;
    pub fn connection_failed(stage: IpcErrorStage, message: impl Into<String>) -> Self;
}
```

#### JSON Representation
```json
{
  "code": "pairing-required",
  "message": "PIN required for initial authorization",
  "stage": "preauth",
  "retryable": false
}
```

---

## 2. Updated Callers & Fixtures (Mechanical Literal Updates)

The following locations had their `erd_app::PairingRecord` literal initializations mechanically updated to include default endpoint metadata (`last_endpoint: None, endpoint_aliases: Vec::new()`):

1. `clients/rust/erd-app/src/session.rs` (lines 264, 1385)
2. `clients/rust/erd-app/src/tcp_write_tests.rs` (line 68)
3. `clients/rust/erd-app/src/bin/erd_client.rs` (line 459)
4. `clients/rust/erd-app/tests/receiver_telemetry.rs` (line 81)
5. `clients/rust/erd-app/tests/session_mock.rs` (lines 233, 328, 419, 501, 583, 675)
6. `clients/rust/erd-mobile/src/storage.rs` (lines 487, 512)
7. `clients/rust/tauri-shell/src-tauri/src/discovery_tests.rs` (line 66)
8. `clients/rust/tauri-shell/src-tauri/src/mailbox_tests.rs` (line 77)
9. `clients/rust/tauri-shell/src-tauri/src/desktop_integration_tests.rs` (lines 512, 518)
10. `clients/rust/tauri-shell/src-tauri/src/lib.rs` (re-exports `erd_app::PairingSummary`)
11. `clients/rust/ios-shell/src/tests.rs` (line 317)
12. `clients/rust/erd-host/tests/pairing_isolation.rs` (line 46)

---

## 3. Test Verification Evidence

Executed on Omarchy (`indo@100.91.254.71`) in `/home/indo/projects/erd-pairing-20260910`:

### Unit Tests
- `cargo test -p erd-app --lib pairing::tests`:
  - `test_client_default_path_filename`: ok
  - `test_legacy_record_readability`: ok
  - `test_metadata_roundtrip_and_dedup`: ok
  - `test_preservation_of_credentials`: ok
  - `test_missing_record_no_resurrection`: ok
  - `test_id_key_mismatch_no_mutation`: ok
  - `test_key_free_summary_serialization`: ok
  - `test_legacy_client_migration_skips_when_client_pairings_already_exists`: ok
  - `test_legacy_client_migration_preserves_legacy_and_writes_client_pairings`: ok
  - `test_deletion_isolated_from_legacy`: ok
  - `test_temporary_file_naming_isolated`: ok
  - `ephemeral_pairing_store_roundtrip_and_delete`: ok
  - **12 passed; 0 failed**
- `cargo test -p erd-app --lib error::tests`:
  - `test_ipc_error_code_and_stage_serialization`: ok
  - `test_ipc_error_machine_fields_and_no_secret_leak`: ok
  - **2 passed; 0 failed**

### Workspace Regression Suites
- `erd-app`: 186 passed; 0 failed (133 lib, 16 bin, 37 integration tests)
- `erd-mobile`: 60 passed; 0 failed (28 lib, 32 integration tests)
- `erd-host --test pairing_isolation`: 2 passed; 0 failed
- `tauri-shell --lib pairing`: 11 passed; 0 failed (including key-free summary checks)

### Strict Clippy Gate
- Command: `cargo clippy --manifest-path clients/rust/Cargo.toml -p erd-app -p erd-mobile -p tauri-shell -p erd-host --no-deps -- -D warnings`
- Result: **0 warnings, 0 errors** (exit code 0)
