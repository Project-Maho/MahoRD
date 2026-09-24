# Phase C Task Implementation Report: R9 iOS Explicit Saved-Credential Reconnect

- Task ID: `st_01a089e7`
- Goal: Implement R9 iOS explicit saved-credential reconnect across native Rust and UI TypeScript.
- Worker: `hephaestus`
- Session: `01a0890a-69f1-7e5e-80cb-959dc3ddb61c` (Depth: 1)
- Date: 2026-09-10
- Base Commits: Accepted A/B increments (`0eafe10`, `9a364a9`, `fa476b1`, `8c4570e`)
- Target Remote: `indo@100.91.254.71` (`/home/indo/projects/erd-pairing-20260910`)
- Environment: `PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:$LD_LIBRARY_PATH`
- Deliverable: Tested Native & UI Code + Scoped Verifications + Report (`.omo/pairing-20260910/reports/c-ios.md`)

---

## 1. Executive Summary

Implemented, hardened, and verified finding **R9** (iOS explicit saved-credential reconnect and exact-ID Keychain flow) across native commands, state management, UI controllers, QA provisioning interfaces, and test harnesses:
1. `clients/rust/ios-shell/src/commands.rs`: Added flat `pairing_id` to `connect`, added key-free `list_pairings` and `forget_pairing` commands, returning typed `IpcError`.
2. `clients/rust/ios-shell/src/state.rs`: Exact-ID Keychain lookup via `store.load`, eliminated `find_by_host` and name-based fallback entirely, verified missing/unknown ID rejection before starting transport or bootstrap, persisted validated endpoints via `store.remember_endpoint` upon authentication, and classified session/IO errors into machine-readable `IpcError` types. Preserved `lifecycle_lock` and `connect_async` serialized execution.
3. `clients/rust/ios-shell/src/lib.rs`: Registered `list_pairings` and `forget_pairing`, re-exported public `IpcError`, `IpcErrorCode`, `IpcErrorStage`, `PairingEndpoint`, and `PairingSummary`.
4. `clients/rust/ios-shell/src/qa.rs`: Standardized debug QA provisioning on the SAME production client/Keychain store (`PairingStore::open_default()`), guaranteed import failure is never masked as success, and propagated explicit `pairing_id`, `tcp_port`, and `udp_port` through `StartupResponse` without exposing secrets or PINs.
5. `clients/rust/ios-shell/src/tests.rs`: Added 6 dedicated R9 regressions proving pre-transport rejection of missing/unknown IDs, exact-ID lookup without host fallback, secret-free summary serialization, endpoint metadata reload, and QA provisioning propagation.
6. `clients/rust/ios-shell/ui/connection-state.js`: Extended `validateConnectRequest` with `pairingId`, implemented `savedPairings` collection, `refreshPairings()`, `forgetPairing(id)`, `connectSavedPairing(pairingId)`, and ensured discovered hosts never inherit or select credentials.
7. `clients/rust/ios-shell/ui/app.js`: Rendered saved connections section with distinct card metadata, wired saved-mode click to connect without PIN passing exact `pairingId`, preserved scoped host/ports, and consumed explicit `pairingId` and ports from `startup`.
8. `clients/rust/ios-shell/ui/index.html` & `styles.css`: Added markup and styles for `#section-pairings` and `.pairing-card`.
9. `clients/rust/ios-shell/ui/test/connection-state.test.mjs` & `discovery.test.mjs`: Added unit and integration tests covering saved-mode click invoke argument, same-name non-inference, key exclusion, missing-credential rejection, and startup auto-connect propagation.
10. **Physical-Target Build Proof**: Verified physical-iOS compilation on Mac via `cargo build --manifest-path clients/rust/Cargo.toml --target aarch64-apple-ios -p erd-ios` (exit code 0; produced `liberd_ios_lib.a` and `erd-ios`).

---

## 2. Interface and Contract Compliance

### 2.1 Flat Arguments and Native IPC Commands (`commands.rs` & `lib.rs`)

```rust
#[tauri::command]
pub fn list_pairings() -> Result<Vec<PairingSummary>, IpcError>;

#[tauri::command]
pub fn forget_pairing(id: String) -> Result<(), IpcError>;

#[tauri::command]
pub async fn connect(
    state: State<'_, AppState>,
    host: String,
    tcp_port: Option<u16>,
    udp_port: Option<u16>,
    pin: Option<String>,
    pairing_id: Option<String>,
) -> Result<SessionStats, IpcError>;
```

- Flat invoke signature preserved: `host, tcp_port, udp_port, pin, pairing_id` (Rust snake_case) and `host, tcpPort, udpPort, pin, pairingId` (JS camelCase).
- `list_pairings`: Queries `PairingStore::open_default()`, loads all client records, and converts them to key-free `PairingSummary`. Zero symmetric keys or secret fields exposed.
- `forget_pairing`: Deletes a pairing by ID from the store without touching legacy files or host records.

### 2.2 Exact-ID Keychain Lookup & Zero Pre-Transport Fallback (`state.rs`)

1. **Pre-Transport Credential Verification**:
   - If explicit PIN is absent (`pin.is_none()` or empty) and `pairing_id` is absent:
     Returns `Err(IpcError::pairing_required("PIN required for initial authorization"))` immediately. No network sockets are opened, and no bootstrap packets are transmitted.
   - If explicit PIN is absent and `pairing_id` is supplied:
     Queries `store.load(id.trim())`.
     If `record.is_none()`, immediately returns `Err(IpcError::pairing_required(format!("Unknown pairing ID '{id}'; PIN required")))`.
     Zero network sockets are opened; no fallback to `find_by_host`, host name, or IP.
2. **Removal of `find_by_host`**:
   The legacy `connect_via_keychain` method that searched by `host` has been removed. Credentials can only be loaded by exact, matching `pairing_id`.
3. **Session Reconnection**:
   When exact `PairingRecord` is loaded, connects via `session.connect_with_pairing(record)`.
4. **Lifecycle Lock**:
   Existing `lifecycle_lock: Arc<tokio::sync::Mutex<()>>` and `disconnect_blocking` invocation at start of `connect_blocking` are preserved intact.

### 2.3 Verified Endpoint Persistence (`state.rs`)

Upon successful authentication (either via `pair_with_pin` or `connect_with_pairing`), the validated endpoint is recorded:

```rust
let endpoint = PairingEndpoint::new(
    config.host.clone(),
    config.tcp_port,
    config.udp_port,
);
let store = match PairingStore::open_default() {
    Ok(s) => s,
    Err(e) => {
        let _ = session.disconnect();
        let err = IpcError::connection_failed(IpcErrorStage::Client, format!("Failed to open pairing store: {e}"));
        self.set_error(current_generation, err.message.clone());
        self.disconnect_blocking().ok();
        return Err(err);
    }
};
match store.remember_endpoint(&ready_session.pairing.id, &ready_session.pairing.key, endpoint) {
    Ok(persisted) => {
        if !persisted {
            tracing::warn!(id = %ready_session.pairing.id, "remember_endpoint returned false (key mismatch or deleted record)");
        }
    }
    Err(e) => {
        let _ = session.disconnect();
        let err = IpcError::connection_failed(IpcErrorStage::Client, format!("Failed to persist endpoint metadata: {e}"));
        self.set_error(current_generation, err.message.clone());
        self.disconnect_blocking().ok();
        return Err(err);
    }
}
```

- Preserves existing credentials without key replacement.
- Deduplicates prior aliases and updates `last_endpoint`.
- Any failure in `remember_endpoint` cleans up the provisional session and returns typed `IpcError`, preventing false-connected UI states.

### 2.4 Error Domain Classification (`state.rs`)

`classify_session_error(err: &SessionError) -> IpcError` maps internal errors to machine-readable types:
- `SessionError::PairingRejected(PairingRejectReason::DeniedByHost)` -> `IpcErrorCode::PairingDenied` (stage `Preauth`)
- `SessionError::PairingRejected(PairingRejectReason::LockedOut)` -> `IpcErrorCode::PairingLockedOut` (stage `Preauth`)
- `SessionError::PairingRejected(PairingRejectReason::PairingDisabled)` -> `IpcErrorCode::PairingDisabled` (stage `Preauth`)
- `SessionError::PairingNotFound` -> `IpcErrorCode::PairingRequired` (stage `Preauth`)
- `SessionError::HandshakeAckTimeout` -> `IpcErrorCode::HandshakeTimeout` (stage `Handshake`)
- `SessionError::MissingAuthenticatedRegistration` -> `IpcErrorCode::IncompatiblePeer` (stage `Handshake`)
- `SessionError::Io(ErrorKind::ConnectionRefused | HostUnreachable | NetworkUnreachable)` -> `IpcErrorCode::NetworkUnreachable` (stage `Connect`)
- `SessionError::Io(ErrorKind::ConnectionReset | UnexpectedEof | BrokenPipe)` -> `IpcErrorCode::RemoteClosed`
- Other errors fall back to `IpcErrorCode::ConnectionFailed` with appropriate stages.
- Zero secret keys or plaintext PINs included in error strings.

### 2.5 UI Credential Selection and Untrusted Discovery Isolation (`connection-state.js` & `app.js`)

1. **Saved Connections Section**:
   `#section-pairings` renders saved pairings separately from nearby discovered hosts.
   Each card displays host name, unique ID, and last verified endpoint.
2. **Explicit Selection Mode**:
   Clicking a saved pairing calls `selectPairing(pairing)`.
   Populates host input with `pairing.lastEndpoint.host`, clears PIN input, and sets `selectedPairing`.
   Submitting connects via `connectSavedPairing(pairingId)` with `pin: null` and `pairingId: selectedPairing.id`.
3. **Discovered Host Non-Inheritance**:
   Clicking a discovered host card calls `selectDiscoveredHost(host)`.
   Crucially, `selectedPairing` is set to `null`. Even if the discovered host has the exact same name as a saved pairing, it is treated as untrusted and requires manual PIN input for connection. No pairing ID is inherited.

### 2.6 QA Provisioning and Startup Interface (`qa.rs` & `app.js`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StartupResponse {
    pub host: Option<String>,
    #[serde(default)]
    pub tcp_port: Option<u16>,
    #[serde(default)]
    pub udp_port: Option<u16>,
    #[serde(default)]
    pub pairing_id: Option<String>,
    #[serde(default, alias = "auto_connect")]
    pub auto_connect: bool,
}
```

- **Production Store Alignment**: Uses `erd_app::PairingStore::open_default()`—the exact same client/Keychain store as reconnection. Removed redundant parallel writes to `MobilePairingStore`.
- **Integrity**: If `store.save()` fails, import is treated as failed: error is logged, file is not deleted, and `auto_connect: false` is returned.
- **Explicit Propagation**: Propagates `pairing_id`, `tcp_port`, and `udp_port` to `StartupResponse`.
- **Zero Secrets**: Contains zero key bytes or PINs.
- **Backward Compatibility**: Old format with only `host` and `pairing` parses transparently via `#[serde(default)]`.
- **UI Controller Integration**: On `startup` resolution in `app.js`, if `auto_connect` and `host` are present, dispatches connection with explicit `pairingId` and ports without guessing from host or hostname.

---

## 3. Pre-Action Pinned RED Evidence

All new regression targets and RED invocations were pinned in `/var/folders/zh/7cc25lt91b1_dj577306nwdh0000gn/T/ulw-20260910-111421.XXXXXX.md.pNqwGP19PH` before modifying production code.

### 3.1 Rust RED Invocations
- **Target Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-ios --lib test_connect"
  ```
- **Exit Code**: `101`
- **Failure Output**:
  ```text
  error[E0061]: this method takes 4 arguments but 5 arguments were supplied
     --> ios-shell/src/tests.rs:342:14
      |
  342 |             .connect_async("127.0.0.1".to_string(), Some(19730), Some(19731), None, None)
      |              ^^^^^^^^^^^^^                                                          ---- unexpected argument #5 of type `std::option::Option<_>`
  error[E0609]: no field `code` on type `std::string::String`
  error[E0609]: no field `stage` on type `std::string::String`
  error[E0609]: no field `retryable` on type `std::string::String`
  error[E0061]: this method takes 4 arguments but 5 arguments were supplied (line 355)
  error: could not compile `erd-ios` (lib test) due to 8 previous errors
  ```
- **Confirmation**: Confirmed that prior `connect_async` rejected the 5th argument (`pairing_id`) and returned bare `String` instead of structured `IpcError`.

### 3.2 JS RED Invocations
- **Target Command**:
  ```bash
  bun test clients/rust/ios-shell/ui/test/connection-state.test.mjs clients/rust/ios-shell/ui/test/discovery.test.mjs
  ```
- **Exit Code**: `1`
- **Failure Output**:
  ```text
  TypeError: manager.refreshPairings is not a function. (In 'manager.refreshPairings()', 'manager.refreshPairings' is undefined)
  (fail) unpaired discovered host with identical name does not choose saved credential
  (fail) saved-mode click passes explicit pairingId to native connect without PIN
  (fail) list_pairings populates savedPairings and excludes secret keys from state
  3 tests failed. 29 pass. 3 fail.
  ```
- **Confirmation**: Confirmed that prior UI manager lacked pairing metadata collection, saved-mode connection handling, and pairing state.

---

## 4. Post-Action GREEN Evidence

### 4.1 Remote Rust iOS Crate Tests (Omarchy `indo@100.91.254.71`)

- **Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-ios"
  ```
- **Exit Code**: `0`
- **Output**:
  ```text
     Compiling erd-ios v0.1.0 (/home/indo/projects/erd-pairing-20260910/clients/rust/ios-shell)
      Finished `test` profile [unoptimized + debuginfo] target(s) in 6.29s
       Running unittests src/lib.rs (clients/rust/target/debug/deps/erd_ios_lib-08638918e6826959)

  running 21 tests
  test tests::tests::build_session_config_rejects_empty_bracketed_hosts_and_zero_ports ... ok
  test tests::tests::build_session_config_normalizes_ipv6_brackets_and_preserves_scope ... ok
  test tests::tests::frame_repacking_exact_layout_and_header ... ok
  test tests::tests::frame_repacking_odd_height ... ok
  test tests::tests::cancellation_validation_accepts_nonfinite_coords_to_release ... ok
  test tests::tests::key_validation_rejects_invalid_inputs ... ok
  test tests::tests::frame_repacking_stride_repack ... ok
  test tests::tests::poisoned_mutex_returns_error_instead_of_silent_recovery ... ok
  test tests::tests::presentation_sequence_validation ... ok
  test tests::tests::session_stats_serialization_conforms_to_contract ... ok
  test tests::tests::pairing_lookup_by_host_address_never_infers_pairing_from_advertised_hostname ... ok
  test tests::tests::trackpad_normalized_drag_produces_pixel_movement ... ok
  test tests::tests::touch_mode_switching_emits_release ... ok
  test tests::tests::test_list_pairings_excludes_secret_key_bytes ... ok
  test tests::tests::touch_cancellation_emits_release ... ok
  test tests::tests::test_connect_exact_id_loads_correct_stored_pairing ... ok
  test tests::tests::touch_unit_viewport_normalization ... ok
  test tests::tests::test_connect_missing_id_and_missing_pin_fails_before_transport ... ok
  test tests::tests::test_remember_endpoint_survives_store_reload ... ok
  test tests::tests::test_connect_unknown_id_fails_before_transport_without_bootstrap ... ok
  test tests::tests::test_qa_provisioning_imports_and_propagates_pairing_id ... ok

  test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
  ```

### 4.2 Remote Strict Clippy Gate
- **Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo clippy --manifest-path clients/rust/Cargo.toml -p erd-ios --no-deps -- -D warnings"
  ```
- **Exit Code**: `0`
- **Output**: 0 warnings, 0 errors.

### 4.3 Bun Local UI Test Suite (Mac)
- **Command**:
  ```bash
  bun test clients/rust/ios-shell/ui/test/
  ```
- **Exit Code**: `0`
- **Summary**: 59 passed, 0 failed across 7 test files (1264ms).
  - `frame-parser.test.mjs`: 5 passed
  - `discovery.test.mjs`: 25 passed (including identical name non-inheritance)
  - `connection-state.test.mjs`: 9 passed (including saved-mode click argument passing, secret key exclusion, pairing-required on missing credentials, and startup provisioning auto-connect propagation)
  - `input-queue.test.mjs`: 4 passed
  - `lifecycle.test.mjs`: 5 passed
  - `keyboard-mapper.test.mjs`: 4 passed
  - `touch-coords.test.mjs`: 7 passed

### 4.4 Physical-Target Compilation Proof on macOS (aarch64-apple-ios)
- **Command**:
  ```bash
  cargo build --manifest-path clients/rust/Cargo.toml --target aarch64-apple-ios -p erd-ios
  ```
- **Exit Code**: `0` (finished in 1m 01s)
- **Produced Artifacts**:
  - `clients/rust/target/aarch64-apple-ios/debug/liberd_ios_lib.a` (480 MB static library)
  - `clients/rust/target/aarch64-apple-ios/debug/erd-ios` (31 MB executable)
- **Confirmation**: Following the discovery owner's fix in `erd-net/src/discovery/apple.rs`, the full native iOS binary builds cleanly for physical ARM64 iOS devices without compilation errors or link failures.

---

## 5. Physical Device Build & Runtime Verification Status

In accordance with instructions ("State plainly which Keychain/device runtime proof remains unavailable rather than calling a mock physical proof"):

### 5.1 Device Target & Provisioning Prerequisites
- **Connected Devices (`devicectl`)**:
  - iPhone 16 Plus: `F1C581E0-A54E-5E85-8013-4F02DF80F98B`
  - iPhone 12 Pro: `DBB7A424-6196-5A4D-88EA-CE441C4B8132`
- **Signing & Entitlements**:
  - Generated entitlements are currently empty.
  - `ExportOptions.plist` method is set to `debugging`.
  - Xcode project at `clients/rust/ios-shell/gen/apple/erd-ios.xcodeproj`.

### 5.2 Evidence Requiring Physical iPhone (Phase E / Lead Verification)
The following runtime properties require execution on physical iOS hardware with valid provisioning profiles and cannot be validated via local unit tests or headless runners:
1. **Hardware-Backed Secure Keychain**:
   - Hardware-backed AES Keychain encryption under `kSecAttrAccessibleAfterFirstUnlock`.
   - Migration of existing legacy Keychain items in `com.eclipticrd.ios.pairing` service on physical iOS devices.
   - Persistence of endpoint metadata across application terminations and device reboots when restarted without the QA provisioning file.
2. **Physical Touch & WebKit WebView Gestures**:
   - Real touch-down to frame presented latency on ProMotion 120Hz physical displays.
   - Backgrounding behavior under iOS SpringBoard lifecycle (`UIApplicationWillResignActiveNotification`).
3. **Assigned Phase**: Physical iOS deployment, provisioning, and end-to-end device testing belong to **Phase E** and coordinator final acceptance.

---

## 6. Deliverable Checklist & Audit

- [x] ONE GOAL: Implement R9 iOS explicit saved-credential reconnect.
- [x] DELIVERABLE: Tested native/UI code, scoped evidence, and `.omo/pairing-20260910/reports/c-ios.md`.
- [x] SCOPE: `ios-shell/src/commands.rs`, `state.rs`, `lib.rs`, `qa.rs`, associated tests, and `ios-shell` UI connection/discovery/credential selection files/tests.
- [x] Did not alter desktop, core metadata, discovery, or separate lifecycle implementation.
- [x] Kept `lifecycle_lock` and existing `connect_async` ownership and serialization.
- [x] Added flat `pairing_id` / `pairingId`.
- [x] Added key-free `list_pairings` and `forget_pairing` commands.
- [x] Exact-ID Keychain lookup via `store.load` without `find_by_host` fallback.
- [x] Successful endpoint persistence through shared `store.remember_endpoint` API.
- [x] Typed IPC errors with machine codes, stages, and zero secret leakage.
- [x] QA provisioning uses same production store, never masks save errors, and propagates `pairing_id` and ports without secrets.
- [x] UI supports saved-ID mode without PIN, fresh pairing with explicit PIN, and preserves scoped host/ports.
- [x] Same-name discovery cannot choose credentials.
- [x] Proved missing/unknown IDs cause no transport/bootstrap.
- [x] Proved correct stored ID is used, metadata survives reload, and no secret fields enter JS.
- [x] Covered UI saved-mode click -> actual invoke argument.
- [x] Verified on Omarchy Linux Rust test suite (21 passed) and local Bun test suite (59 passed).
- [x] Verified physical iOS compilation on Mac (`aarch64-apple-ios` staticlib and binary built cleanly).
- [x] Honestly recorded remaining physical device runtime evidence.
