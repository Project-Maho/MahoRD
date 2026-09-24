# Manual QA Matrix: R4 Public Key-Free Pairing Summary IPC & R11 Client Default-PIN Fallback Removal

- Goal ID: `a-ipc`
- Task ID: `st_01a0892b`
- Date: 2026-09-10
- Execution Machine: `indo@100.91.254.71` (`/home/indo/projects/erd-pairing-20260910`)
- Environment: Linux 7.1.9-arch1-2 x86_64 GNU/Linux, `PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig`, `LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:$LD_LIBRARY_PATH`

---

## 1. Surface Evidence

| Scenario ID | Criterion Reference | Surface | Exact Invocation | Verdict | Artifact Refs |
|---|---|---|---|---|---|
| `SCN-R4-IPC-JSON` | Finding R4 / Contract §3.1, §10.1 (Test A.5) | Rust IPC Command Seam & Tauri Invocation (`commands::list_pairings()`) | `cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib pairing_tests::test_list_pairings_json_excludes_key_field` | **PASS** | `art-green-output`, `art-seam-json` |
| `SCN-R4-SEAM-METADATA` | Finding R4 / Contract §3.1, §3.2 | Isolated Store Seam (`commands::list_pairings_internal(&store)`) | `cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib test_list_pairings_store_seam_isolated_store_serializes_only_allowed_metadata -- --nocapture` | **PASS** | `art-seam-json`, `art-green-output` |
| `SCN-R11-MISSING-PIN-RECORD` | Finding R11 / Contract §1, §3.3 | Client Session Auth Seam (`commands::authenticate_client_session`) | `cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib pairing_tests::test_missing_pin_and_missing_record_fails_immediately_without_bootstrap` | **PASS** | `art-green-output` |
| `SCN-R11-RECONNECT-FAIL-NO-PIN` | Finding R11 / Contract §1, §3.3 | Client Session Auth Seam with Closed Loopback Port | `cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib pairing_tests::test_failed_reconnect_preserves_original_cause_and_never_falls_back_to_bootstrap_pin` | **PASS** | `art-green-output` |
| `SCN-R11-LOOPBACK-TLS-REJECT` | Finding R11 / Contract §1, §3.3, §10.1 | Real Loopback `TlsPskServer` (Bootstrap-Only Listener) | `cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib pairing_tests::test_loopback_reconnect_failure_does_not_trigger_bootstrap_request` | **PASS** | `art-green-output` |
| `SCN-FULL-SUITE-NON-REGRESSION` | Overall Workspace Invariants | Full Tauri-Shell Test Suite | `cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell --lib` | **PASS** | `art-full-suite-output` |

---

## 2. Adversarial Cases

| Scenario ID | Criterion Reference | Adversarial Class | Expected Behavior | Verdict | Artifact Refs |
|---|---|---|---|---|---|
| `ADV-R4-SECRET-LEAK` | Finding R4 | Secret Key Exfiltration in IPC Response | `commands::list_pairings()` output and JSON serialization must not contain `"key"` field or base64 key material under any circumstances. | **PASS** | `art-red-output`, `art-seam-json` |
| `ADV-R11-FALLBACK-ON-FAIL` | Finding R11 | Unauthorized Bootstrap Probe on Reconnect Failure | When reconnect fails against peer expecting paired TLS, client must NOT send fallback bootstrap TLS packet with `"12345678"`. | **PASS** | `art-mutation1-output`, `art-green-output` |
| `ADV-R11-FALLBACK-ON-MISSING` | Finding R11 | Auto-Pairing Probe with Default PIN on Unpaired Target | When user did not provide PIN and store has no record, client must immediately reject and not attempt `pair_with_pin("12345678")`. | **PASS** | `art-mutation2-output`, `art-green-output` |
| `ADV-R11-ERROR-ERASURE` | Finding R11 / Contract §6, Lead Amendment #8 | Error Classification & Cause Erasing | Native reconnect failures must preserve underlying cause (`I/O failed`, `Connection refused`, `TLS transport error`), and not mask as generic `"pairing-required"`. | **PASS** | `art-green-output` |
| `ADV-CONCURRENCY-XDG-RACE` | Test Discipline | Concurrency / Multi-Threaded Environment Variable Contention | Parallel test runners sharing process memory must not overwrite or clear each other's temporary isolated store paths. Guarded via RAII process lock. | **PASS** | `art-green-output` |
| `ADV-R8-PRESERVATION` | Lead Amendment #2, Scope Instruction | Credential-Selection Regression in Phase A | Credential-selection matching logic in `resolve_stored_pairing` (case-insensitivity, prefix match, testbed IP overrides) must remain intact until Phase C. | **PASS** | `art-green-output`, `art-full-suite-output` |

---

## 3. Artifact References

| Artifact ID | Kind | Description | Relative Path |
|---|---|---|---|
| `art-red-output` | Terminal Transcript | Regression failure receipt proving `commands::list_pairings` leaked `"key"` prior to R4 implementation | `.omo/pairing-20260910/evidence/st_01a0892b/red-test-output.txt` |
| `art-green-output` | Terminal Transcript | Full green execution output for all 9 pairing and discovery unit/integration tests | `.omo/pairing-20260910/evidence/st_01a0892b/green-test-output.txt` |
| `art-mutation1-output` | Terminal Transcript | Mutation proof output showing test failure when fallback to default PIN `"12345678"` was injected on reconnect failure | `.omo/pairing-20260910/evidence/st_01a0892b/mutation1-reconnect-fallback-output.txt` |
| `art-mutation2-output` | Terminal Transcript | Mutation proof output showing test failure when fallback to default PIN `"12345678"` was injected on missing PIN/record | `.omo/pairing-20260910/evidence/st_01a0892b/mutation2-missing-pin-fallback-output.txt` |
| `art-seam-json` | JSON Data | Actual serialized JSON produced by isolated store seam demonstrating public metadata only (`id`, `hostName`, `addedAtUnixMs`) and zero secrets | `.omo/pairing-20260910/evidence/st_01a0892b/seam-json-serialization.json` |
| `art-full-suite-output` | Terminal Transcript | Comprehensive workspace test run of all 57 unit tests in `tauri-shell` demonstrating zero regressions | `.omo/pairing-20260910/evidence/st_01a0892b/full-tauri-shell-test-output.txt` |
