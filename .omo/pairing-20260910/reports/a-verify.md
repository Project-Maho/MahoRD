# Phase A Independent Verification Report: R1, R2, R4, R11 Evidence & Integration

- Task ID: `st_01a08976`
- Verifier / Worker: `hephaestus` (Senpi task child)
- Parent / Root Session: `01a0890a-69f1-7e5e-80cb-959dc3ddb61c`
- Review Plan: `docs/remote-connection-pairing-review-plan-20260910.md`
- Binding Contracts: `.omo/pairing-20260910/contracts.md` (Amendments 1, 2, 3, 4, 7, 8)
- Base Commit: `71f8b05f5a0e9d53ce0249beb46ca864b7a836f8`
- Committed Increments:
  - `0eafe10dc73189730b54bb2d4f3ef44ec5e16b62`: `fix(pairing): separate client credentials from host authorizations` (R1)
  - `9a364a955d5b65b96e5aeedc6c6b0366d8ca2a08`: `fix(auth): bind bootstrap handshakes to approved pairing identities` (R2, Host R11)
- Working Tree Increments:
  - `clients/rust/tauri-shell/src-tauri/src/lib.rs` (R4, Client R11 authentication helpers & public summary DTO)
  - `clients/rust/tauri-shell/src-tauri/src/pairing_tests.rs` (R4, Client R11 test suite with `CancellableServer` test harness)
- Target Remote: `indo@100.91.254.71` (`/home/indo/projects/erd-pairing-20260910`)
- Environment Flags: `PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig`, `LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:$LD_LIBRARY_PATH`
- Date: 2026-09-10

---

## 1. Requirement Verification Verdicts

| Finding ID | Title | Scope & Artifacts | Verdict |
|---|---|---|---|
| **R1** | Directional Separation of Trust / Default Credential Stores & Safe Client Migration | `erd-app::pairing` (`client-pairings.json`), `erd-host::session` (`host-authorizations.json`), `erd-host/tests/pairing_isolation.rs` | **PASS** |
| **R2** | Cryptographic Binding of Bootstrap Consent | `erd-host/src/session.rs` (`granted_pairing_id`, pre-auth gating, identity mismatch rejection, duplicate handshake rejection) | **PASS** |
| **R4** | Zero Secrets in Public APIs / Key-Free Pairing Summary IPC | `tauri-shell/src-tauri/src/lib.rs` (`PairingSummary`, `commands::list_pairings`), `tauri-shell/src-tauri/src/pairing_tests.rs` | **PASS** |
| **R11** | Secure Random Default Bootstrap PIN & Prohibition of Default-PIN Fallback | `erd-host/src/main.rs` (random PIN injection seam), `tauri-shell/src-tauri/src/lib.rs` (`authenticate_client_session`), `pairing_tests.rs` | **PASS** |

---

## 2. Audit of Coordinator Acceptance Findings & Test Harness Verification

This section audits the resolution of coordinator concerns (`a-ipc-follow-up.md`) and lead review directives regarding test synchronization, timeout handling, and panic/cleanup behavior in `clients/rust/tauri-shell/src-tauri/src/pairing_tests.rs`.

### 2.1 Elimination of Closed-Port Race
- **Concern:** In `test_failed_reconnect_preserves_original_cause_and_never_falls_back_to_bootstrap_pin`, earlier revisions bound an ephemeral port listener and immediately dropped it before attempting a client connection. This opened a race window where another process on the system could bind to that port before the test client connected.
- **Audited Implementation:** Replaced with an owned deterministic failing peer via `CancellableServer::spawn_failing_peer()`. A `TcpListener` is bound to `127.0.0.1:0` and retained throughout the test. When the client connects, the worker accepts the stream and immediately drops it:
  ```rust
  let accepted = rt.block_on(async {
      tokio::select! {
          res = tokio_listener.accept() => {
              match res {
                  Ok((stream, _peer_addr)) => {
                      let std_stream = stream.into_std().ok()?;
                      let _ = std_stream.set_nonblocking(false);
                      Some(std_stream)
                  }
                  Err(_) => None,
              }
          }
          _ = cancel_rx => None
      }
  });
  // Drop stream immediately to deterministically fail transport
  drop(stream);
  ```
- **Finding:** The port remains continuously allocated and owned by the test process. Connection reset/transport failure is deterministic, eliminating ephemeral port collision races.

### 2.2 Bounded Worker Synchronization
- **Concern:** In `test_loopback_reconnect_failure_does_not_trigger_bootstrap_request`, the test started a blocking accept without an explicit timeout deadline.
- **Audited Implementation:**
  1. Accept is wrapped in Tokio `select!` with a `tokio::sync::oneshot::channel::<()>` cancellation receiver (`cancel_rx`).
  2. TLS handshake execution uses `TlsPskServer::accept_stream_until(stream, Instant::now() + Duration::from_secs(3))` with an explicit 3-second deadline.
  3. Worker completion is synchronized with the test thread using `outcome_rx.recv_timeout(deadline)` (`Duration::from_secs(3)`).
- **Finding:** All accept, handshake, and synchronization operations have explicit timeout bounds.

### 2.3 Precise Audit of Cleanup and Drop Semantics (No Synchronous Join on Panic)
- **Direct Lead Review Requirement:** Accurately characterize the cleanup paths under normal exit vs panic/timeout unwinding, without claiming synchronous thread joining on panic.
- **Audited Behavior in `CancellableServer`:**
  1. **Normal Completion Path (`CancellableServer::join(deadline)`):**
     ```rust
     fn join(mut self, deadline: Duration) -> ServerOutcome {
         let outcome = self
             .outcome_rx
             .recv_timeout(deadline)
             .expect("server worker must complete within deadline");
         if let Some(handle) = self.handle.take() {
             handle
                 .join()
                 .expect("server worker thread must join cleanly without panicking");
         }
         outcome
     }
     ```
     In normal execution, the test thread waits up to `deadline` (3s) for the typed `ServerOutcome`. Once received, the worker has finished its execution; `handle.join()` runs synchronously and verifies that the worker thread exited without panicking.
  2. **Panic / Timeout Unwind Path (`CancellableServer::drop`):**
     ```rust
     impl Drop for CancellableServer {
         fn drop(&mut self) {
             // Deterministically cancel the accept if still in-flight
             if let Some(tx) = self.cancel_tx.take() {
                 let _ = tx.send(());
             }
             // Dropping self.handle detaches the JoinHandle.
             // It does NOT synchronously block or join on panic unwinding.
         }
     }
     ```
     - On panic unwinding or timeout failure before `join()` completes, `Drop` fires.
     - `Drop` signals cancellation via `cancel_tx.send(())` and **detaches** `self.handle`.
     - **There is NO synchronous join on panic.** This is a deliberate, correct design: if a worker thread were blocked or panicking, attempting an unbounded synchronous join in `Drop` would deadlock the unwinding test thread.
     - Because `self.handle` is detached, the worker terminates asynchronously:
       - If waiting in accept, the oneshot `cancel_rx` triggers immediate exit of `tokio::select!`.
       - If in the TLS handshake, `accept_stream_until` terminates when its 3-second deadline expires.
  3. **Test Infrastructure Fixture Checks (Not Product Regressions):**
     - Two lifecycle checks exist in `pairing_tests.rs`: `test_cancellable_accept_cancels_cleanly_before_client_connection` and `test_cancellable_accept_normal_completion_with_real_client`.
     - **Audit Assessment:** These two tests verify the behavior of the test fixture harness (`CancellableServer`) itself under cancellation and connection. They are positive verification of test infrastructure, not new product features.
     - **Exclusion of Producer Assertion-Flip Claim:** The producer's previous claim of "RED 1" by mutating an assertion (`assert_eq!(outcome, ServerOutcome::AcceptedBootstrap)` on cancelled accept) is explicitly **rejected and excluded** as meaningful regression evidence. Mutating a test expectation does not demonstrate product regression. Only true product mutations and original production RED logs are admitted as regression evidence.

### 2.4 Elimination of Pre-Completion Flag Check
- **Concern:** The test previously inspected an `AtomicBool` (`bootstrap_attempted`) before joining the worker thread, creating scheduling-dependent race conditions.
- **Audited Implementation:** The atomic boolean has been deleted. The server worker transmits a typed `ServerOutcome` enum:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  enum ServerOutcome {
      Cancelled,
      DroppedStream,
      AcceptedBootstrap,
      RejectedTls(String),
  }
  ```
  The outcome is received in `server.join(...)` after the worker thread finishes. The test asserts that `outcome` is `ServerOutcome::RejectedTls` strictly after thread settlement.

---

## 3. Detailed Per-Requirement Verification Evidence

### 3.1 R1: Directional Separation of Trust & Safe Legacy Client Migration
- **Invariant:** Host inbound authorizations and client outbound credentials must never share a storage file, directory path, or identity namespace. Inbound host must never auto-import legacy mixed `pairing-keys.json`. Client must safely migrate legacy records into `client-pairings.json` as a copy, preserving `pairing-keys.json` read-only.
- **Production Implementation:**
  - `clients/rust/erd-app/src/pairing.rs`: `default_path()` resolves to `client-pairings.json`. `load_all()` checks for absence of `client-pairings.json` and presence of `pairing-keys.json`, copying legacy records without deleting or modifying `pairing-keys.json`.
  - `clients/rust/erd-host/src/session.rs`: `PairingStore::default_path()` resolves to `host-authorizations.json`. Strictly never imports from `pairing-keys.json`.
- **Pre-Fix RED Evidence:**
  - Remote run recorded in `reports/a-store.md`: Exit code 101. `test_outbound_client_pairing_rejected_as_host_authorization` panicked at `Host authorization store must be empty and must not contain client outbound keys (trust mixing detected...)`.
- **Independent Remote GREEN Execution Receipts:**
  - **Host Inbound Isolation (Single Filter):**
    ```bash
    ssh -o BatchMode=yes indo@100.91.254.71 \
      "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
       export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
       cd /home/indo/projects/erd-pairing-20260910 && \
       cargo test --manifest-path clients/rust/Cargo.toml -p erd-host --test pairing_isolation"
    ```
    - **Exit Code:** `0`
    - **Raw Output:**
      ```text
      running 2 tests
      test test_legacy_pairing_keys_not_auto_imported_by_host_and_migrated_by_client ... ok
      test test_outbound_client_pairing_rejected_as_host_authorization ... ok
      test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s
      ```
  - **Client Outbound Migration & Deletion Isolation (Single Filter):**
    ```bash
    ssh -o BatchMode=yes indo@100.91.254.71 \
      "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
       export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
       cd /home/indo/projects/erd-pairing-20260910 && \
       cargo test --manifest-path clients/rust/Cargo.toml -p erd-app --lib pairing::tests"
    ```
    - **Exit Code:** `0`
    - **Raw Output:**
      ```text
      running 6 tests
      test pairing::tests::test_client_default_path_filename ... ok
      test pairing::tests::ephemeral_pairing_store_roundtrip_and_delete ... ok
      test pairing::tests::test_legacy_client_migration_skips_when_client_pairings_already_exists ... ok
      test pairing::tests::test_legacy_client_migration_preserves_legacy_and_writes_client_pairings ... ok
      test pairing::tests::test_temporary_file_naming_isolated ... ok
      test pairing::tests::test_deletion_isolated_from_legacy ... ok
      test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 119 filtered out; finished in 0.00s
      ```

### 3.2 R2: Cryptographic Binding of Bootstrap Consent
- **Invariant:** Bootstrap TLS (`erd-b1`) is restricted to the pairing handshake. It cannot transition to `Authenticated` without host operator approval. Subsequent application handshake must be strictly bound to the granted `pairing_id`. Duplicate handshakes must be rejected. Unconsented input/control packets must be rejected.
- **Production Implementation:**
  - `clients/rust/erd-host/src/session.rs`: Tracks `granted_pairing_id: Option<String>`. Rejects unconsented application handshake with `SessionError::PreAuth`, rejects mismatched pairing ID with `SessionError::IdentityMismatch`, and rejects subsequent handshakes with `SessionError::AlreadyAuthenticated`. Capture pipelines and input injectors remain inactive until authentication completes.
- **Pre-Fix RED Evidence:**
  - Recorded in `.omo/pairing-20260910/evidence/st_01a0892a-red-regression.log`:
    - `test_bootstrap_without_consent_handshake_rejected_with_no_capture_or_input`: panicked at `server must reject unconsented bootstrap handshake with PreAuth, got: Ok(())`.
    - `test_bootstrap_consent_b_cannot_use_a`: panicked at `server must reject handshake with mismatched pairing ID with IdentityMismatch, got: Ok(())`.
    - `test_bootstrap_authenticated_session_rejects_duplicate_handshake`: panicked at `server must reject duplicate handshake with AlreadyAuthenticated, got: Ok(())`.
- **Independent Remote GREEN Execution Receipt (Single Filter):**
  ```bash
  ssh -o BatchMode=yes indo@100.91.254.71 \
    "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
     export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
     cd /home/indo/projects/erd-pairing-20260910 && \
     cargo test --manifest-path clients/rust/Cargo.toml -p erd-host --lib bootstrap"
  ```
  - **Exit Code:** `0`
  - **Raw Output:**
    ```text
    running 7 tests
    test session::tests::lockout_disables_bootstrap_after_five_failures ... ok
    test session::tests::bootstrap_kdf_runs_once_per_unchanged_pairing_window ... ok
    test session::tests::cached_bootstrap_obeys_lockout_expiry_and_pairing_revocation ... ok
    test session::tests::test_bootstrap_authenticated_session_rejects_duplicate_handshake ... ok
    test session::tests::test_bootstrap_without_consent_handshake_rejected_with_no_capture_or_input ... ok
    test session::tests::test_bootstrap_consent_b_cannot_use_a ... ok
    test session::tests::test_bootstrap_normal_b_and_paired_a_work ... ok
    test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 83 filtered out; finished in 0.49s
    ```

### 3.3 R4: Zero Secrets in Public APIs / Key-Free Pairing Summary IPC
- **Invariant:** IPC commands, Tauri invoke handlers, and UI logging must never expose long-term 256-bit symmetric keys. UI consumes only public summary metadata (`PairingSummary`).
- **Production Implementation:**
  - `clients/rust/tauri-shell/src-tauri/src/lib.rs`: Defined `pub struct PairingSummary` containing only `id`, `host_name` (`hostName` in JSON), `added_at_unix_ms` (`addedAtUnixMs`), and optional `last_endpoint`. Symmetric keys are completely omitted. `commands::list_pairings` returns `Result<Vec<PairingSummary>, String>`. `list_pairings_internal(&PairingStore)` provides an isolated command seam.
- **Original RED Evidence:**
  - Prior to introducing `PairingSummary`, `commands::list_pairings` returned `Result<Vec<erd_app::PairingRecord>, String>`. `PairingRecord` serializes the raw 32-byte secret symmetric key field as `"key":"..."`. When `test_list_pairings_json_excludes_key_field` was first asserted against the unpatched IPC DTO, it failed because `"key"` and raw base64 key material were present in the serialized JSON.
- **Verification of Serialized Public DTO (Zero Key Leaks):**
  - Seam execution produces serialized JSON:
    `[{"id":"SEAM-UUID-999","hostName":"desktop-host-target","addedAtUnixMs":1725999999000}]`
  - Key audit: Exactly 0 occurrences of `"key"`, 0 occurrences of base64 key material.
- **Independent Remote GREEN Execution Receipt (Single Filter):**
  ```bash
  ssh -o BatchMode=yes indo@100.91.254.71 \
    "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
     export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
     cd /home/indo/projects/erd-pairing-20260910 && \
     cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib test_list_pairings"
  ```
  - **Exit Code:** `0`
  - **Raw Output:**
    ```text
    running 2 tests
    test pairing_tests::test_list_pairings_json_excludes_key_field ... ok
    test pairing_tests::test_list_pairings_store_seam_isolated_store_serializes_only_allowed_metadata ... ok
    test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 57 filtered out; finished in 0.00s
    ```

### 3.4 R11: Secure Random Default Bootstrap PIN & Prohibition of Default-PIN Fallback
- **Invariant:**
  - Host: Default bootstrap PIN must be a cryptographically secure random 8-digit string (`random_pin()`), not `"12345678"`. Explicit `--bootstrap-pin` bypasses generation for automated testing.
  - Client: If no PIN is provided and no matching record exists in the store, authentication must immediately return error `No PIN provided and no previous pairing found in store`. If stored reconnect fails, preserve the original error cause (`Reconnect error: {e}`) and **NEVER** attempt bootstrap pairing with `"12345678"`.
- **Production Implementation:**
  - `clients/rust/erd-host/src/main.rs`: `select_pin` accepts an injected generator closure `F: FnMut() -> String`. Default branch calls `generator()`. Main calls `select_pin(cli.bootstrap_pin, cli.pin.as_deref(), random_pin)?`.
  - `clients/rust/tauri-shell/src-tauri/src/lib.rs`: `authenticate_client_session` checks for PIN or stored record. On reconnect error, returns `Reconnect error: {e}` without silent fallback to bootstrap pairing.
- **Meaningful Product Mutation Proofs (Admitted RED Evidence):**
  - **Host Default PIN RED:** In unpatched `erd-host/src/main.rs`, the default branch was hardcoded to `"12345678"`. Injected generator test failed with left: 0, right: 1 (recorded in `st_01a0892a-red-regression.log`).
  - **Client Reconnect Fallback Mutation RED:** Adversarially mutating `authenticate_client_session` to fallback to `session.pair_with_pin("12345678")` on reconnect failure caused both `test_failed_reconnect_preserves_original_cause_and_never_falls_back_to_bootstrap_pin` and `test_loopback_reconnect_failure_does_not_trigger_bootstrap_request` to fail with exit code 101 (`panicked: Error must start with 'Reconnect error:', got: Pairing error: ...`).
  - **Client Missing PIN/Record Mutation RED:** Adversarially mutating the missing PIN/record path to attempt `.pair_with_pin("12345678")` caused `test_missing_pin_and_missing_record_fails_immediately_without_bootstrap` to fail with exit code 101 (`panicked: assertion failed: left == right: "Pairing error: ..." vs "No PIN provided and no previous pairing found in store"`).

#### Correction of Cargo Test Invocations
The earlier report included an invalid command:
`cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib test_missing_pin test_failed_reconnect test_loopback_reconnect`
Cargo test only accepts a single test filter as a positional argument. The second positional argument is rejected:
`error: unexpected argument 'test_failed_reconnect' found` (exit code 1).

To provide unambiguous, verified evidence, each single-filter test invocation was independently run on the remote target, alongside the module-scoped filter `pairing_tests`.

#### Independent Remote GREEN Execution Receipts:
- **Host Random PIN Policy (Single Filter `pin`):**
  ```bash
  ssh -o BatchMode=yes indo@100.91.254.71 \
    "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
     export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
     cd /home/indo/projects/erd-pairing-20260910 && \
     cargo test --manifest-path clients/rust/Cargo.toml -p erd-host --bin erd-host pin"
  ```
  - **Exit Code:** `0`
  - **Raw Output:**
    ```text
    running 5 tests
    test tests::test_pin_default_selects_injected_generator_branch ... ok
    test tests::test_pin_explicit_bootstrap_pin_bypasses_generator ... ok
    test tests::test_pin_generate_flag_selects_injected_generator_branch ... ok
    test tests::test_pin_validation_accepts_8_digits_and_rejects_invalid ... ok
    test tests::test_random_pin_retained_and_format_valid ... ok
    test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
    ```

- **Client Single-Filter Invocations:**
  1. Filter: `test_missing_pin`:
     ```bash
     ssh -o BatchMode=yes indo@100.91.254.71 \
       "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
        export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
        cd /home/indo/projects/erd-pairing-20260910 && \
        cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib test_missing_pin"
     ```
     - **Exit Code:** `0`
     - **Raw Output:**
       ```text
       running 1 test
       test pairing_tests::test_missing_pin_and_missing_record_fails_immediately_without_bootstrap ... ok
       test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 58 filtered out; finished in 0.00s
       ```

  2. Filter: `test_failed_reconnect`:
     ```bash
     ssh -o BatchMode=yes indo@100.91.254.71 \
       "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
        export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
        cd /home/indo/projects/erd-pairing-20260910 && \
        cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib test_failed_reconnect"
     ```
     - **Exit Code:** `0`
     - **Raw Output:**
       ```text
       running 1 test
       test pairing_tests::test_failed_reconnect_preserves_original_cause_and_never_falls_back_to_bootstrap_pin ... ok
       test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 58 filtered out; finished in 0.01s
       ```

  3. Filter: `test_loopback_reconnect`:
     ```bash
     ssh -o BatchMode=yes indo@100.91.254.71 \
       "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
        export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
        cd /home/indo/projects/erd-pairing-20260910 && \
        cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib test_loopback_reconnect"
     ```
     - **Exit Code:** `0`
     - **Raw Output:**
       ```text
       running 1 test
       test pairing_tests::test_loopback_reconnect_failure_does_not_trigger_bootstrap_request ... ok
       test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 58 filtered out; finished in 0.08s
       ```

  4. Filter: `test_cancellable_accept`:
     ```bash
     ssh -o BatchMode=yes indo@100.91.254.71 \
       "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
        export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
        cd /home/indo/projects/erd-pairing-20260910 && \
        cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib test_cancellable_accept"
     ```
     - **Exit Code:** `0`
     - **Raw Output:**
       ```text
       running 2 tests
       test pairing_tests::test_cancellable_accept_cancels_cleanly_before_client_connection ... ok
       test pairing_tests::test_cancellable_accept_normal_completion_with_real_client ... ok
       test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 57 filtered out; finished in 0.08s
       ```

- **Client Module-Scoped Filter (`pairing_tests`):**
  ```bash
  ssh -o BatchMode=yes indo@100.91.254.71 \
    "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
     export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
     cd /home/indo/projects/erd-pairing-20260910 && \
     cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib pairing_tests"
  ```
  - **Exit Code:** `0`
  - **Raw Output:**
    ```text
    running 7 tests
    test pairing_tests::test_failed_reconnect_preserves_original_cause_and_never_falls_back_to_bootstrap_pin ... ok
    test pairing_tests::test_list_pairings_store_seam_isolated_store_serializes_only_allowed_metadata ... ok
    test pairing_tests::test_list_pairings_json_excludes_key_field ... ok
    test pairing_tests::test_missing_pin_and_missing_record_fails_immediately_without_bootstrap ... ok
    test pairing_tests::test_cancellable_accept_cancels_cleanly_before_client_connection ... ok
    test pairing_tests::test_loopback_reconnect_failure_does_not_trigger_bootstrap_request ... ok
    test pairing_tests::test_cancellable_accept_normal_completion_with_real_client ... ok
    test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 52 filtered out; finished in 0.08s
    ```

---

## 4. Master Remote Verification & Full Workspace Evidence

### 4.1 Master Filtered Pairing Command (Mandated by Task)
```bash
ssh -o BatchMode=yes indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo test --manifest-path clients/rust/Cargo.toml -p erd-host -p erd-app -p tauri-shell pairing"
```
- **Exit Code:** `0`
- **Output Breakdown:**
  - `erd-app` lib unittests: 6 passed (`pairing::tests::*`).
  - `erd-app` integration (`tests/session_mock.rs`): 2 passed (`connect_with_pairing_direct_round_trip`, `mock_server_pairing_handshake_and_input_round_trip`).
  - `erd-host` lib unittests: 3 passed (`pairing_store_mirrors_swift_json_and_is_mode_0600`, `bootstrap_kdf_runs_once_per_unchanged_pairing_window`, `cached_bootstrap_obeys_lockout_expiry_and_pairing_revocation`).
  - `erd-host` integration (`tests/pairing_isolation.rs`): 2 passed (`test_legacy_pairing_keys_not_auto_imported_by_host_and_migrated_by_client`, `test_outbound_client_pairing_rejected_as_host_authorization`).
  - `tauri-shell` lib unittests: 11 passed (7 `pairing_tests`, 3 `discovery_tests`, 1 `desktop_integration_tests`).
  - **Total Non-Zero Matching Tests: 24 passed; 0 failed; 0 ignored.**

### 4.2 Full Workspace Regression Recount (342 Passed, 3 Doc-tests, 1 Ignored)
Due to the addition of the two harness fixture checks (`test_cancellable_accept_cancels_cleanly_before_client_connection` and `test_cancellable_accept_normal_completion_with_real_client`), the prior count of 340 has been updated to 342.

```bash
ssh -o BatchMode=yes indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo test --manifest-path clients/rust/Cargo.toml -p erd-host -p erd-app -p tauri-shell"
```
- **Exit Code:** `0`
- **Exact Suite Breakdown:**
  - `erd-app` lib unittests: 125 passed; 0 failed
  - `erd-app` bin `erd_client`: 16 passed; 0 failed
  - `erd-app` integration tests:
    - `agent_control_e2e`: 3 passed
    - `cli_mcp_contract`: 5 passed
    - `cli_receiver_telemetry`: 3 passed
    - `client_copy_cost`: 3 passed
    - `core_semantics`: 6 passed
    - `media_reassembly`: 8 passed
    - `receiver_telemetry`: 5 passed
    - `session_mock`: 7 passed
    *(Subtotal `erd-app`: 181 passed)*
  - `erd-host` lib unittests: 90 passed; 0 failed
  - `erd-host` bin `erd-host`: 5 passed; 0 failed
  - `erd-host` integration tests:
    - `linux_audio_selection`: 6 passed
    - `pairing_isolation`: 2 passed
    *(Subtotal `erd-host`: 103 passed)*
  - `tauri-shell` lib unittests: 58 passed; 0 failed; 1 ignored (`discovery_tests::observe_installed_tailscale`)
    *(Subtotal `tauri-shell`: 58 passed)*
  - Doc-tests: 3 passed (`erd-host/src/session.rs`)
  - **Workspace Grand Total: 342 passed (181 + 103 + 58), 3 doc-tests passed, 0 failed, 1 ignored.**

### 4.3 Compiler & Lint Diagnostics
```bash
ssh -o BatchMode=yes indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo check --manifest-path clients/rust/Cargo.toml -p erd-host -p erd-app -p tauri-shell"
```
- **Exit Code:** `0` (Clean build: 0 errors, 0 warnings).

```bash
ssh -o BatchMode=yes indo@100.91.254.71 \
  "export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig; \
   export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH; \
   cd /home/indo/projects/erd-pairing-20260910 && \
   cargo clippy --manifest-path clients/rust/Cargo.toml -p erd-host -p tauri-shell --no-deps -- -D warnings"
```
- **Exit Code:** `0` (0 errors, 0 warnings).

---

## 5. Working Tree & Boundary Audit

1. **Pre-existing Dirty State Integrity:**
   - Evaluated working tree diff against `.omo/pairing-20260910/baseline.patch`.
   - Pre-existing modifications in `inject_macos.rs`, `discovery_tests.rs`, `tauri-shell/src-tauri/ui/index.html`, `tests/page-harness.mjs`, `tauri-shell/ui/index.html`, and `tauri-shell/ui/performance.test.mjs` remain byte-for-byte identical to the baseline patch.
   - All unrelated WebGL, discovery, and input adjustments are completely intact.
2. **Read-Only Verification Compliance:**
   - The verifier remained strictly read-only on all production source files and test sources across the workspace. Only this verification report was authored.
3. **Loopback Resource Cleanup:**
   - Test socket listeners and temporary directories are bounded by RAII guards (`TempStoreGuard`, `CancellableServer`).
   - `CancellableServer::drop` non-blockingly cancels in-flight accepts and detaches the thread handle, avoiding deadlock on unwinding while allowing background tasks to terminate within strict 3-second bounds.
   - No orphan processes, hanging sockets, or persistent temporary files remain on the remote testing environment.

---

## 6. Conclusion & Recommendation

All Phase A criteria (**R1**, **R2**, **R4**, **R11**) have been independently verified against production sources, remote test executions, meaningful product mutation proofs, and architectural boundary invariants:
1. **R1 (PASS):** Directional trust separation enforced; legacy stores protected against host auto-import; client migration verified safe and read-only on legacy backup.
2. **R2 (PASS):** Bootstrap consent strictly bound to operator-granted identity; unconsented and mismatched handshakes rejected without worker initiation; duplicate handshakes rejected.
3. **R4 (PASS):** Zero 256-bit symmetric keys exposed in public IPC commands or DTOs; serialized JSON verified key-free.
4. **R11 (PASS):** Host defaults to cryptographically secure random 8-digit PIN; client never falls back to bootstrap PIN `"12345678"` upon reconnect failure or missing credentials, preserving original error cause.
5. **Test Harness & Lead Review Compliance (PASS):**
   - Closed-port race eliminated via owned failing peer.
   - Worker synchronization bounded by 3-second deadlines.
   - `Drop` cleanup verified: signals cancellation and detaches without claiming synchronous thread joins on panic.
   - Assertion-flip proof excluded; meaningful product mutation proofs retained.
   - Test count accurately updated to 342 passed (58 in tauri-shell).
   - Single-filter Cargo test invocations verified and documented with raw receipts.

Phase A is fully verified and ready for final lead acceptance review.
