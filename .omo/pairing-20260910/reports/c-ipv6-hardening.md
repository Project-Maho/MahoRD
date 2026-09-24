# Phase C Task Implementation Report: R10 Interface-Name Resolver Assertion Hardening

- Task ID: `st_01a08a51` / `st_01a08a59` (Follow-Up)
- Goal: Make R10 interface-name resolver evidence fail reliably when its asserted behavior is absent, with explicit POSIX platform cfg guards and strict assertions.
- Worker: `hephaestus`
- Parent Session: `01a0890a-69f1-7e5e-80cb-959dc3ddb61c` (Depth: 1)
- Date: 2026-09-10
- Base Commits: Accepted A/B increments (`0eafe10`, `9a364a9`, `fa476b1`, `8c4570e`)
- Target Remote: `indo@100.91.254.71` (`/home/indo/projects/erd-pairing-20260910`)
- Environment: `PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:$LD_LIBRARY_PATH`
- Deliverables:
  - Hardened regression test suite: `clients/rust/erd-net/tests/discovery_scoped_ipv6.rs`
  - Evidence logs: `.omo/pairing-20260910/evidence/identity-resolver-hardening-sensitivity.log`, `.omo/pairing-20260910/evidence/identity-resolver-hardening-positive.log`
  - Scoped report: `.omo/pairing-20260910/reports/c-ipv6-hardening.md`
  - Pinned plan & execution log in append-only notepad: `/var/folders/zh/7cc25lt91b1_dj577306nwdh0000gn/T/ulw-20260910-111421.XXXXXX.md.pNqwGP19PH`

---

## 1. Problem Statement and Root Cause

The lead review identified two successive defects in `clients/rust/erd-net/tests/discovery_scoped_ipv6.rs`:

### 1.1 Silent Pass Failure Modes in the Original Test
The test `test_resolver_resolves_interface_name_scope` was originally written with nested conditionals:
```rust
#[test]
fn test_resolver_resolves_interface_name_scope() {
    if let Ok(iter) = ("fe80::1%lo", 19730u16).to_socket_addrs() {
        let list: Vec<_> = iter.collect();
        if let Some(std::net::SocketAddr::V6(v6_addr)) = list.first() {
            assert_eq!(*v6_addr.ip(), Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1));
            assert_eq!(v6_addr.port(), 19730);
            assert!(v6_addr.scope_id() > 0, "interface name must resolve to positive scope_id");
        }
    }
}
```
1. **Resolver Failure (`Err`)**: If `to_socket_addrs()` failed (e.g. invalid interface name, platform resolver failure, unparseable input), `if let Ok` was bypassed entirely. The test executed 0 assertions and passed silently.
2. **Empty Address List**: If `to_socket_addrs()` returned an empty iterator, `list.first()` returned `None`. The test bypassed inner assertions and passed silently.
3. **IPv4-Only Result**: If `list.first()` yielded `SocketAddr::V4`, `if let Some(SocketAddr::V6)` did not match. The test executed 0 assertions and passed silently.
4. **Weak Scope Assertion**: The original test only asserted `v6_addr.scope_id() > 0`, without validating the expected interface index on the supported runner.

### 1.2 Lead Follow-Up: Overbroad `not(Apple)` CFG Guard
In the initial hardening pass, `libc_compat` extern `if_nametoindex` and `LOOPBACK_IFACE = "lo"` were guarded with `#[cfg(not(any(target_os = "macos", target_os = "ios")))]`.
- **Defect**: This guard inadvertently included Windows (`target_os = "windows"`). POSIX `if_nametoindex` is not a cross-platform Windows binding, and `"lo"` is not a valid Windows network interface.
- **Resolution Mandate**: Restrict POSIX interface-name helpers, imports, externs, and both positive and negative OS-name resolver tests explicitly to supported Linux, macOS, and iOS platforms (`target_os = "linux"` and `any(target_os = "macos", target_os = "ios")`).
- **Unconditional Baseline**: Keep ALL numeric-scope, publication, scope-zero, and IPv4-priority tests unconditional.
- **No Extra Dependencies**: No new platform fallbacks, raw Windows FFI, hardcoded indices, or extra crate dependencies.
- **SAFETY Documentation**: Add explicit `SAFETY` explanation for the raw C FFI call.

---

## 2. Hardening Implementation

In strict compliance with the task brief and lead guidance:
- **Scope Restriction**: Only `clients/rust/erd-net/tests/discovery_scoped_ipv6.rs` was modified. Production code was unchanged. Concurrent `ios-shell` files were not touched or synced.
- **No Swallowed Errors**: Nested `if let` blocks were eliminated in favor of strict unconditional expectations.
- **Explicit Platform CFG Boundaries**:
  - `LOOPBACK_IFACE`: `"lo0"` on `#[cfg(any(target_os = "macos", target_os = "ios"))]`; `"lo"` on `#[cfg(target_os = "linux")]`. Windows and other platforms do not define this constant.
  - `if_nametoindex`: `libc::if_nametoindex` on Apple; `libc_compat::if_nametoindex` on Linux.
  - `get_platform_loopback_interface()`: Guarded by `#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]`.
  - Both positive (`test_resolver_resolves_interface_name_scope`) and negative (`test_resolver_rejects_invalid_interface_name_scope`) OS-name tests: Guarded by `#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]`.
  - On non-POSIX targets (e.g. Windows), these OS-name tests are omitted at compile time via `cfg`, leaving unconditional numeric-scope and publication tests intact.
- **Strict Linux/Apple Boundary**: This is a compile-time target boundary, not permission to skip an assertion failure on Linux or Apple. On supported runners, the test executes all assertions strictly.

### 2.1 Refined Platform Interface Definitions & SAFETY Documentation
```rust
#[cfg(any(target_os = "macos", target_os = "ios"))]
const LOOPBACK_IFACE: &str = "lo0";
#[cfg(target_os = "linux")]
const LOOPBACK_IFACE: &str = "lo";

#[cfg(any(target_os = "macos", target_os = "ios"))]
use libc::if_nametoindex;

#[cfg(target_os = "linux")]
mod libc_compat {
    extern "C" {
        pub fn if_nametoindex(ifname: *const std::ffi::c_char) -> u32;
    }
}
#[cfg(target_os = "linux")]
use libc_compat::if_nametoindex;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
fn get_platform_loopback_interface() -> (&'static str, u32) {
    let c_name =
        std::ffi::CString::new(LOOPBACK_IFACE).expect("valid C string for loopback interface");
    // SAFETY: `c_name` is an owned, null-terminated C string (`CString`) pointing to a
    // statically allocated ASCII interface name ("lo" on Linux, "lo0" on macOS/iOS).
    // The pointer is valid and non-null for the lifetime of `c_name`.
    // The POSIX `if_nametoindex` function reads the string up to the null terminator
    // (at most IFNAMSIZ characters) without mutating memory or retaining the pointer.
    let idx = unsafe { if_nametoindex(c_name.as_ptr()) };
    (LOOPBACK_IFACE, idx)
}
```

### 2.2 Hardened Positive & Negative OS-Name Resolver Tests
```rust
#[test]
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
fn test_resolver_resolves_interface_name_scope() {
    let (iface, expected_scope) = get_platform_loopback_interface();
    assert!(
        expected_scope > 0,
        "platform loopback interface '{iface}' must have positive index via if_nametoindex"
    );

    let host_str = format!("fe80::1%{iface}");
    let list: Vec<_> = (host_str.as_str(), 19730u16)
        .to_socket_addrs()
        .expect("interface name resolver must succeed for platform loopback interface")
        .collect();

    assert!(
        !list.is_empty(),
        "interface name resolver must return at least one address"
    );

    match list[0] {
        std::net::SocketAddr::V6(v6_addr) => {
            assert_eq!(*v6_addr.ip(), Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1));
            assert_eq!(v6_addr.port(), 19730);
            assert!(
                v6_addr.scope_id() > 0,
                "interface name must resolve to positive scope_id"
            );
            assert_eq!(
                v6_addr.scope_id(),
                expected_scope,
                "resolver scope_id must match exact index of '{iface}' from if_nametoindex"
            );
        }
        std::net::SocketAddr::V4(_) => {
            panic!("expected IPv6 socket address for scoped link-local");
        }
    }
}

#[test]
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
fn test_resolver_rejects_invalid_interface_name_scope() {
    let result = ("fe80::1%nonexistent_iface99", 19730u16).to_socket_addrs();
    assert!(
        result.is_err(),
        "interface name resolver must return Err for nonexistent interface"
    );
}
```

### 2.3 Unconditional Tests Across All Platforms
The following 7 tests remain completely unconditional without `cfg` attributes:
1. `test_scoped_ipv6_link_local_published`: Link-local `fe80::1%5` publishes to `ServiceStateAction::Publish`.
2. `test_unscoped_ipv6_link_local_retracted`: Link-local without scope retracts.
3. `test_scoped_ipv6_link_local_with_scope_zero_retracted`: Link-local with `Some(0)` retracts.
4. `test_ipv4_preferred_over_scoped_ipv6_link_local`: IPv4 `192.168.1.50` preferred over scoped IPv6.
5. `test_scoped_ipv6_published_string_and_resolver_retain_scope`: Published numeric scope `"fe80::1%5"` resolves via `ToSocketAddrs` to `SocketAddr::V6` with `scope_id == 5`.
6. `test_choose_preferred_endpoint_selects_scoped_ipv6_and_preserves_scope`: Direct endpoint selection logic.
7. `test_parse_service_metadata_scoped_accepts_scoped_ipv6`: Direct scoped metadata parser entry point.

---

## 3. Assertion-Sensitivity Evidence (Controlled Mutation)

To prove that the strengthened test fails reliably when its asserted behavior is absent, a controlled mutation was executed on the remote runner `indo@100.91.254.71`:
- **Controlled Invalidation**: The strengthened test was pointed to an invalid interface (`"fe80::1%nonexistent_iface99"`).
- **Comparison to Prior Behavior**:
  - Prior implementation with `if let Ok`: exited with code 0 (passed silently with 0 assertions executed).
  - Strengthened implementation: panics immediately at `.expect()` with exit code 101.
- **Classification**: Labeled strictly as **assertion-sensitivity evidence** rather than original R10 RED. Original publication/resolver RED artifacts from Phase C (`c-ipv6.md`) remain untouched and preserved.

### 3.1 Raw Failure Output (Exit Code 101)
Captured in `.omo/pairing-20260910/evidence/identity-resolver-hardening-sensitivity.log`:
```text
   Compiling erd-net v0.1.0 (/home/indo/projects/erd-pairing-20260910/clients/rust/erd-net)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.57s
     Running tests/discovery_scoped_ipv6.rs (clients/rust/target/debug/deps/discovery_scoped_ipv6-2c04201a1ca7c202)

running 1 test
test test_resolver_resolves_interface_name_scope ... FAILED

failures:

---- test_resolver_resolves_interface_name_scope stdout ----
thread 'test_resolver_resolves_interface_name_scope' (4186263) panicked at erd-net/tests/discovery_scoped_ipv6.rs:159:10:
interface name resolver must succeed for real existing runner interface 'lo': Custom { kind: Uncategorized, error: "failed to lookup address information: Name or service not known" }
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    test_resolver_resolves_interface_name_scope

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 7 filtered out; finished in 0.00s

error: test failed, to rerun pass `-p erd-net --test discovery_scoped_ipv6`
```

---

## 4. Positive Remote Verification (Linux Runner)

The full suite of 27 discovery tests was executed against the remote runner `indo@100.91.254.71`.

### 4.1 Combined Discovery Test Suite
- **Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo test --manifest-path clients/rust/Cargo.toml -p erd-net --test discovery_scoped_ipv6 --test discovery_metadata"
  ```
- **Exit Code**: `0`
- **Results**:
  - `discovery_metadata.rs`: 18 passed, 0 failed
  - `discovery_scoped_ipv6.rs`: 9 passed, 0 failed (all 9 passed, including `test_resolver_resolves_interface_name_scope` with loopback interface index querying and `test_resolver_rejects_invalid_interface_name_scope`)
  - Total: 27 passed, 0 failed.
- Captured in: `.omo/pairing-20260910/evidence/identity-resolver-hardening-positive.log`

### 4.2 Verbatim Test Output
```text
   Compiling erd-net v0.1.0 (/home/indo/projects/erd-pairing-20260910/clients/rust/erd-net)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.80s
     Running tests/discovery_metadata.rs (clients/rust/target/debug/deps/discovery_metadata-1c7355981dce07a4)

running 18 tests
test parse_service_metadata_accepts_publicly_numbered_ethernet_ip ... ok
test parse_service_metadata_accepts_valid_v3_records_and_prefers_ipv4 ... ok
test parse_service_metadata_rejects_invalid_fullname ... ok
test parse_service_metadata_rejects_missing_protocol ... ok
test parse_service_metadata_rejects_oversized_metadata ... ok
test parse_service_metadata_rejects_unscoped_ipv6_link_local_only ... ok
test parse_service_metadata_rejects_unspecified_only_addresses ... ok
test parse_service_metadata_rejects_loopback_only_addresses ... ok
test parse_service_metadata_rejects_empty_addresses ... ok
test parse_service_metadata_rejects_unsupported_protocol ... ok
test parse_service_metadata_rejects_zero_or_missing_ports ... ok
test tracker_add_service_makes_host_visible_in_snapshot ... ok
test tracker_address_replacement_updates_existing_entry ... ok
test tracker_clear_resets_snapshot_to_empty ... ok
test tracker_remove_service_removes_host_from_snapshot ... ok
test parse_service_metadata_rejects_multicast_only_addresses ... ok
test parse_service_metadata_accepts_usable_ipv6_when_ipv4_absent ... ok
test parse_service_metadata_rejects_out_of_range_udp_port ... ok

test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/discovery_scoped_ipv6.rs (clients/rust/target/debug/deps/discovery_scoped_ipv6-2c04201a1ca7c202)

running 9 tests
test test_ipv4_preferred_over_scoped_ipv6_link_local ... ok
test test_choose_preferred_endpoint_selects_scoped_ipv6_and_preserves_scope ... ok
test test_parse_service_metadata_scoped_accepts_scoped_ipv6 ... ok
test test_unscoped_ipv6_link_local_retracted ... ok
test test_resolver_rejects_invalid_interface_name_scope ... ok
test test_scoped_ipv6_link_local_with_scope_zero_retracted ... ok
test test_resolver_resolves_interface_name_scope ... ok
test test_scoped_ipv6_link_local_published ... ok
test test_scoped_ipv6_published_string_and_resolver_retain_scope ... ok

test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

### 4.3 Clippy Cleanliness
- **Command**:
  ```bash
  ssh indo@100.91.254.71 "cd /home/indo/projects/erd-pairing-20260910 && PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:\$LD_LIBRARY_PATH cargo clippy --manifest-path clients/rust/Cargo.toml -p erd-net --tests -- -D warnings"
  ```
- **Exit Code**: `0`
- **Result**: 0 warnings, 0 errors across crate and tests under `-D warnings`.

---

## 5. Explicit CFG Boundaries and Platform Evidence Separation

In strict compliance with lead instructions:
1. **POSIX-Only OS-Name Evidence**:
   - Interface-name resolution (`fe80::1%lo` / `fe80::1%lo0`) is an operating-system-specific POSIX mechanism backed by `getaddrinfo` / `if_nametoindex`.
   - On Linux, this is verified against the real `lo` interface (index > 0).
   - On macOS/iOS, this is defined for `lo0` and uses `libc::if_nametoindex`.
   - Windows is explicitly excluded from these OS-name test items via `cfg`. No raw Windows FFI or Windows loopback bindings were introduced.
2. **No False Windows Claim**:
   - No claim is made that Windows compilation was executed. The test suite was verified solely on the authorized Linux remote runner (`indo@100.91.254.71`).
3. **No Skip on Supported Runners**:
   - The `cfg` boundary distinguishes supported POSIX platforms from unsupported platforms.
   - On supported platforms (Linux/Apple), the tests run unconditionally without skips, conditional returns, or silent pass branches.
4. **Scope Isolation**:
   - Only `clients/rust/erd-net/tests/discovery_scoped_ipv6.rs` was modified in the source tree.
   - Production discovery code (`erd-net/src/discovery.rs`, `endpoint.rs`, `apple.rs`) and `Cargo.toml` remain untouched.
   - Concurrent `ios-shell` files were not modified or synced.
   - Original Phase C publication and resolver RED artifacts remain preserved in `c-ipv6.md`.
