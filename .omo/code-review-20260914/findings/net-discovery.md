# Lane: net-discovery

## Scope reviewed

- `clients/rust/maho-net/src/discovery.rs` (174 lines)
- `clients/rust/maho-net/src/discovery/apple.rs` (1250 lines)
- `clients/rust/maho-net/src/discovery/endpoint.rs` (244 lines)
- `clients/rust/maho-net/src/discovery/mdns.rs` (356 lines)
- `clients/rust/maho-net/src/discovery/stub.rs` (27 lines)
- `clients/rust/maho-net/src/discovery/tracker.rs` (31 lines)
- `clients/rust/maho-net/src/bin/maho_discover.rs` (39 lines)

Total: 2121 lines.

## Findings

### [P0] Remote DoS via 100% CPU Busy-Loop on Error or Socket EOF in `AppleDnsServiceBrowser` and `AppleDnsServiceAdvertiser`
- **Location**: `clients/rust/maho-net/src/discovery/apple.rs:822-828` (and secondary site `clients/rust/maho-net/src/discovery/apple.rs:1072-1078`)
- **Evidence**:
`clients/rust/maho-net/src/discovery/apple.rs:822-828`:
```rust
                    for (idx, &handle) in dispatch_handles.iter().enumerate() {
                        let pfd_idx = 2 + idx;
                        if pfd_idx < poll_fds.len() && (poll_fds[pfd_idx].revents & libc::POLLIN != 0) {
                            // SAFETY: handle is verified non-null and polled readable.
                            unsafe { DNSServiceProcessResult(handle) };
                        }
                    }
```
`clients/rust/maho-net/src/discovery/apple.rs:1072-1078`:
```rust
                    for (idx, &h) in registered_handles.iter().enumerate() {
                        let pfd_idx = 1 + idx;
                        if pfd_idx < poll_fds.len() && (poll_fds[pfd_idx].revents & libc::POLLIN != 0) {
                            // SAFETY: h is valid and polled readable.
                            unsafe { DNSServiceProcessResult(h) };
                        }
                    }
```
- **Impact**: In both the browser event loop and advertiser event loop, the `DNSServiceErrorType` return value of `DNSServiceProcessResult` is completely ignored. When a service query or registration socket encounters an error or reaches EOF (for example, if mDNSResponder closes a query, the daemon restarts, or the socket reaches hangup), standard POSIX `poll()` reports `POLLIN` (and/or `POLLHUP`) immediately without blocking. Because the error is discarded, the dead `DNSServiceRef` handle is never deallocated or removed from `dispatch_handles`. On every subsequent loop turn, `libc::poll` returns instantaneously, causing the worker thread to spin in an infinite busy-loop at 100% CPU. This drains battery, starves the thread pool, and locks out discovery operations.
- **Fix**: Check the return code of `DNSServiceProcessResult(handle)`. If it returns any error other than `K_DNS_SERVICE_ERR_NO_ERROR`, deallocate the handle with `DNSServiceRefDeallocate`, clear the corresponding `Option<DNSServiceRef>` in `ActiveService`, and remove it from `dispatch_handles`. In `AppleDnsServiceAdvertiser`, stop the loop and record the backend error.
- **Confidence**: high

---

### [P0] Local Network Peer Can Permanently Brick All Future Discovery Snapshots via Single Failed Resolve
- **Location**: `clients/rust/maho-net/src/discovery/apple.rs:683-691` (and secondary site `clients/rust/maho-net/src/discovery/apple.rs:859-866`)
- **Evidence**:
`clients/rust/maho-net/src/discovery/apple.rs:683-691`:
```rust
                    for (key, service) in active_services.iter_mut() {
                        // SAFETY: service.context is valid and pinned on this worker thread.
                        let s_ctx = unsafe { &mut *service.context };
                        if let Some(err) = s_ctx.error.take() {
                            let mut guard = err_clone.lock().unwrap_or_else(|e| e.into_inner());
                            *guard = Some(err);
                            let mut tracker_guard = tracker_clone.lock().unwrap_or_else(|e| e.into_inner());
                            tracker_guard.remove(&s_ctx.fullname);
                        }
```
`clients/rust/maho-net/src/discovery/apple.rs:859-866`:
```rust
    pub fn snapshot(&self) -> Result<Vec<DiscoveredHost>, DiscoveryError> {
        let guard = self.last_error.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(err) = guard.clone() {
            return Err(err);
        }
        let tracker_guard = self.tracker.lock().unwrap_or_else(|e| e.into_inner());
        Ok(tracker_guard.snapshot())
    }
```
- **Impact**: In `AppleDnsServiceBrowser`, `last_error` is a shared slot that is checked on every call to `LanDiscovery::snapshot()`. Crucially, `last_error` is never reset to `None`. If any third-party device on the local network advertises a `_maho-rd._tcp` service that fails resolution (e.g. peer leaves the network before resolution completes, or mDNSResponder returns an error like `-65538`), `s_ctx.error` is copied directly into `last_error`. From that moment on, every subsequent call to `snapshot()` unconditionally returns `Err`, permanently disabling LAN discovery for the lifetime of the application, even if valid hosts are active in the tracker. Any untrusted device on the local network can remotely deny discovery service by publishing an unresolvable record.
- **Fix**: Do not propagate per-service resolution errors (`s_ctx.error`) into the browser's global `last_error`. An error resolving an individual service should only log a warning and discard that specific service. Reserve `last_error` exclusively for fatal browser-level errors (such as `DNSServiceBrowse` failing or policy denial).
- **Confidence**: high

---

### [P1] Index Desynchronization in `AppleDnsServiceAdvertiser` Dispatches Events to Wrong Handles
- **Location**: `clients/rust/maho-net/src/discovery/apple.rs:1046-1078`
- **Evidence**:
`clients/rust/maho-net/src/discovery/apple.rs:1046-1078`:
```rust
                    for &h in &registered_handles {
                        // SAFETY: h is valid registered DNSServiceRef handle.
                        let fd = unsafe { DNSServiceRefSockFD(h) };
                        if fd >= 0 {
                            poll_fds.push(libc::pollfd {
                                fd,
                                events: libc::POLLIN,
                                revents: 0,
                            });
                        }
                    }

                    // SAFETY: poll_fds is valid contiguous slice of pollfd structs.
                    let res = unsafe { libc::poll(poll_fds.as_mut_ptr(), poll_fds.len() as libc::nfds_t, -1) };
                    if res <= 0 {
                        let err_kind = io::Error::last_os_error();
                        if err_kind.kind() != io::ErrorKind::Interrupted {
                            break;
                        }
                        continue;
                    }

                    if poll_fds[0].revents & libc::POLLIN != 0 {
                        break;
                    }

                    for (idx, &h) in registered_handles.iter().enumerate() {
                        let pfd_idx = 1 + idx;
                        if pfd_idx < poll_fds.len() && (poll_fds[pfd_idx].revents & libc::POLLIN != 0) {
                            // SAFETY: h is valid and polled readable.
                            unsafe { DNSServiceProcessResult(h) };
                        }
                    }
```
- **Impact**: `poll_fds` only inserts entries when `fd >= 0`. However, the dispatch loop iterates through `registered_handles.iter().enumerate()` and accesses `poll_fds[1 + idx]`. If any handle yields `fd < 0` (which `DNSServiceRefSockFD` returns if a handle is invalid or uninitialized), the indices between `poll_fds` and `registered_handles` become desynchronized. When another handle has incoming data, `poll_fds[pfd_idx]` corresponds to the wrong handle in `registered_handles`. Calling `DNSServiceProcessResult` on the wrong handle fails to service the active socket, causes registration callbacks to be missed, and can trigger undefined or erroneous behavior in mDNSResponder.
- **Fix**: Collect `(handle, fd)` into a parallel dispatch vector when `fd >= 0` (matching the safe implementation in `AppleDnsServiceBrowser`), and iterate over that filtered vector when dispatching `DNSServiceProcessResult`.
- **Confidence**: high

---

### [P1] Premature Retraction in Multi-Interface Environments Unconditionally Erases Active Hosts
- **Location**: `clients/rust/maho-net/src/discovery/apple.rs:716-730`
- **Evidence**:
`clients/rust/maho-net/src/discovery/apple.rs:716-730`:
```rust
                        if s_ctx.state_dirty {
                            s_ctx.state_dirty = false;
                            let action = decide_service_state_action(
                                &s_ctx.fullname,
                                s_ctx.hosttarget.as_deref(),
                                s_ctx.srv_port,
                                &s_ctx.txt_items,
                                &s_ctx.addresses,
                            );
                            let mut tracker_guard = tracker_clone.lock().unwrap_or_else(|e| e.into_inner());
                            match action {
                                ServiceStateAction::Publish(host) => {
                                    tracker_guard.upsert(host);
                                }
                                ServiceStateAction::Retract => {
                                    tracker_guard.remove(&s_ctx.fullname);
                                }
                            }
                        }
```
- **Impact**: On a multi-interface system (e.g. Wi-Fi and Ethernet active simultaneously), Bonjour creates separate `ActiveService` instances for each interface with the same `fullname`. When interface 1 resolves, it publishes the host to `tracker`. When interface 2 resolves its SRV/TXT records, `resolve_callback` marks `s_ctx.state_dirty = true` before its address resolution (`addr_callback`) has completed (`s_ctx.addresses` is still empty). On the next loop turn, `decide_service_state_action` evaluates interface 2's empty address list and returns `ServiceStateAction::Retract`. Line 728 unconditionally deletes `&s_ctx.fullname` from `tracker`, wiping out the valid host published by interface 1. If interface 2 cannot resolve addresses on its subnet, the host is permanently deleted from `tracker`, even though interface 1 resolved it completely.
- **Fix**: Before removing `&s_ctx.fullname` on `ServiceStateAction::Retract`, check whether another `ActiveService` sharing that `fullname` is in a valid published state (identical to the surviving host check implemented for `BrowseOp::Removed` at lines 582-602). Only call `tracker_guard.remove(&s_ctx.fullname)` if no other interface has a publishable state.
- **Confidence**: high

---

### [P1] Denial-of-Service and Permanent Peer Starvation via Unbounded Retention of Unresponsive Services
- **Location**: `clients/rust/maho-net/src/discovery/apple.rs:620-625` (and secondary site `clients/rust/maho-net/src/discovery/apple.rs:683-691`)
- **Evidence**:
`clients/rust/maho-net/src/discovery/apple.rs:620-625`:
```rust
                                if active_services.len() >= MAX_CONCURRENT_SERVICES {
                                    continue;
                                }
                                if active_services.contains_key(&key) {
                                    continue;
                                }
```
`clients/rust/maho-net/src/discovery/apple.rs:683-691`:
```rust
                    for (key, service) in active_services.iter_mut() {
                        // SAFETY: service.context is valid and pinned on this worker thread.
                        let s_ctx = unsafe { &mut *service.context };
                        if let Some(err) = s_ctx.error.take() {
                            let mut guard = err_clone.lock().unwrap_or_else(|e| e.into_inner());
                            *guard = Some(err);
                            let mut tracker_guard = tracker_clone.lock().unwrap_or_else(|e| e.into_inner());
                            tracker_guard.remove(&s_ctx.fullname);
                        }
```
- **Impact**: `active_services` is bounded by `MAX_CONCURRENT_SERVICES = 64`. Entries are inserted upon `BrowseOp::Added` and are only ever removed upon `BrowseOp::Removed`. If a service fails to respond to `DNSServiceResolve` or `DNSServiceGetAddrInfo`, or if resolution fails with an error (`s_ctx.error`), the entry is retained in `active_services` indefinitely without any timeout or eviction. Once 64 unresponsive or dead services accumulate (which is readily triggered by LAN port scans, stale mDNS announcements, or adversarial flooding), line 621 drops all new `BrowseOp::Added` events. Because Bonjour browse notifications are edge-triggered, legitimate peers joining later are permanently ignored and will never be discovered.
- **Fix**: Add a creation timestamp or resolution timeout to `ActiveService`. Periodically sweep `active_services` and evict services whose resolution has timed out (e.g. after 10 seconds), or when `s_ctx.error` is encountered, deallocating their `DNSServiceRef` handles and freeing their boxed contexts.
- **Confidence**: high

---

### [P1] Unbounded Peer Map Growth in `DiscoveryTracker` and `mdns.rs` Without Capacity Limits or TTL
- **Location**: `clients/rust/maho-net/src/discovery/tracker.rs:16-18` (and secondary site `clients/rust/maho-net/src/discovery/mdns.rs:82-90`)
- **Evidence**:
`clients/rust/maho-net/src/discovery/tracker.rs:16-18`:
```rust
    pub fn upsert(&mut self, host: DiscoveredHost) {
        self.hosts.insert(host.id.clone(), host);
    }
```
`clients/rust/maho-net/src/discovery/mdns.rs:82-90`:
```rust
                                if let Ok(host) = parse_service_metadata(
                                    &fullname,
                                    &host_target,
                                    srv_port,
                                    &txt_items,
                                    &addrs,
                                ) {
                                    if let Ok(mut tr_guard) = tracker_clone.lock() {
                                        tr_guard.upsert(host);
                                    }
                                }
```
- **Impact**: `DiscoveryTracker` stores discovered hosts in a `BTreeMap<String, DiscoveredHost>` with no upper bound on element count, no TTL, and no timestamp tracking. In `mdns.rs`, incoming resolved services are inserted without any rate limit or map size check. A malicious node or misconfigured device on the local network that transmits mDNS advertisements with randomized service names can force `DiscoveryTracker` to allocate memory without bound until the host process runs out of memory (OOM). Furthermore, peers that disconnect ungracefully without sending mDNS goodbye packets remain in the tracker indefinitely.
- **Fix**: Enforce a maximum capacity limit (e.g. 256 hosts) in `DiscoveryTracker::upsert`. Store a `last_seen: Instant` on each entry and add a periodic sweep method that purges hosts that have not refreshed within a bounded TTL.
- **Confidence**: high

---

### [P2] Scope ID Stripped in `mdns.rs` Drops All Scoped IPv6 Link-Local Hosts on Non-Apple Platforms
- **Location**: `clients/rust/maho-net/src/discovery/mdns.rs:77-85` (and secondary site `clients/rust/maho-net/src/discovery.rs:78-84`)
- **Evidence**:
`clients/rust/maho-net/src/discovery/mdns.rs:77-85`:
```rust
                                let addrs: Vec<IpAddr> =
                                    info.get_addresses().iter().copied().collect();

                                if let Ok(host) = parse_service_metadata(
                                    &fullname,
                                    &host_target,
                                    srv_port,
                                    &txt_items,
                                    &addrs,
                                ) {
```
`clients/rust/maho-net/src/discovery.rs:78-84`:
```rust
pub fn parse_service_metadata(
    fullname: &str,
    host_target: &str,
    srv_port: u16,
    txt: &[(String, Vec<u8>)],
    addresses: &[IpAddr],
) -> Result<DiscoveredHost, DiscoveryError> {
    if addresses.is_empty() {
        return Err(DiscoveryError::InvalidPayload("no addresses found".into()));
    }
    let addr_tuples: Vec<(IpAddr, Option<u32>)> = addresses.iter().map(|&ip| (ip, None)).collect();
    let endpoint = choose_preferred_endpoint(&addr_tuples).ok_or_else(|| {
        DiscoveryError::InvalidPayload("no usable unicast address found".into())
    })?;
```
- **Impact**: In `mdns.rs` (the Linux and Windows implementation), resolved addresses are passed to `parse_service_metadata`, which strips scope IDs by mapping every address to `(ip, None)`. In `choose_preferred_endpoint`, link-local IPv6 addresses require a positive scope (`scope.filter(|&s| s > 0)`). Because the scope ID is stripped to `None`, any host that only advertises an IPv6 link-local address is rejected with `DiscoveryError::InvalidPayload("no usable unicast address found")`. Scoped link-local IPv6 discovery is completely non-functional on Linux and Windows, despite `parse_service_metadata_scoped` existing specifically to support it.
- **Fix**: In `mdns.rs`, resolve the interface index or scope ID from the network interface receiving the mDNS event, and invoke `parse_service_metadata_scoped` with the appropriate `Option<u32>` scope IDs instead of discarding them.
- **Confidence**: high

---

### [P2] Unspecified Bind Registers Separate Conflicting Instances on Every Physical Interface
- **Location**: `clients/rust/maho-net/src/discovery/apple.rs:952-953` (and secondary site `clients/rust/maho-net/src/discovery/apple.rs:998-1017`)
- **Evidence**:
`clients/rust/maho-net/src/discovery/apple.rs:952-953`:
```rust
        } else if bind_addr.ip().is_unspecified() {
            enumerate_allowed_physical_interfaces()?
```
`clients/rust/maho-net/src/discovery/apple.rs:998-1017`:
```rust
                for &if_index in &interfaces_to_register {
                    let mut reg_ref: DNSServiceRef = std::ptr::null_mut();
                    // SAFETY: reg_ref is valid mutable pointer, c-strings and txt_bytes are valid, reg_ctx_ptr is heap-pinned.
                    let err = unsafe {
                        DNSServiceRegister(
                            &mut reg_ref,
                            0,
                            if_index,
                            c_name.as_ptr(),
                            REGTYPE_C_STR.as_ptr(),
                            std::ptr::null(),
                            std::ptr::null(),
                            port_be,
                            txt_bytes.len() as u16,
                            txt_bytes.as_ptr() as *const libc::c_void,
                            Some(register_callback),
                            reg_ctx_ptr as *mut libc::c_void,
                        )
                    };
```
- **Impact**: When the advertiser binds to an unspecified address (`0.0.0.0` or `::`), `enumerate_allowed_physical_interfaces()` enumerates all active physical interfaces (e.g. `en0`, `en1`, and Apple internal mesh interfaces `awdl0`/`llw0`). The advertiser then calls `DNSServiceRegister` separately for each interface with the same service name. In mDNSResponder, registering the same service name repeatedly triggers name conflicts and auto-renaming, causing the host to appear as `Host`, `Host (2)`, and `Host (3)` across interfaces. This creates duplicate conflicting records for peer browsers on the network.
- **Fix**: When `bind_addr.ip().is_unspecified()`, pass `interfaceIndex = 0` to a single `DNSServiceRegister` call. mDNSResponder automatically registers on all active interfaces without naming conflicts.
- **Confidence**: high

---

### [P2] TXT Record Serialization Silently Drops Host Names Between 251 and 255 Bytes
- **Location**: `clients/rust/maho-net/src/discovery/apple.rs:980-988`
- **Evidence**:
`clients/rust/maho-net/src/discovery/apple.rs:980-988`:
```rust
                let os_name = std::env::consts::OS;
                let txt = format!("protocol=3\0name={name_owned}\0os={os_name}\0udp_port={udp_port}");
                let mut txt_bytes = Vec::new();
                for part in txt.split('\0') {
                    if !part.is_empty() && part.len() <= 255 {
                        txt_bytes.push(part.len() as u8);
                        txt_bytes.extend_from_slice(part.as_bytes());
                    }
                }
```
- **Impact**: In `endpoint.rs`, `validate_resolved_service` permits host names up to 255 bytes (`name.len() <= 255`). However, `AppleDnsServiceAdvertiser` formats the entry as `"name={name_owned}"`, prepending 5 bytes (`"name="`). If `name_owned` has a length between 251 and 255 bytes, `part.len()` will be between 256 and 260 bytes. Because `part.len() <= 255` evaluates to false, the entire `"name=..."` attribute is silently omitted from the advertised TXT record. Any client resolving this service will fail validation with `DiscoveryError::InvalidPayload("missing name in TXT")`, rendering the service unresolvable.
- **Fix**: Explicitly validate at the start of `AppleDnsServiceAdvertiser::start` that `name.len() <= 250` (or `name.len() + 5 <= 255`), returning `DiscoveryError::InvalidPayload` immediately if the name cannot fit within a single DNS-SD TXT attribute string.
- **Confidence**: high

---

### [P2] `addr_callback` Permits Sentinel Interface Index `!0u32` as a Valid IPv6 Scope ID
- **Location**: `clients/rust/maho-net/src/discovery/apple.rs:460-466`
- **Evidence**:
`clients/rust/maho-net/src/discovery/apple.rs:460-466`:
```rust
            } else if family == libc::AF_INET6 {
                let in6_addr = address as *const libc::sockaddr_in6;
                let ip = Ipv6Addr::from((*in6_addr).sin6_addr.s6_addr);
                let scope = (*in6_addr).sin6_scope_id;
                let effective_scope = if scope != 0 { scope } else { interface_index };
                (Some(IpAddr::V6(ip)), Some(effective_scope))
```
- **Impact**: In Apple's mDNSResponder FFI, `interface_index` can be passed as `kDNSServiceInterfaceIndexLocalOnly` (`!0u32` = `4294967295`) for local-only records. If `sin6_scope_id == 0`, `effective_scope` becomes `4294967295`. `DiscoveredEndpoint::new` tests `scope > 0`, which evaluates to true, formatting the IP address as `fe80::...%4294967295`. When clients attempt to connect to this endpoint via `to_socket_addrs()`, socket creation or connection fails because interface index `4294967295` is an invalid system network interface.
- **Fix**: Check `interface_index != K_DNS_SERVICE_INTERFACE_INDEX_LOCAL_ONLY` before assigning it as `effective_scope`, treating sentinel or negative/max values as `None`.
- **Confidence**: high

## Non-findings checked

- `clients/rust/maho-net/src/discovery/apple.rs`: FFI panic boundary safety: all C callbacks (`browse_callback`, `resolve_callback`, `addr_callback`, `register_callback`) wrap execution inside `std::panic::catch_unwind(AssertUnwindSafe(...))` preventing unwinding across the `extern "C"` ABI.
- `clients/rust/maho-net/src/discovery/apple.rs`: Heap context pointer ownership pairing: `browse_ctx_ptr`, `s_ctx_ptr`, and `reg_ctx_ptr` allocated via `Box::into_raw` are consistently reclaimed via `drop(Box::from_raw(...))` across all startup, removal, error, and drop paths.
- `clients/rust/maho-net/src/discovery/apple.rs`: Network byte order conversions: `port` in `resolve_callback` is correctly converted from big-endian via `u16::from_be`, and converted to big-endian via `port.to_be()` in `DNSServiceRegister`.
- `clients/rust/maho-net/src/discovery/apple.rs`: Socket address family extraction: `(*ifa.ifa_addr).sa_family` correctly accesses the 1-byte BSD `sa_family_t` at offset 1 on Darwin.
- `clients/rust/maho-net/src/discovery/apple.rs`: Interface list memory deallocation: `libc::freeifaddrs` is called on all code paths in `lookup_interface_index_for_ip` and `enumerate_allowed_physical_interfaces`.
- `clients/rust/maho-net/src/discovery/apple.rs`: Worker thread wake-up signaling: `UnixStream` socket pair (`wake_tx`/`wake_rx`) configured with `set_nonblocking(true)` and `libc::MSG_NOSIGNAL` prevents blocking and SIGPIPE during thread termination.
- `clients/rust/maho-net/src/discovery/endpoint.rs`: TXT payload bounds enforcement: `validate_resolved_service` strictly validates that individual keys and values do not exceed 255 bytes and total TXT payload does not exceed 1300 bytes.
- `clients/rust/maho-net/src/discovery/endpoint.rs`: Protocol version gating: records with protocol versions other than `"3"` are rejected before publishing.
- `clients/rust/maho-net/src/discovery/endpoint.rs`: Zero port validation: SRV and UDP ports are checked for non-zero values, rejecting unroutable service definitions.
- `clients/rust/maho-net/src/discovery/apple.rs`: Safe TXT cursor stepping: `resolve_callback` advances the cursor before length slicing and checks `cursor + len <= slice.len()`, preventing out-of-bounds reads on truncated TXT buffers.
- `clients/rust/maho-net/src/discovery/stub.rs`: Stub platform safety: clean error propagation returning `DiscoveryError::Unavailable` on unsupported operating systems.
- `clients/rust/maho-net/src/discovery/endpoint.rs`: Endpoint priority ordering: `choose_preferred_endpoint` reliably prioritizes usable unicast IPv4, then global/ULA IPv6, then scoped link-local IPv6, correctly filtering unroutable endpoints.
- `clients/rust/maho-net/src/bin/maho_discover.rs`: CLI argument parsing: correctly handles `--timeout-secs` and `--help` flags and reports invalid arguments.
