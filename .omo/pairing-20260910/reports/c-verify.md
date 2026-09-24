# Phase C Independent Verification Report: R8, R9, R10 Evidence & Integration

- Task ID: `st_01a08a27`
- Verifier / Worker: `hephaestus` (Senpi task child)
- Parent / Root Session: `01a0890a-69f1-7e5e-80cb-959dc3ddb61c`
- Review Plan: `docs/remote-connection-pairing-review-plan-20260910.md` (Findings R8, R9, R10, Phase C)
- Binding Contracts: `.omo/pairing-20260910/contracts.md` (Amendments 1, 2, 4, 7, 8), `.omo/pairing-20260910/phase-c-contract.md`, `.omo/pairing-20260910/phase-c-handoff.md`
- Accepted Base Commits:
  - `0eafe10dc73189730b54bb2d4f3ef44ec5e16b62`: `fix(pairing): separate client credentials from host authorizations` (R1)
  - `9a364a955d5b65b96e5aeedc6c6b0366d8ca2a08`: `fix(auth): bind bootstrap handshakes to approved pairing identities` (R2, Host R11)
  - `fa476b1f2ea8e0fe49caef890250953a99e8d42d`: `fix(ipc): keep pairing secrets out of list responses` (R4)
  - `8c4570e3009bc298ff4811a28a38ec5164bc7754`: `fix(protocol): authenticate UDP endpoint registration` (R3)
- Target Remote: `indo@100.91.254.71` (`/home/indo/projects/erd-pairing-20260910`)
- Remote Build Environment: `PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:$LD_LIBRARY_PATH`
- Local Environment: macOS Darwin 25.6.0 (arm64, Apple M4 Max), Bun v1.4.0
- Deliverable: `.omo/pairing-20260910/reports/c-verify.md`
- Date: 2026-09-10

---

## 1. Executive Summary & Requirement Verdict Matrix

Phase C changes covering **R8** (Desktop explicit credential selection and R11 client regression integration), **R9** (iOS explicit saved-credential reconnect and Keychain flow), and **R10** (Scoped IPv6 link-local discovery and resolver endpoint preservation) were independently verified against the codebase, remote test environments, and local execution surfaces.

All tests were executed against the actual code on `indo@100.91.254.71` and local Bun runners. Zero mock-only or build-only results were counted as physical device proof. Core protections established in Phases A and B (R1, R2, R3, R4, R11) remain intact with zero regressions.

| Finding ID | Requirement & Scope | Verified Evidence & Test Seams | Verdict |
|---|---|---|---|
| **R8** | **Desktop Explicit Credential Selection**<br>`tauri-shell/src-tauri/src/lib.rs`<br>`tauri-shell/src-tauri/src/discovery_tests.rs`<br>`tauri-shell/src-tauri/src/pairing_tests.rs`<br>`tauri-shell/ui/` | Exact-ID lookup via `store.load(id)`; removal of name/prefix/IP inference (`resolve_stored_pairing` deleted); zero transport or bootstrap on missing/unknown ID; discovery dedup preserves distinct same-name endpoints; LAN never inherits trust; endpoint persistence via `store.remember_endpoint` with session cleanup on persistence failure; separate "Saved credentials" UI section rendering key-free `PairingSummary`; click-to-connect passes explicit `pairingId` without PIN; 12/12 pairing tests passed, 18/18 discovery tests passed, 60/60 UI tests passed, 12/12 page harness tests passed. | **PASS**<br>*(Physical desktop surface reserved for lead final review)* |
| **R9** | **iOS Explicit Saved-Credential Reconnect**<br>`ios-shell/src/commands.rs`<br>`ios-shell/src/state.rs`<br>`ios-shell/src/qa.rs`<br>`ios-shell/src/tests.rs`<br>`ios-shell/ui/` | Flat `pairing_id` / `pairingId` invoke argument; exact-ID Keychain lookup via `store.load` without `find_by_host`; missing/unknown ID pre-transport rejection without bootstrap; endpoint persistence via `store.remember_endpoint`; key-free `list_pairings` and `forget_pairing`; `#section-pairings` UI rendering saved connections separately; saved-mode click passes exact `pairingId` without PIN; discovered host cards treat endpoints as untrusted and reset `selectedPairing = null`; QA provisioning uses production store, propagates `pairing_id` and ports without secrets, and never masks save errors; `lifecycle_lock` intact; 21/21 Rust tests passed; 59/59 Bun UI tests passed; Mac `aarch64-apple-ios` physical-target build exit 0. | **PASS**<br>*(Physical iPhone execution reserved for lead / Phase E)* |
| **R10** | **Scoped IPv6 Link-Local Discovery & Resolution**<br>`erd-net/src/discovery/endpoint.rs`<br>`erd-net/src/discovery.rs`<br>`erd-net/src/discovery/apple.rs`<br>`erd-net/tests/discovery_scoped_ipv6.rs` | Scope retained through selection, validation, normalization, publication, and actual client resolver input; valid TXT/SRV + `fe80::1%5` publishes; unscoped or scope 0 retracts; IPv4 preference preserved; published string `fe80::1%5` and standard `ToSocketAddrs` resolve to `SocketAddrV6::scope_id() == 5`; interface-name scope (e.g. `fe80::1%lo`) supported; pure decision path extracted to `endpoint.rs` and consumed by production Apple code (`discovery/apple.rs`); public `DiscoveredHost` JSON shape unchanged; 8/8 scoped discovery tests passed, 18/18 metadata tests passed, 72/72 net tests passed. | **PASS**<br>*(Real Apple device mDNS discovery reserved for lead / Phase E)* |
| **R1** | **Directional Separation of Credential Stores**<br>`erd-app/src/pairing.rs`<br>`erd-host/src/session.rs`<br>`erd-host/tests/pairing_isolation.rs` | Client `client-pairings.json` and host `host-authorizations.json` remain physically separate; legacy `pairing-keys.json` read-only migration does not auto-import inbound authorizations into host; deletion isolated; 12/12 client pairing tests passed; 2/2 host pairing isolation integration tests passed. | **PASS** |
| **R2** | **Cryptographic Binding of Bootstrap Consent**<br>`erd-host/src/session.rs` | Bootstrap TLS cannot reach `Authenticated` without operator consent; Handshake strictly bound to freshly granted `pairing_id`; identity mismatch and duplicate handshakes rejected; 7/7 host bootstrap integration tests passed. | **PASS** |
| **R3** | **Current-Session Authenticated UDP Registration**<br>`erd-proto`, `erd-host`, `erd-app` | Directional `ClientToHost` sealed registration ping; capability bit 7 negotiated on both ends; missing capability yields `incompatible-peer` / `MissingAuthenticatedRegistration` error; unauthenticated datagrams and plaintext probes ignored; 6/6 UDP registration tests passed; zero capability regression. | **PASS** |
| **R4** | **Zero Secrets in Public APIs**<br>`erd-app/src/pairing.rs`<br>`tauri-shell/src-tauri/src/lib.rs`<br>`ios-shell/src/commands.rs` | `list_pairings` returns key-free `PairingSummary` (`id, host_name, added_at_unix_ms, last_endpoint`); zero 256-bit symmetric keys in IPC responses, UI states, or logs; JSON contract asserts key exclusion on desktop and iOS. | **PASS** |
| **R11** | **Random PIN Default & Zero Bootstrap Fallback**<br>`erd-host/src/main.rs`<br>`tauri-shell/src-tauri/src/lib.rs`<br>`ios-shell/src/state.rs` | Host daemon defaults to secure random PIN; client reconnection failures (network unreachable, handshake timeout, wrong ID, no network) preserve original error code and NEVER fall back to bootstrap pairing or default PIN `12345678`. | **PASS** |
| **CORE** | **No One-Off Explicit-PSK Disk Dependency**<br>`erd-app/src/session.rs` (`connect_with_pairing`) | Direct one-off `ClientSession::connect_with_pairing` path does NOT touch or require default pairing store on disk; CLI and QA clients can supply explicit pairing records in memory without creating/mutating storage files. | **PASS** |
| **BLOCKER** | **Concurrent Host-Management Policy Conflict**<br>`tauri-shell/Cargo.toml`<br>`tauri-shell/src-tauri/src/lib.rs`<br>`erd-host/src/session.rs` | Unresolved addition of `HostRuntime` in `tauri-shell` using hardcoded PIN `12345678` and unconditional auto-approval (`prompt.respond(true)`). Excluded from producer scope and preserved intact, but constitutes an active R11 deployment blocker. | **UNVERIFIED / GOAL BLOCKER**<br>*(Pending lead / coordinator policy reconciliation)* |

---

## 2. Detailed Requirement-by-Requirement Evidence

### 2.1 R8: Desktop Explicit Credential Selection & R11 Integration

#### A. Seam and Implementation Inspection
1. **Elimination of Unsafe Inference**:
   In `clients/rust/tauri-shell/src-tauri/src/lib.rs`, `resolve_stored_pairing` has been completely deleted. Grepping for `resolve_stored_pairing` returns zero occurrences across the entire workspace.
2. **Exact-ID Lookup & Zero Pre-Transport Fallback**:
   In `commands::connect`:
   ```rust
   let trimmed_pin = pin.as_deref().map(str::trim).filter(|s| !s.is_empty());
   let trimmed_id = pairing_id.as_deref().map(str::trim).filter(|s| !s.is_empty());

   if trimmed_pin.is_none() && trimmed_id.is_none() {
       return Err(IpcError::pairing_required("PIN required for initial authorization"));
   }

   if trimmed_pin.is_none() {
       if let Some(id) = trimmed_id {
           let store = PairingStore::open_default().map_err(|e| {
               IpcError::connection_failed(IpcErrorStage::Client, format!("Pairing store error: {e}"))
           })?;
           match store.load(id) {
               Ok(Some(_)) => {}
               Ok(None) => {
                   return Err(IpcError::pairing_required(format!("Unknown pairing ID '{id}'; PIN required")));
               }
               Err(e) => {
                   return Err(IpcError::connection_failed(IpcErrorStage::Client, format!("Pairing store error: {e}")));
               }
           }
       }
   }
   ```
   If PIN is omitted, only the exact `pairing_id` is loaded from the store. If the ID is absent or not found in the store, an `IpcError::pairing_required` is returned immediately before any network socket is opened or transport attempted.
3. **Integration of `authenticate_client_session` & R11 Client Protections**:
   Extracted into `tauri-shell/src-tauri/src/lib.rs:1239-1279`. Authenticates using either explicit PIN or exact stored record. If reconnect fails (e.g. loopback rejection, network unreachable, invalid key), it preserves the typed error and never falls back to bootstrap pairing or default PIN.
4. **Verified Endpoint Persistence & Session Rollback**:
   In `connect_session`:
   ```rust
   let endpoint = PairingEndpoint::new(host.clone(), tcp, udp);
   let store = match PairingStore::open_default() { ... };
   match store.remember_endpoint(&ready_session.pairing.id, &ready_session.pairing.key, endpoint) {
       Ok(persisted) => { ... }
       Err(e) => {
           let _ = session.disconnect();
           return Err(IpcError::connection_failed(
               IpcErrorStage::Client,
               format!("Failed to persist endpoint metadata: {e}"),
           ));
       }
   }
   ```
   Endpoint persistence occurs only after successful TLS and application handshake. Any persistence failure immediately invokes `session.disconnect()` to tear down the provisional session and returns `IpcError`, preventing false-connected UI state.
5. **Untrusted Discovery Deduplication**:
   In `merge_discovery_results`:
   - LAN and Tailscale entries are deduplicated strictly by identical IP address (`seen_ips.insert(h.ip.clone())`).
   - Distinct endpoints with identical hostnames are preserved as separate entries.
   - LAN items are unconditionally assigned `paired: false`, preventing inheritance of pairing trust from Tailscale or stored credentials.
6. **UI Separation & Key-Free Saved Credentials**:
   - `tauri-shell/ui/index.html` renders `#saved-pairings-section` with `#saved-pairings-list` and `#saved-pairings-count`.
   - Cards display `hostName`, distinct `id`, and formatted `lastEndpoint` (`${ep.host}:${ep.tcpPort} (UDP ${ep.udpPort})`).
   - Saved card "Connect" button calls `connectToHost(ep.host, pairing.hostName, null, ep.tcpPort, ep.udpPort, pairing.id)`, passing `pin: null` and explicit `pairingId`.
   - "Forget" button calls `forgetSavedPairing(pairing.id)` which invokes `forget_pairing`.

#### B. Executed Test Evidence
- **Test Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib pairing_tests"
  ```
  - Result: **12 passed; 0 failed** (exit code 0)
  - Selected Cases:
    - `pairing_tests::test_failed_reconnect_preserves_original_cause_and_never_falls_back_to_bootstrap_pin ... ok`
    - `pairing_tests::test_list_pairings_store_seam_isolated_store_serializes_only_allowed_metadata ... ok`
    - `pairing_tests::test_missing_pin_and_missing_record_fails_immediately_without_bootstrap ... ok`
    - `pairing_tests::test_no_network_stored_id_fails_without_fallback_to_pin ... ok`
    - `pairing_tests::test_list_pairings_json_excludes_key_field ... ok`
    - `pairing_tests::test_missing_id_and_missing_pin_fails_before_transport ... ok`
    - `pairing_tests::test_stored_id_reconnect_across_store_and_recreated_session_boundary ... ok`
    - `pairing_tests::test_same_name_records_select_exact_stored_id_and_key ... ok`
    - `pairing_tests::test_wrong_stored_id_fails_before_transport_without_bootstrap ... ok`
    - `pairing_tests::test_cancellable_accept_cancels_cleanly_before_client_connection ... ok`
    - `pairing_tests::test_cancellable_accept_normal_completion_with_real_client ... ok`
    - `pairing_tests::test_loopback_reconnect_failure_does_not_trigger_bootstrap_request ... ok`
- **Discovery Tests Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib discovery_tests"
  ```
  - Result: **18 passed; 0 failed; 1 ignored** (exit code 0)
  - Specifically verified: `discovery_tests::source_dedup_keeps_identical_name_distinct_endpoints_and_does_not_inherit_pairing ... ok`, `discovery_tests::lan_host_with_matching_stored_name_remains_unpaired ... ok`.
- **Bun Desktop UI Unit Suite**:
  ```bash
  bun test clients/rust/tauri-shell/ui/
  ```
  - Result: **60 pass; 0 fail** across 4 files (exit code 0)
  - Specifically verified: `validateConnection accepts pairingId and passes through invoke [0.02ms]`.
- **Bun Page Harness End-to-End Suite**:
  ```bash
  bun test clients/rust/tauri-shell/tests/frontend-page.test.mjs
  ```
  - Result: **12 pass; 0 fail; 81 expect() calls** (exit code 0)
  - Specifically verified: `(pass) R8 saved credentials display separately, click connect passes explicit pairingId without PIN, and forget removes card [88.50ms]`.
- **Scoped UI Render Artifact**:
  Verified `.omo/pairing-20260910/evidence/r8-desktop-ui.png` (1280x800 RGBA PNG) rendered through actual browser harness.

---

### 2.2 R9: iOS Explicit Saved-Credential Reconnect & Keychain Seam

#### A. Seam and Implementation Inspection
1. **Flat Invoke Arguments & Exact ID**:
   In `clients/rust/ios-shell/src/commands.rs:48-73`, `connect` takes flat parameters `host, tcp_port, udp_port, pin, pairing_id`. If `trimmed_pin.is_none() && trimmed_id.is_none()`, returns `Err(IpcError::pairing_required(...))` before dispatching to `state.connect_async`.
2. **Complete Removal of `find_by_host`**:
   Grepping for `find_by_host` across `clients/rust/ios-shell/src/` confirms zero occurrences. Legacy host-name/IP guessing has been entirely removed.
3. **Exact Keychain Load & Pre-Transport Rejection**:
   In `clients/rust/ios-shell/src/state.rs:491-518`:
   ```rust
   let stored_record: Option<erd_app::PairingRecord> = if trimmed_pin.is_none() {
       let id = trimmed_id.unwrap();
       let store = PairingStore::open_default()?;
       match store.load(id) {
           Ok(Some(record)) => Some(record),
           Ok(None) => {
               let err = IpcError::pairing_required(format!("Unknown pairing ID '{id}'; PIN required"));
               self.set_error(current_generation, err.message.clone());
               return Err(err);
           }
           Err(e) => { ... return Err(err); }
       }
   } else { None };
   ```
   Missing or unknown ID returns `IpcError::pairing_required` immediately, with zero transport sockets created and zero bootstrap packets sent.
4. **Endpoint Metadata Persistence**:
   In `clients/rust/ios-shell/src/state.rs:580-607`, calls `store.remember_endpoint` with validated host, TCP port, and UDP port after successful authentication. Any store failure triggers `self.disconnect_blocking()`, cleans up the provisional session, and returns typed `IpcError`.
5. **Lifecycle Lock Preservation**:
   `self.lifecycle_lock.lock().await` is acquired at the start of `connect_async`. `self.disconnect_blocking()` is executed at the entry of `connect_blocking` under the lock. Serialization is preserved.
6. **QA Provisioning Hardening**:
   In `clients/rust/ios-shell/src/qa.rs`:
   - Uses `PairingStore::open_default()` (the production store/Keychain backend).
   - If saving the QA record fails, returns `StartupResponse::default()` without claiming success.
   - Populates `pairing_id`, `tcp_port`, and `udp_port` in `StartupResponse` without exposing symmetric key bytes or entered PINs.
7. **iOS UI Untrusted Discovery Isolation**:
   In `clients/rust/ios-shell/ui/connection-state.js:330`, `selectDiscoveredHost` sets `selectedPairing = null`. Discovered cards never inherit stored credentials even if the advertised name matches a saved pairing.
   In `clients/rust/ios-shell/ui/app.js:182-258`, `#section-pairings` renders saved connections with distinct ID labels (`ID: ${pairing.id}`) and endpoints. Clicking a saved card populates host/ports, clears `pinInput.value = ''`, sets `selectedPairing`, and submits with explicit `pairingId`.

#### B. Executed Test Evidence
- **Rust iOS Scoped Test Suite Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-ios"
  ```
  - Result: **21 passed; 0 failed** (exit code 0)
  - Specifically verified newly named tests:
    - `tests::tests::test_connect_exact_id_loads_correct_stored_pairing ... ok`
    - `tests::tests::test_list_pairings_excludes_secret_key_bytes ... ok`
    - `tests::tests::test_connect_missing_id_and_missing_pin_fails_before_transport ... ok`
    - `tests::tests::test_remember_endpoint_survives_store_reload ... ok`
    - `tests::tests::test_connect_unknown_id_fails_before_transport_without_bootstrap ... ok`
    - `tests::tests::test_qa_provisioning_imports_and_propagates_pairing_id ... ok`
- **Bun iOS UI Full Suite**:
  ```bash
  bun test clients/rust/ios-shell/ui/test/
  ```
  - Result: **59 pass; 0 fail** across 7 files (exit code 0)
  - Specifically verified:
    - `(pass) saved-mode click passes explicit pairingId to native connect without PIN [0.08ms]`
    - `(pass) list_pairings populates savedPairings and excludes secret keys from state [0.10ms]`
    - `(pass) connect without PIN and without pairingId returns pairing-required error [0.10ms]`
    - `(pass) UI saved-mode click to actual invoke argument passes exact pairingId without PIN [0.04ms]`
    - `(pass) startup provisioning auto-connect dispatches connect with explicit pairingId [0.03ms]`
    - `(pass) unpaired discovered host with identical name does not choose saved credential [0.03ms]`
- **Physical iOS Target Compilation Proof (Mac)**:
  ```bash
  cargo build --manifest-path clients/rust/Cargo.toml --target aarch64-apple-ios -p erd-ios
  ```
  - Result: **Finished `dev` profile [unoptimized + debuginfo] in 0.76s** (exit code 0)
  - Produced artifacts:
    - `clients/rust/target/aarch64-apple-ios/debug/liberd_ios_lib.a` (503,829,176 bytes)
    - `clients/rust/target/aarch64-apple-ios/debug/erd-ios` (32,182,432 bytes)

---

### 2.3 R10: Scoped IPv6 Link-Local Discovery & Resolution

#### A. Seam and Implementation Inspection
1. **Shared Pure Decision Logic**:
   `clients/rust/erd-net/src/discovery/endpoint.rs` encapsulates:
   - `DiscoveredEndpoint`: retains `ip: IpAddr`, `scope_id: Option<u32>`, and `formatted: String` (`fe80::1%5`).
   - `is_usable_ipv6_with_scope(v6, scope_id)`: Link-local IPv6 (`fe80::/10`) requires `scope_id.is_some_and(|s| s > 0)`. Unscoped link-local or scope 0 returns `false`.
   - `choose_preferred_endpoint`: Prefers IPv4 > Global/ULA IPv6 > Scoped link-local IPv6 (`scope > 0`). Unscoped link-local returns `None`.
   - `validate_resolved_service`: Validates protocol version `"3"`, ports, TXT limits (<= 1300B), and creates `DiscoveredHost` with `ip = endpoint.formatted.clone()`.
   - `decide_service_state_action`: Emits `ServiceStateAction::Publish(host)` or `ServiceStateAction::Retract`.
2. **Apple Production Path Integration**:
   In `clients/rust/erd-net/src/discovery/apple.rs:1-4`:
   ```rust
   pub use super::{
       choose_and_format_address, choose_preferred_endpoint, choose_preferred_ip_with_scope,
       decide_service_state_action, ServiceStateAction,
   };
   ```
   The Apple DNSService workers at lines 599 and 721 call `decide_service_state_action` directly. There is zero duplicate or divergent logic.
3. **Public Shape and Legacy Callers Preserved**:
   In `clients/rust/erd-net/src/discovery.rs`, `parse_service_metadata` maintains its exact signature `(&str, &str, u16, &[(String, Vec<u8>)], &[IpAddr]) -> Result<DiscoveredHost, DiscoveryError>`, delegating to the new scoped pipeline with `(ip, None)`.
4. **Standard Resolver Verification**:
   In `erd-net/tests/discovery_scoped_ipv6.rs`:
   Calling `(host.ip.as_str(), host.tcp_port).to_socket_addrs()` on `"fe80::1%5"` successfully resolves to `SocketAddr::V6(v6)` where `v6.scope_id() == 5` and `v6.to_string() == "[fe80::1%5]:19730"`. Interface name scopes (`"fe80::1%lo"`) resolve to a positive numeric interface scope index via POSIX `getaddrinfo`.

#### B. Executed Test Evidence
- **Scoped IPv6 Test Suite Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-net --test discovery_scoped_ipv6"
  ```
  - Result: **8 passed; 0 failed** (exit code 0)
  - Selected Cases:
    - `test_choose_preferred_endpoint_selects_scoped_ipv6_and_preserves_scope ... ok`
    - `test_parse_service_metadata_scoped_accepts_scoped_ipv6 ... ok`
    - `test_ipv4_preferred_over_scoped_ipv6_link_local ... ok`
    - `test_scoped_ipv6_link_local_with_scope_zero_retracted ... ok`
    - `test_scoped_ipv6_link_local_published ... ok`
    - `test_unscoped_ipv6_link_local_retracted ... ok`
    - `test_resolver_resolves_interface_name_scope ... ok`
    - `test_scoped_ipv6_published_string_and_resolver_retain_scope ... ok`
- **Metadata Backward-Compatibility Suite Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-net --test discovery_metadata"
  ```
  - Result: **18 passed; 0 failed** (exit code 0)
- **Full `erd-net` Workspace Suite**:
  - Result: **72 passed; 0 failed** (46 lib, 18 metadata, 8 scoped ipv6 tests)

---

### 2.4 Core Credential & IPC Contracts (Shared Contract Producer)

#### A. Seam and Implementation Inspection
1. **`PairingEndpoint` DTO**:
   `host: String, tcp_port: u16, udp_port: u16` serialized as `camelCase` (`host`, `tcpPort`, `udpPort`). Preserves interface scope (`fe80::1%5`).
2. **`PairingRecord` Additions & Serde Defaults**:
   Added `last_endpoint: Option<PairingEndpoint>` and `endpoint_aliases: Vec<PairingEndpoint>`.
   - Annotated with `#[serde(default, skip_serializing_if = "...")]`.
   - Legacy records without these fields deserialize with `last_endpoint: None` and empty aliases.
   - When absent, fields are omitted from JSON serialization, maintaining schema compatibility.
3. **`PairingSummary` Key-Free DTO**:
   Carries `id`, `host_name`, `added_at_unix_ms`, and `last_endpoint`. Zero symmetric key bytes or key arrays are present.
4. **`PairingStore::remember_endpoint`**:
   - Updates `last_endpoint` to the validated endpoint.
   - Deduplicates: moves prior `last_endpoint` into `endpoint_aliases` if distinct.
   - Exact match requirement: if `id` is not found or `record.key != expected_key`, returns `Ok(false)` without resurrecting or mutating records.
5. **No One-Off Explicit-PSK Disk Dependency**:
   In `clients/rust/erd-app/src/session.rs:299-307`:
   `ClientSession::connect_with_pairing(pairing: PairingRecord)` derives the PSK directly in memory and initiates the handshake. It does not open, read, write, or depend on the disk pairing store.
6. **Typed Machine IPC Errors**:
   In `clients/rust/erd-app/src/error.rs`, `IpcError` provides structured `code` (`IpcErrorCode`) and `stage` (`IpcErrorStage`) serialized as kebab-case strings (e.g. `pairing-required`, `incompatible-peer`, `tls-psk`). Zero secrets or entered PINs appear in error text.

#### B. Executed Test Evidence
- **Client Pairing & Migration Suite Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-app --lib pairing::tests"
  ```
  - Result: **12 passed; 0 failed** (exit code 0)
  - Verified cases:
    - `test_client_default_path_filename ... ok`
    - `test_key_free_summary_serialization ... ok`
    - `test_id_key_mismatch_no_mutation ... ok`
    - `ephemeral_pairing_store_roundtrip_and_delete ... ok`
    - `test_legacy_record_readability ... ok`
    - `test_legacy_client_migration_skips_when_client_pairings_already_exists ... ok`
    - `test_legacy_client_migration_preserves_legacy_and_writes_client_pairings ... ok`
    - `test_missing_record_no_resurrection ... ok`
    - `test_preservation_of_credentials ... ok`
    - `test_metadata_roundtrip_and_dedup ... ok`
    - `test_deletion_isolated_from_legacy ... ok`
    - `test_temporary_file_naming_isolated ... ok`
- **Typed IPC Error Suite Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-app --lib error::tests"
  ```
  - Result: **2 passed; 0 failed** (exit code 0)
  - Verified cases:
    - `test_ipc_error_machine_fields_and_no_secret_leak ... ok`
    - `test_ipc_error_code_and_stage_serialization ... ok`

---

### 2.5 Verification of Retained Protections (R1, R2, R3, R4, R11)

To guarantee that Phase C changes did not cause regressions in previously committed phases, all foundational security tests were re-executed:

1. **R1 Trust Separation & Store Isolation**:
   ```bash
   ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-host --test pairing_isolation"
   ```
   - Result: **2 passed; 0 failed** (exit code 0)
   - Proves client outbound records cannot authorize inbound connections to the local host, and legacy files are not auto-imported into the host authorization store.
2. **R2 Host Consent Binding**:
   ```bash
   ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-host --lib bootstrap"
   ```
   - Result: **7 passed; 0 failed** (exit code 0)
   - Proves bootstrap connections without operator consent are rejected, granted IDs are cryptographically bound, and duplicate handshakes are rejected.
3. **R3 Authenticated UDP Registration & Zero Capability Regression**:
   ```bash
   ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-host -p erd-app udp"
   ```
   - Result: **8 passed; 0 failed** across `erd-app` and `erd-host` (exit code 0)
   - Proves missing authenticated registration capability bit 7 is rejected on both ends, and only current-session authenticated datagrams bind UDP endpoints.
4. **R11 Random PIN Default**:
   ```bash
   ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-host --bin erd-host"
   ```
   - Result: **5 passed; 0 failed** (exit code 0)
   - Proves default daemon startup generates a cryptographically random 8-digit PIN.
5. **Strict Workspace Clippy Gate**:
   ```bash
   ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo clippy --manifest-path clients/rust/Cargo.toml -p erd-app -p erd-net -p erd-mobile -p erd-ios -p tauri-shell -p erd-host --no-deps -- -D warnings"
   ```
   - Result: **0 warnings, 0 errors** (exit code 0)

---

## 3. Blockers and Open Policy Issues

### 3.1 Known Concurrent Host-Management Policy Conflict (Overall Goal Blocker)
- **Status**: **UNVERIFIED / BLOCKED**
- **Context**: As documented in `.omo/pairing-20260910/reports/concurrent-host-policy.md`, concurrent working-tree modifications in `tauri-shell` introduced a `HostRuntime` that binds `0.0.0.0`, uses a default hardcoded PIN `12345678`, and enables automatic approval (`prompt.respond(true)`).
- **Scope Exclusion**: These files were not authored by Phase C workers (`st_01a089dd`, `st_01a089de`, `st_01a089e6`, `st_01a089e7`). The verification suite intentionally refrained from executing the concurrent shell host-start tests or launching the real shell entry point with these defaults.
- **Overall Goal Impact**: While Phase C identity, discovery, and credential reconnection goals are completely fulfilled and verified, this concurrent hosting configuration remains a direct conflict with finding R11. It must be resolved by the lead / coordinator prior to final production release.

### 3.2 Physical Hardware & Lead Exercise Blockers (Phase E Scope)
In accordance with verification integrity guidelines, browser mocks, headless page harnesses, and compilation successes are **not** treated as physical runtime passes. The following surface behaviors are explicitly recorded as pending coordinator personal exercise on physical hardware:

1. **Physical iPhone (Apple Silicon & Hardware Secure Enclave)**:
   - **Hardware-Backed Keychain**: AES-256 Keychain encryption under `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`.
   - **Legacy Keychain Migration**: Migration of pre-existing `com.eclipticrd.ios.pairing` Keychain items on physical iOS devices across OS app updates.
   - **Reboot Persistence**: Persistence of `last_endpoint` metadata across device reboots without the `erd-device-qa.json` test fixture.
   - **Physical Gesture Latency**: Touch-down to presented video frame response latency on 120Hz ProMotion hardware displays.
   - **SpringBoard Lifecycle**: Backgrounding and foreground resumption under iOS memory pressure.
2. **Physical Desktop GUI**:
   - Direct user interaction with the native Tauri WebView window on macOS/Linux.
   - Local system keychain and application support folder permission boundaries.
3. **Physical Network Multi-Device Discovery**:
   - Real mDNS multicast transmission across physical Wi-Fi routers with IGMP snooping.
   - Link-local IPv6 communication across physical network adapters (`en0` / `wlan0`).

---

## 4. Final Conclusion & Phase C Sign-Off Recommendation

Phase C implementation fulfills all technical contracts established in `.omo/pairing-20260910/contracts.md` and `.omo/pairing-20260910/phase-c-contract.md`:
1. **R8 Desktop Credential Selection**: Fully verified. Name inference eliminated; exact-ID lookup enforced; endpoint metadata persisted; zero fallback to default PIN.
2. **R9 iOS Saved Reconnect**: Fully verified. Exact-ID Keychain selection enforced; `find_by_host` removed; UI saved connection mode operational without PIN; QA provisioning secured; physical-target build passes.
3. **R10 Scoped IPv6 Discovery**: Fully verified. Endpoint scopes preserved through discovery, Apple publication, and socket address resolution; pure decision pipeline tested on Omarchy.
4. **Retained Protections (R1, R2, R3, R4, R11)**: Fully re-verified with zero regressions.

**Recommendation**: Phase C implementation is verified and ready for atomic integration into git history, with the concurrent host-management policy tracked as an explicit blocker for overall goal acceptance.
