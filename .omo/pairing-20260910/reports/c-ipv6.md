# Phase C Task Implementation Report: R10 Scoped IPv6 Link-Local Discovery

- Task ID: `st_01a089de`
- Goal: Implement R10 scoped IPv6 discovery end to end.
- Worker: `hephaestus`
- Session: `01a0890a-69f1-7e5e-80cb-959dc3ddb61c` (Depth: 1)
- Date: 2026-09-10
- Base Commits: Accepted A/B increments (`0eafe10`, `9a364a9`, `fa476b1`, `8c4570e`)
- Target Remote: `indo@100.91.254.71` (`/home/indo/projects/erd-pairing-20260910`)
- Environment: `PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:$LD_LIBRARY_PATH`
- Deliverable: Tested Code + Scoped Verifications + Report (`.omo/pairing-20260910/reports/c-ipv6.md`)

---

## 1. Executive Summary

Implemented, hardened, and verified finding **R10** (Scoped IPv6 link-local discovery and endpoint preservation) across `clients/rust/erd-net/src/discovery/endpoint.rs`, `clients/rust/erd-net/src/discovery.rs`, `clients/rust/erd-net/src/discovery/apple.rs`, and the dedicated regression suite `clients/rust/erd-net/tests/discovery_scoped_ipv6.rs`.

### 1.1 Root Cause of R10 Scope Loss / Rejection
Prior to this change:
1. In `erd-net/src/discovery/apple.rs`, `choose_and_format_address` extracted `selected_ip` as an unscoped `IpAddr` (`IpAddr::V6(v6)`) alongside a formatted string `fe80::1%<scope>`.
2. `decide_service_state_action` passed `&[selected_ip]` to `parse_service_metadata`. Standard library `IpAddr` does not retain interface scope IDs.
3. In `erd-net/src/discovery.rs`, `parse_service_metadata` applied `is_usable_ipv6(&selected_ip)`.
4. `is_usable_ipv6` checked `!is_unscoped_ipv6_link_local(v6)`. Because `fe80::1` is within `fe80::/10` (`(segments[0] & 0xffc0) == 0xfe80`), `is_unscoped_ipv6_link_local` returned `true`, causing `is_usable_ipv6` to return `false`.
5. Consequently, `parse_service_metadata` failed with `DiscoveryError::InvalidPayload("no usable unicast address found")`, and `decide_service_state_action` unconditionally returned `ServiceStateAction::Retract`.
6. Valid mDNS services with valid TXT/SRV records and scoped link-local addresses (e.g., `(fe80::1, Some(5))`) were completely dropped and never published to client discovery snapshots.

### 1.2 Architectural Resolution: Shared Pure Decision Path
In strict compliance with the coordinator brief ("move ONLY necessary pure decision logic into a shared path used by production Apple code and test that path on Omarchy, not a test copy/proxy"):
1. **New Pure Module `erd-net/src/discovery/endpoint.rs`**:
   - `DiscoveredEndpoint`: Encapsulates `ip: IpAddr`, `scope_id: Option<u32>`, and `formatted: String`.
   - `is_usable_ipv6_with_scope(v6: &Ipv6Addr, scope_id: Option<u32>) -> bool`:
     - Unspecified (`::`), loopback (`::1`), and multicast (`ff00::/8`) addresses are rejected (`false`).
     - Link-local addresses (`fe80::/10`) are accepted **if and only if** `scope_id.is_some_and(|s| s > 0)`.
     - Unscoped link-local addresses (`scope_id == None` or `Some(0)`) are strictly rejected (`false`).
     - Global unicast and ULA IPv6 addresses are accepted without requiring scope (`true`).
   - `choose_preferred_endpoint(addresses: &[(IpAddr, Option<u32>)]) -> Option<DiscoveredEndpoint>`:
     - **Priority 1**: Usable IPv4 address (`is_usable_ipv4(&ip)`).
     - **Priority 2**: Usable global or ULA IPv6 address (`is_usable_ipv6(&ip)`).
     - **Priority 3**: Scoped link-local IPv6 with positive numeric scope ID (`s > 0`). Formatted as `<ipv6>%<scope_id>`.
     - Aliased as `choose_preferred_ip_with_scope` and `choose_and_format_address` for caller and test compatibility.
   - `validate_resolved_service(fullname, host_target, srv_port, txt, endpoint) -> Result<DiscoveredHost, DiscoveryError>`:
     - Validates payload boundaries: fullname (1..=255, contains `._erd._tcp.`), nonzero srv_port, TXT limits (<= 1300B total, key 1..=255, value <= 255, ASCII without `=`), protocol version `"3"`, name (1..=255 UTF-8), os (1..=64 UTF-8), nonzero udp_port.
     - Validates endpoint usability via `is_usable_endpoint(endpoint)` without stripping interface scope.
     - Constructs `DiscoveredHost` with `host.ip = endpoint.formatted.clone()`.
   - `decide_service_state_action`:
     - Selects preferred endpoint via `choose_preferred_endpoint`.
     - Validates service via `validate_resolved_service`.
     - Emits `ServiceStateAction::Publish(host)` retaining scoped IP, or `ServiceStateAction::Retract` on invalid target, unusable address, or invalid TXT payload.
2. **Apple Production Path Integration (`erd-net/src/discovery/apple.rs`)**:
   - Replaced redundant local copies of `choose_and_format_address`, `ServiceStateAction`, and `decide_service_state_action` with direct clean public re-exports and internal imports from `super`:
     ```rust
     pub use super::{
         choose_and_format_address, choose_preferred_endpoint, choose_preferred_ip_with_scope,
         decide_service_state_action, ServiceStateAction,
     };
     use super::{DiscoveredHost, DiscoveryError, DiscoveryTracker};
     ```
   - Avoids duplicate local bindings (E0252) by using `pub use super::{...}` for intended public re-exports and internal uses of decision functions, and `use super::{...}` only for private types (`DiscoveredHost`, `DiscoveryError`, `DiscoveryTracker`).
   - Removed imports made unused by extraction (`validate_resolved_service`, `DiscoveredEndpoint`).
   - Production Apple DNSService workers at lines 653 and 775 call the shared `decide_service_state_action` directly.
   - Added `decide_service_state_action_publishes_scoped_link_local_ipv6` to Apple unit test module.
3. **Preservation of Existing Callers & Public JSON Shape (`erd-net/src/discovery.rs`)**:
   - `DiscoveredHost` struct layout, field names (`id`, `name`, `ip`, `os`, `tcp_port`, `udp_port`), and JSON serialization are strictly unchanged.
   - `parse_service_metadata(fullname, host_target, srv_port, txt, addresses: &[IpAddr])` preserves its exact signature and contract, mapping `&[IpAddr]` to `(ip, None)` tuples and delegating to `validate_resolved_service`. All 18 existing tests in `erd-net/tests/discovery_metadata.rs` pass with zero changes.
   - Added `parse_service_metadata_scoped(..., addresses: &[(IpAddr, Option<u32>)])` for callers providing interface scopes.

### 1.3 Client Resolver Verification
Proved that published scoped strings resolve directly via standard Rust `ToSocketAddrs`:
- When `host.ip` is `"fe80::1%5"` and `host.tcp_port` is `19730`, calling `(host.ip.as_str(), host.tcp_port).to_socket_addrs()` returns `SocketAddr::V6(addr)` where:
  - `addr.ip() == Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)`
  - `addr.port() == 19730`
  - `addr.scope_id() == 5`
  - `addr.to_string() == "[fe80::1%5]:19730"`
- Verified interface name scope handling (e.g. `"fe80::1%lo"`), confirming POSIX `getaddrinfo` resolution to the loopback interface numeric index.

---

## 2. File Ownership and Code Hygiene

### 2.1 File Ownership Table

| File Path | Status | Pure LOC | Role & Summary |
|---|---|---|---|
| `clients/rust/erd-net/src/discovery/endpoint.rs` | New Shared Module | 211 LOC | DiscoveredEndpoint, preference selection, usable IPv6 predicate with scope, `validate_resolved_service`, `decide_service_state_action`. |
| `clients/rust/erd-net/src/discovery.rs` | Modified Source | 135 LOC | Re-export pure endpoint items; delegated `parse_service_metadata` and `parse_service_metadata_scoped`. Kept well under 250 LOC. |
| `clients/rust/erd-net/src/discovery/apple.rs` | Modified Source | 1085 LOC | Imported shared decision path from `super`; removed duplicated local definitions; added scoped IPv6 publication unit test. |
| `clients/rust/erd-net/tests/discovery_scoped_ipv6.rs` | New Test Target | 176 LOC | Dedicated R10 regression suite (8 tests) covering publication, retraction, preference, resolver scope retention, and interface names. |

### 2.2 LOC Measurement & Smell Audit
- Measured via `awk '!/^[[:space:]]*$/ && !/^[[:space:]]*(\/\/|#|--)/' <file> | wc -l`:
  - `erd-net/src/discovery.rs`: 135 pure LOC (Healthy, <= 200)
  - `erd-net/src/discovery/endpoint.rs`: 211 pure LOC (Healthy band, <= 250)
  - `erd-net/tests/discovery_scoped_ipv6.rs`: 176 pure LOC (Healthy, <= 200)
- Single responsibility: `endpoint.rs` owns endpoint representation and service state action decisions.
- Boundary purity: Untrusted inputs (TXT records, address slices) parsed at boundary into typed `DiscoveredEndpoint` and `DiscoveredHost`.
- Variant matching: Exhaustive match on `ServiceStateAction` and `endpoint.ip`.
- Zero `unwrap` in production code; zero compiler/clippy warnings under strict `-D warnings`.
- No edits to `erd-app`, `erd-host`, or shell crates.

---

## 3. Pre-Action Pinned RED Phase Evidence

In strict accordance with TDD discipline, regression tests were pinned in `/var/folders/zh/7cc25lt91b1_dj577306nwdh0000gn/T/ulw-20260910-111421.XXXXXX.md.pNqwGP19PH` before modifying the decision pipeline.

### 3.1 RED Invocations and Raw Output
- **Target Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-net --test discovery_scoped_ipv6"
  ```
- **Exit Code**: `101`
- **Raw Execution Log**:
  ```text
     Compiling erd-net v0.1.0 (/home/indo/projects/erd-pairing-20260910/clients/rust/erd-net)
      Finished `test` profile [unoptimized + debuginfo] target(s) in 1.13s
       Running tests/discovery_scoped_ipv6.rs (clients/rust/target/debug/deps/discovery_scoped_ipv6-2c04201a1ca7c202)

  running 6 tests
  test test_ipv4_preferred_over_scoped_ipv6_link_local ... ok
  test test_scoped_ipv6_link_local_with_scope_zero_retracted ... ok
  test test_unscoped_ipv6_link_local_retracted ... ok
  test test_resolver_resolves_interface_name_scope ... ok
  test test_scoped_ipv6_link_local_published ... FAILED
  test test_scoped_ipv6_published_string_and_resolver_retain_scope ... FAILED

  failures:

  ---- test_scoped_ipv6_link_local_published stdout ----
  thread 'test_scoped_ipv6_link_local_published' (4138098) panicked at erd-net/tests/discovery_scoped_ipv6.rs:41:13:
  expected Publish(host) for scoped link-local fe80::1%5, got Retract
  note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

  ---- test_scoped_ipv6_published_string_and_resolver_retain_scope stdout ----
  thread 'test_scoped_ipv6_published_string_and_resolver_retain_scope' (4138100) panicked at erd-net/tests/discovery_scoped_ipv6.rs:132:40:
  expected Publish

  failures:
      test_scoped_ipv6_link_local_published
      test_scoped_ipv6_published_string_and_resolver_retain_scope

  test result: FAILED. 4 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

  error: test failed, to rerun pass `-p erd-net --test discovery_scoped_ipv6`
  ```
- **Audit**: Failure occurred for the exact architectural reason identified in R10: the unpatched Apple decision pipeline stripped scope and returned `ServiceStateAction::Retract` instead of `Publish`. No assertions were inverted.

---

## 4. Post-Action GREEN Phase Evidence

### 4.1 Scoped R10 Discovery Suite (`discovery_scoped_ipv6.rs`)
- **Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-net --test discovery_scoped_ipv6"
  ```
- **Exit Code**: `0`
- **Output**:
  ```text
     Compiling erd-net v0.1.0 (/home/indo/projects/erd-pairing-20260910/clients/rust/erd-net)
      Finished `test` profile [unoptimized + debuginfo] target(s) in 0.49s
       Running tests/discovery_scoped_ipv6.rs (clients/rust/target/debug/deps/discovery_scoped_ipv6-2c04201a1ca7c202)

  running 8 tests
  test test_choose_preferred_endpoint_selects_scoped_ipv6_and_preserves_scope ... ok
  test test_parse_service_metadata_scoped_accepts_scoped_ipv6 ... ok
  test test_ipv4_preferred_over_scoped_ipv6_link_local ... ok
  test test_resolver_resolves_interface_name_scope ... ok
  test test_unscoped_ipv6_link_local_retracted ... ok
  test test_scoped_ipv6_link_local_with_scope_zero_retracted ... ok
  test test_scoped_ipv6_published_string_and_resolver_retain_scope ... ok
  test test_scoped_ipv6_link_local_published ... ok

  test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
  ```

### 4.2 Combined Discovery Suite (`discovery_scoped_ipv6` + `discovery_metadata`)
- **Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-net --test discovery_scoped_ipv6 --test discovery_metadata"
  ```
- **Exit Code**: `0`
- **Summary**: 26 tests executed, 26 passed, 0 failed, 0 ignored.
  - `discovery_metadata.rs`: 18 passed
  - `discovery_scoped_ipv6.rs`: 8 passed

### 4.3 Full `erd-net` Crate Regression Suite
- **Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-net"
  ```
- **Exit Code**: `0`
- **Summary**: 72 tests executed, 72 passed, 0 failed, 0 ignored.
  - `src/lib.rs` (unit tests): 46 passed
  - `tests/discovery_metadata.rs`: 18 passed
  - `tests/discovery_scoped_ipv6.rs`: 8 passed

### 4.4 Strict Clippy Cleanliness
- **Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo clippy --manifest-path clients/rust/Cargo.toml -p erd-net --no-deps -- -D warnings"
  ```
- **Exit Code**: `0`
- **Result**: 0 warnings, 0 errors.

### 4.5 Workspace Downstream Type Check
- **Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo check --manifest-path clients/rust/Cargo.toml -p erd-proto -p erd-net -p erd-decode -p erd-host -p erd-app"
  ```
- **Exit Code**: `0`
- **Result**: All workspace crates depending on `erd-net` check cleanly with no type regressions.

---

## 5. Explicit Separation of Evidence

In strict accordance with the prompt ("clearly separate Linux proof from Apple code/device evidence still owed"):

### 5.1 Fully Verified on Omarchy (Linux Runner)
1. **Decision Logic & Contract Guarantees**:
   - `choose_preferred_endpoint`: Verified IPv4 priority, global/ULA IPv6 priority, and scoped link-local priority (`fe80::1%5`).
   - `is_usable_ipv6_with_scope`: Verified acceptance of link-local with `scope > 0`, rejection of unscoped link-local (`scope == None`) and zero scope (`scope == Some(0)`).
   - `validate_resolved_service`: Verified end-to-end payload validation and `DiscoveredHost` construction.
   - `decide_service_state_action`: Verified emission of `ServiceStateAction::Publish(host)` with `host.ip == "fe80::1%5"` for valid services, and `ServiceStateAction::Retract` for invalid services or addresses.
   - `parse_service_metadata` backward-compatibility: Verified all 18 existing metadata tests pass without modification.
2. **Resolver Scope Retention**:
   - Verified that `(host.ip.as_str(), host.tcp_port).to_socket_addrs()` produces `SocketAddr::V6` with `scope_id == 5` and exact string `"[fe80::1%5]:19730"`.
   - Verified interface-name resolution (`fe80::1%lo` -> positive scope index).

### 5.2 Evidence Still Owed (Physical Apple Environment / Phase E)
The following behaviors rely directly on Apple OS daemons and physical hardware and cannot run on Omarchy Linux:
1. **Live `mDNSResponder` / `DNSServiceBrowse` Daemon**:
   - Real-time LAN advertising and browsing through Apple's native `DNSServiceBrowse` FFI.
   - Dynamic interface discovery via `enumerate_allowed_physical_interfaces()` and `getifaddrs` on macOS / iOS.
2. **Apple-Target Compilation Status**:
   - Attempted direct cross-check on Omarchy via `cargo check --target aarch64-apple-darwin`. Failed at C dependency build (`ring` cc-rs failed due to missing macOS SDK / Darwin C compiler flags on Linux).
   - Linux Omarchy runner lacks Darwin cross-compilation toolchain and excludes `apple.rs` (`#[cfg(any(target_os = "macos", target_os = "ios"))]`).
   - Therefore, actual Apple-target compilation of `apple.rs` remains to be verified during physical iOS build/signing on Mac (authorized Mac exception for iOS shell worker `st_01a089e7`) or coordinator Phase E review. No false claim of Apple-target compilation is made from Linux.
3. **Physical iPhone Network Testing**:
   - Live discovery of an Apple Mac host from an actual physical iPhone over a link-local IPv6 Wi-Fi network.
   - Verification of UI host card population and PIN/key connection initiation using the scoped IPv6 endpoint on physical device.
- **Assigned Phase**: Physical iOS runtime evidence and end-to-end device testing belong to **Phase E** and coordinator final review.

---

## 6. Deliverable Checklist & Completion Verification

- [x] ONE GOAL: Implement R10 scoped IPv6 discovery end to end.
- [x] DELIVERABLE: Tested code + `.omo/pairing-20260910/reports/c-ipv6.md`.
- [x] SCOPE: `erd-net/src/discovery.rs`, `discovery/apple.rs`, discovery tests and necessary pure shared discovery helper files (`erd-net/src/discovery/endpoint.rs`).
- [x] Preserved public `DiscoveredHost` JSON shape and existing callers.
- [x] Traced and fixed `choose_preferred_ip_with_scope -> decide_service_state_action -> validate_resolved_service/publication`.
- [x] Fixed loss/rejection of scope: valid TXT/SRV with only `fe80::1` and scope 5 publishes; absent/zero scope retracts; IPv4 preference preserved.
- [x] Proved published string (`fe80::1%5`) and actual resolver input/result (`SocketAddrV6::scope_id() == 5`) retain scope.
- [x] Moved pure decision logic into shared path used by production Apple code and tested on Omarchy without test proxies.
- [x] No OS scope guessing or credential/name trust added.
- [x] Consistent interface-name and numeric positive scope support.
- [x] No edits to app or shell files.
- [x] Pinned pre-action RED and post-action GREEN evidence in notepad.
- [x] All 8 scoped discovery tests, 18 metadata tests, 72 net tests, and strict clippy clean.
- [x] Clearly separated Linux proof from Apple device evidence still owed.
