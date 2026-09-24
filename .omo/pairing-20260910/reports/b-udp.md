# Phase B Task Implementation Report: R3 Current-Session Authenticated UDP Registration

- Task ID: `st_01a08983`
- Worker: `hephaestus`
- Session: `01a0890a-69f1-7e5e-80cb-959dc3ddb61c` (Depth: 1)
- Date: 2026-09-10
- Base Commit: `71f8b05f5a0e9d53ce0249beb46ca864b7a836f8`
- Target Remote: `indo@100.91.254.71` (`/home/indo/projects/erd-pairing-20260910`)
- Environment: `PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:$LD_LIBRARY_PATH`
- Deliverable: Tested Code + Scoped Builds + Report (`.omo/pairing-20260910/reports/b-udp.md`)

---

## 1. Executive Summary

Implemented, hardened, and verified finding **R3** (Current-session authenticated UDP registration on host and native clients) across `erd-proto`, `erd-host`, `erd-app`, and the CLI client `erd_client`:

### 1.1 Core Architecture & Protocol Invariants
1. **Mandatory Authenticated Registration Capability (Bit 7)**:
   - Defined `Capabilities::AUTHENTICATED_UDP_REGISTRATION = Capabilities(1 << 7)` in `erd-proto/src/handshake.rs`, and included it in `Capabilities::all()`.
   - Both host and client advertise this capability during initial TCP handshake negotiation.
   - Host validates that the client handshake contains bit 7 before starting media workers (`self.media_source.start`) and before transmitting `HandshakeAck`. Missing capability returns `SessionError::MissingAuthenticatedRegistration` and drops the connection without media exposure.
   - Client validates that the host `HandshakeAck` contains bit 7 before marking session `Ready` and before initiating UDP socket transmission. Missing capability returns `SessionError::MissingAuthenticatedRegistration` and aborts connection setup.
   - Preserves framing version 3 without fallback to unauthenticated plaintext probes.
2. **Authenticated Registration Packet Shape**:
   - Abolished plaintext `0xff` probes on both client and host.
   - Client generates a standard version 3 `PacketHeader` with `packet_type: PacketType::Ping`, `sequence: 0`, `timestamp_ms: current_unix_ms()`, and `flags: 0`.
   - The datagram is sealed using the session's directional `ClientToHost` cipher (`DatagramCipher::seal_datagram(&header, &[])`), producing an authentic 40-byte datagram: `[Header (12B)][Nonce (12B)][Empty Payload (0B)][GCM Tag (16B)]`.
3. **Preservation of Nonce Ownership**:
   - The registration datagram consumes nonce counter 1 on the `ClientToHost` `DatagramCipher`.
   - The cipher instance is moved directly into the client session state without re-deriving or resetting the counter. Subsequent UDP datagram transmissions strictly advance from counter 2 onward.
   - Verified through behavioral tests where the client dispatches an actual subsequent session UDP packet (`session.send_udp`), and the host receiver decrypts it using the same cipher instance with nonce counter 2.
4. **Bounded Discovery Loop & TCP Control Starvation Prevention**:
   - `HostServer::discover_udp_peer` bounds work to `MAX_UDP_DISCOVERY_BURST = 32` packets per call. Under sustained floods of invalid or post-registration packets, the method yields back to the outer session loop, preventing TCP control, ping/pong heartbeats, and admission deadlines from being starved.
5. **Endpoint Binding and Tamper/Hijack Resistance on Host**:
   - `HostServer::discover_udp_peer` strictly requires datagrams from the matching TCP peer IP.
   - Datagrams under 40 bytes are discarded immediately without state mutation.
   - Cryptographic verification via `cipher.open_datagram` decrypts the payload and authenticates the 12-byte header as AAD against the session key and session salt.
   - Validates post-authentication contract: must be `header.packet_type == PacketType::Ping` and `payload.is_empty()`. Non-ping or non-empty datagrams are discarded without state mutation.
   - Plaintext probes (`0xff`), wrong-key packets, tampered tags, wrong-direction nonces, and prior-session/replayed packets are demonstrably consumed from the socket buffer and discarded without modifying `udp_peer`.
   - **Endpoint Fixed After First Valid Registration**: Once `*udp_peer` is registered, subsequent packets (including forged or alternate-source registration attempts) cannot change the destination endpoint. Controlled media frames continue reaching only the legitimately registered socket.
6. **CLI Client Explicit Capability Mask**:
   - Updated `erd-app/src/bin/erd_client.rs:383` to include `Capabilities::AUTHENTICATED_UDP_REGISTRATION` in its explicit capability mask alongside `SessionConfig::direct`.

---

## 2. Scope Review, Provenance Audit, and Boundary Compliance

### 2.1 Provenance of `serve_with_stop` and Concurrent Host Additions
A mid-run scope review audited the presence of `serve_with_stop`, rewritten `serve` with a 100 ms nonblocking polling sleep, and `test_serve_with_stop_exits_when_flag_set` in `erd-host/src/session.rs`.
- **Ownership & Provenance Finding**: These hunks were **not authored or accepted by Phase A of this coordinator session**, nor were they authored by this worker (`hephaestus`). As established by `git status` at the start of this run and documented in `.omo/pairing-20260910/reports/concurrent-host-policy.md`, they pre-existed in the working tree as unconfirmed concurrent host-management work (`AppState::start_host` in `tauri-shell`), which directly depends on `serve_with_stop`.
- **Action Taken**: In strict compliance with coordinator instructions, these pre-existing concurrent hunks were preserved without deletion so concurrent shell code is not broken, and they are strictly excluded from all R3 diff and commit claims.
- **Tauri Shell Boundary**: All changes in `clients/rust/tauri-shell` (including `Cargo.toml`, `src-tauri/src/lib.rs`, and `page-harness.mjs`) are outside R3 scope and were **not touched**.
- **No Unrelated Cleanups**: Reverted the unrelated `last_input_ack` dereference edit in `erd-app/src/session.rs`.
- **No Production Polling APIs Added**: No production polling loops, sleep delays, or shutdown methods were added to support R3 tests. `discover_udp_peer` bounds work to 32 packets per call and returns to the existing outer loop.

### 2.2 Assigned File Ownership Table

| File Path | Status | Role & Summary of Changes |
|---|---|---|
| `clients/rust/erd-proto/src/handshake.rs` | Assigned Source | Added `Capabilities::AUTHENTICATED_UDP_REGISTRATION = Self(1 << 7);`, included in `Capabilities::all()`. |
| `clients/rust/erd-proto/tests/protocol_v3.rs` | Assigned Test | Added `capabilities_authenticated_udp_registration_round_trip` testing bit 7 wire codec and inclusion in `all()`. |
| `clients/rust/erd-host/src/session.rs` | Assigned Source & Tests | Added `SessionError::MissingAuthenticatedRegistration`; added bit 7 capability check before media start and ack; advertised bit 7 in `HandshakeAck`; secured `discover_udp_peer` with sealed empty Ping verification, fixed endpoint, bounded burst (32 packets), and hijack rejection; updated legacy fixtures to modern capability; added 3 new regression tests on real UDP sockets with independent ciphers, positive `peek_from`/`WouldBlock` consumption proofs, and zero sleeps. |
| `clients/rust/erd-app/src/session.rs` | Assigned Source & Tests | Added `SessionError::MissingAuthenticatedRegistration`; added bit 7 to `SessionConfig::direct`; added bit 7 validation in `begin_handshake` before Ready; replaced `[0xff]` send with sealed empty Ping datagram; preserved cipher nonce; updated `test_udp_cancellation_and_restart` fixture. |
| `clients/rust/erd-app/src/bin/erd_client.rs` | Assigned Source | Updated explicit capability mask in CLI `run_client` at line 383 to include `Capabilities::AUTHENTICATED_UDP_REGISTRATION`. |
| `clients/rust/erd-app/src/tcp_write_tests.rs` | Assigned Test Fixture | Updated mock host fixture to receive and verify client's authenticated registration Ping instead of `[0xff]`. |
| `clients/rust/erd-app/tests/session_mock.rs` | Assigned Test Fixture | Updated modern roundtrip fixtures to receive authenticated registration Ping; added `udp_client_rejects_host_missing_authenticated_registration_capability` and `udp_client_sends_authenticated_registration_and_preserves_nonce` with actual subsequent session UDP send (`session.send_udp`) and host counter 2 verification. |
| `clients/rust/erd-app/tests/receiver_telemetry.rs` | Assigned Test Fixture | Updated mock host harness to receive and verify client's authenticated registration Ping instead of `[0xff]`. |
| `clients/rust/erd-app/tests/cli_receiver_telemetry.rs` | Assigned Test Fixture | Updated mock host harness to receive and verify client's authenticated registration Ping instead of `[0xff]`. |
| `clients/rust/erd-app/tests/cli_mcp_contract.rs` | Assigned Test Fixture | Updated mock host ack fixture to advertise `AUTHENTICATED_UDP_REGISTRATION`. |
| `.omo/pairing-20260910/reports/b-udp.md` | Deliverable Report | This document. |
| `/var/folders/zh/7cc25lt91b1_dj577306nwdh0000gn/T/ulw-20260910-111421.XXXXXX.md.pNqwGP19PH` | Append-only Notepad | Pinned RED test invocations, captured failure receipts, and recorded GREEN transitions before action. |

---

## 3. Verified RED Regression Evidence (Before Fix)

Prior to implementing changes in production code, tests were pinned and executed on the remote builder `indo@100.91.254.71` to observe genuine failures caused by the unpatched implementation.

### 3.1 Target 1 RED: Capability Bit 7 in `erd-proto`
- **Command**:
  ```bash
  cargo test --manifest-path clients/rust/Cargo.toml -p erd-proto --test protocol_v3 capabilities_authenticated_udp_registration_round_trip
  ```
- **Observed Result**: Exit code 101.
  ```text
  error[E0599]: no associated function or constant named `AUTHENTICATED_UDP_REGISTRATION` found for struct `erd_proto::Capabilities` in the current scope
     --> erd-proto/tests/protocol_v3.rs:858:30
      |
  858 |     let caps = Capabilities::AUTHENTICATED_UDP_REGISTRATION;
      |                              ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ associated function or constant not found in `erd_proto::Capabilities`
  ```

### 3.2 Target 2 RED: Host Capability Check
- **Command**:
  ```bash
  cargo test --manifest-path clients/rust/Cargo.toml -p erd-host --lib session::tests::test_udp_host_rejects_client_missing_authenticated_registration_capability
  ```
- **Observed Result**: Exit code 101.
  ```text
  thread 'session::tests::test_udp_host_rejects_client_missing_authenticated_registration_capability' panicked at erd-host/src/session.rs:4955:13:
  assertion `left != right` failed: host must NOT send HandshakeAck to client lacking authenticated UDP registration capability
    left: HandshakeAck
   right: HandshakeAck
  ```

### 3.3 Target 3 RED: Client Capability Check
- **Command**:
  ```bash
  cargo test --manifest-path clients/rust/Cargo.toml -p erd-app --test session_mock udp_client_rejects_host_missing_authenticated_registration_capability
  ```
- **Observed Result**: Exit code 101.
  ```text
  thread 'udp_client_rejects_host_missing_authenticated_registration_capability' panicked at erd-app/tests/session_mock.rs:584:5:
  client must fail before Ready when host lacks authenticated registration capability, got: Ok(ReadySession { ... capabilities: Capabilities(0) ... })
  ```

### 3.4 Target 4 RED: Host Rejection of Plaintext Probe and Attacker Hijacking
- **Command**:
  ```bash
  cargo test --manifest-path clients/rust/Cargo.toml -p erd-host --lib session::tests::test_udp_discover_peer_rejects_invalid_packets_and_fixes_endpoint
  ```
- **Observed Result**: Exit code 101.
  ```text
  thread 'session::tests::test_udp_discover_peer_rejects_invalid_packets_and_fixes_endpoint' panicked at erd-host/src/session.rs:4795:9:
  assertion `left == right` failed: plaintext probe must not register endpoint
    left: Some(127.0.0.1:40858)
   right: None
  ```
  *Evidence confirms that unpatched `discover_udp_peer` accepted `0xff` probes and mistakenly registered the attacker socket.*

### 3.5 Target 5 RED: Client Authenticated Registration Datagram
- **Command**:
  ```bash
  cargo test --manifest-path clients/rust/Cargo.toml -p erd-app --test session_mock udp_client_sends_authenticated_registration_and_preserves_nonce
  ```
- **Observed Result**: Exit code 101.
  ```text
  thread '<unnamed>' panicked at erd-app/tests/session_mock.rs:640:87:
  called `Result::unwrap()` on an `Err` value: Truncated
  ```
  *Evidence confirms that unpatched client transmitted 1-byte `[0xff]` probe instead of 40-byte sealed datagram.*

---

## 4. Hardened UDP Test Architecture

1. **Independent Cipher Instances**:
   - Tests instantiate separate cipher objects for each transmission direction and peer role:
     - `client_c2h`: Client-owned `ClientToHost` cipher used to seal client datagrams.
     - `host_c2h`: Host-owned `ClientToHost` cipher used inside `discover_udp_peer` to open inbound packets.
     - `host_h2c`: Host-owned `HostToClient` cipher used to seal outbound media frames.
     - `client_h2c`: Client-owned `HostToClient` cipher used to decrypt received media frames.
     - `prior_client_c2h`: Sealed with prior-session salt, verifying cross-session cryptographic rejection.
   - Nonce counters and internal AEAD states are strictly isolated across sender and receiver boundaries.
2. **Positive Packet Consumption Proofs**:
   - Every stage employs an explicit two-step proof:
     - **Before `discover_udp_peer`**: `server.udp_socket.peek_from(&mut peek_buf)` asserts the packet was delivered to the socket buffer, asserting on exact byte length and sender socket address.
     - **Execution**: `server.discover_udp_peer(...)` executes.
     - **After `discover_udp_peer`**: `server.udp_socket.recv_from(&mut check_buf)` asserts `ErrorKind::WouldBlock`, proving the packet was actually consumed and drained from the socket queue rather than ignored or skipped.
3. **Subsequent UDP Transmission and Nonce Preservation Proof**:
   - In `udp_client_sends_authenticated_registration_and_preserves_nonce`, after registration is accepted (consuming nonce counter 1), the client invokes `session.send_udp(PacketType::Ping, b"subsequent-session-packet")`.
   - The mock host receives the packet on its UDP socket and uses the SAME `host_c2h` cipher instance to open it.
   - It asserts `subseq_header.packet_type == PacketType::Ping`, `subseq_payload == b"subsequent-session-packet"`, and `subseq_header.sequence == 1`.
   - Had the client re-derived its cipher after registration, the client's packet would carry counter 1 and be rejected by the host's replay window. The success proves nonce counter monotonically advanced to 2.
4. **Immutability & Hijack Resistance Proof**:
   - Following valid registration (`udp_peer == Some(client_addr)`), an attacker transmits a validly encrypted registration packet from a rogue port.
   - The packet is proven present via `peek_from`, consumed by `discover_udp_peer`, and `udp_peer` is asserted to remain strictly `Some(client_addr)`.
   - A subsequent media frame encrypted with `host_h2c` is dispatched to `udp_peer.unwrap()`; the legitimate client receives and verifies it, while the attacker's nonblocking read returns `WouldBlock`.
5. **Zero Sleep Delays**:
   - Zero `thread::sleep` calls exist in the test code. Tests execute deterministically and finish in <200 ms.

---

## 5. Verified GREEN Regression Evidence (After Fix)

### 5.1 Focused Target Executions

1. **Target 1 GREEN (`protocol_v3::capabilities_authenticated_udp_registration_round_trip`)**:
   - Command: `cargo test --manifest-path clients/rust/Cargo.toml -p erd-proto --test protocol_v3 capabilities_authenticated_udp_registration_round_trip`
   - Result: `test capabilities_authenticated_udp_registration_round_trip ... ok`. 1 passed, 0 failed. Exit code: 0.

2. **Target 2 GREEN (`erd-host::test_udp_host_rejects_client_missing_authenticated_registration_capability`)**:
   - Command: `cargo test --manifest-path clients/rust/Cargo.toml -p erd-host --lib session::tests::test_udp_host_rejects_client_missing_authenticated_registration_capability`
   - Result: `test session::tests::test_udp_host_rejects_client_missing_authenticated_registration_capability ... ok`. 1 passed, 0 failed. Exit code: 0.

3. **Target 3 GREEN (`erd-app::udp_client_rejects_host_missing_authenticated_registration_capability`)**:
   - Command: `cargo test --manifest-path clients/rust/Cargo.toml -p erd-app --test session_mock udp_client_rejects_host_missing_authenticated_registration_capability`
   - Result: `test udp_client_rejects_host_missing_authenticated_registration_capability ... ok`. 1 passed, 0 failed. Exit code: 0.

4. **Target 4 GREEN (`erd-host::test_udp_discover_peer_rejects_invalid_packets_and_fixes_endpoint`)**:
   - Command: `cargo test --manifest-path clients/rust/Cargo.toml -p erd-host --lib session::tests::test_udp_discover_peer_rejects_invalid_packets_and_fixes_endpoint`
   - Result: `test session::tests::test_udp_discover_peer_rejects_invalid_packets_and_fixes_endpoint ... ok`. 1 passed, 0 failed. Exit code: 0.

5. **Target 5 GREEN (`erd-app::udp_client_sends_authenticated_registration_and_preserves_nonce`)**:
   - Command: `cargo test --manifest-path clients/rust/Cargo.toml -p erd-app --test session_mock udp_client_sends_authenticated_registration_and_preserves_nonce`
   - Result: `test udp_client_sends_authenticated_registration_and_preserves_nonce ... ok`. 1 passed, 0 failed. Exit code: 0.

---

## 6. Scoped Test Suites and Multi-Package Verification

All suites executed strictly on remote builder `indo@100.91.254.71` with required environment variables:
`export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig`
`export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:$LD_LIBRARY_PATH`

### 6.1 UDP Scoped Suite
- **Command**:
  ```bash
  cargo test --manifest-path clients/rust/Cargo.toml -p erd-host -p erd-app udp
  ```
- **Output**:
  ```text
  Running unittests src/lib.rs (erd_app)
  test session::cancellation_tests::udp_cancel_wakes_registered_receive ... ok
  test session::cancellation_tests::client_session_udp_receive_reuses_buffer_across_cancellations ... ok
  test result: ok. 2 passed; 0 failed; 0 ignored; finished in 0.01s

  Running tests/receiver_telemetry.rs (erd_app)
  test mixed_udp_kinds_share_packet_inference_but_tcp_stats_and_invalid_auth_do_not ... ok
  test authenticated_tls_udp_ingress_records_stats_and_assembly_without_changing_ping ... ok
  test result: ok. 2 passed; 0 failed; 0 ignored; finished in 0.01s

  Running tests/session_mock.rs (erd_app)
  test udp_client_sends_authenticated_registration_and_preserves_nonce ... ok
  test udp_client_rejects_host_missing_authenticated_registration_capability ... ok
  test result: ok. 2 passed; 0 failed; 0 ignored; finished in 0.01s

  Running unittests src/lib.rs (erd_host)
  test session::tests::test_udp_discover_peer_rejects_invalid_packets_and_fixes_endpoint ... ok
  test session::tests::test_udp_registration_rejects_prior_session_datagram ... ok
  test session::tests::test_udp_host_rejects_client_missing_authenticated_registration_capability ... ok
  test session::tests::disconnect_before_udp_unblocks_full_media_queue ... ok
  test result: ok. 4 passed; 0 failed; 0 ignored; finished in 0.23s
  ```
- **Summary**: 10 tests selected and passed, 0 failed, 0 ignored. Exit code: 0.

### 6.2 Full Protocol Suite (`erd-proto`)
- **Command**:
  ```bash
  cargo test --manifest-path clients/rust/Cargo.toml -p erd-proto
  ```
- **Summary**: 6 lib unittests + 3 framing_burst tests + 48 protocol_v3 tests + 5 timestamp_stats tests = **62 passed, 0 failed**. Exit code: 0.

### 6.3 Full Host Suite (`erd-host`)
- **Command**:
  ```bash
  cargo test --manifest-path clients/rust/Cargo.toml -p erd-host
  ```
- **Summary**: 94 lib unittests + 5 bin unittests + 6 linux_audio tests + 2 pairing_isolation tests + 3 doc tests = **110 passed, 0 failed**. Exit code: 0.

### 6.4 Full App Suite (`erd-app`)
- **Command**:
  ```bash
  cargo test --manifest-path clients/rust/Cargo.toml -p erd-app
  ```
- **Summary**: 125 lib unittests + 16 bin unittests + 3 agent_control_e2e tests + 5 cli_mcp_contract tests + 3 cli_receiver_telemetry tests + 3 client_copy_cost tests + 6 core_semantics tests + 8 media_reassembly tests + 5 receiver_telemetry tests + 9 session_mock tests = **183 passed, 0 failed**. Exit code: 0.

### 6.5 Full Net Suite (`erd-net`)
- **Command**:
  ```bash
  cargo test --manifest-path clients/rust/Cargo.toml -p erd-net
  ```
- **Summary**: 46 lib unittests + 18 discovery_metadata tests = **64 passed, 0 failed**. Exit code: 0.

### 6.6 Full Tauri-Shell Suite (`tauri-shell`)
- **Command**:
  ```bash
  cargo test --manifest-path clients/rust/Cargo.toml -p tauri-shell
  ```
- **Summary**: 58 passed, 0 failed, 1 ignored (explicit installed Tailscale check). Exit code: 0.

---

## 7. Binary Builds & Producer QA Live Surface Execution

### 7.1 Scoped Binary Compilation on Omarchy
1. `erd-client`:
   - Command: `cargo build --manifest-path clients/rust/Cargo.toml -p erd-app --bin erd-client`
   - Output Path: `clients/rust/target/debug/erd-client` (168,787,536 bytes)
   - Exit code: 0.
2. `erd-pairing-qa-host`:
   - Command: `cargo build --manifest-path .omo/pairing-20260910/qa/native-host/Cargo.toml --target-dir clients/rust/target`
   - Output Path: `clients/rust/target/debug/erd-pairing-qa-host` (116,072,872 bytes)
   - Exit code: 0.

### 7.2 Producer QA Live Surface Verification (`r3-live.mjs`)
*(Note: Executed as producer-side QA validation on Omarchy testbed; coordinator personal rerun is reserved for the phase lead.)*
- **Command**:
  ```bash
  export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig
  export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:$LD_LIBRARY_PATH
  export XDG_RUNTIME_DIR=/run/user/1000
  export WAYLAND_DISPLAY=wayland-1
  export ERD_OUTPUT=HDMI-A-2
  export HYPRLAND_INSTANCE_SIGNATURE=efb50993780079460b0cbed1363e2166a2de1d9f_1788965704_1690871543
  node .omo/pairing-20260910/qa/r3-live.mjs
  ```
- **Output Transcript**:
  ```text
  CLEANUP {"scenario":"baseline","hostPid":4111181,"clientPid":4111183,"closedSockets":0,"removedDirectory":"/tmp/erd-r3-qa-2stYcx","cleanupErrors":[]}
  R3_BASELINE {"adversarial":false,"ports":{"tcp":41341,"udp":33326},"gatePort":33326,"registrations":0,"roguePackets":0,"rogueBytes":0,"exit":{"code":0,"signal":null},"success":true,"clientOutput":"... Reached requested target frame count decoded_frames=1 ... stats={\"frames\":1,\"p50_us\":48475 ... \"udp_authenticated_datagrams\":24,\"udp_authenticated_bytes\":26456 ...}\n"}
  CLEANUP {"scenario":"adversarial","hostPid":4111227,"clientPid":4111229,"closedSockets":2,"removedDirectory":"/tmp/erd-r3-qa-NBhrqw","cleanupErrors":[]}
  R3_ADVERSARIAL {"adversarial":true,"ports":{"tcp":43179,"udp":43450},"gatePort":57462,"registrations":1,"roguePackets":0,"rogueBytes":0,"exit":{"code":0,"signal":null},"success":true,"clientOutput":"... Reached requested target frame count decoded_frames=1 ... stats={\"frames\":1,\"p50_us\":47650 ... \"udp_authenticated_datagrams\":24,\"udp_authenticated_bytes\":26456 ...}\n"}
  R3_SURFACE_PASS
  ```
- **Results**:
  - Baseline scenario: 1 full 3840x1600 frame decoded via real hardware/Wayland surface, target frames reached, 24 authenticated UDP datagrams received, exit code 0.
  - Adversarial scenario: rogue socket attempted injection prior to legitimate client registration. Rogue packet count was **0** and rogue byte count was **0**. Legitimate native client successfully registered and decoded 1 full frame (24 authenticated datagrams received), exit code 0.
  - Test runner verified `R3_SURFACE_PASS` with exit code 0.
  - Owned temporary directories, sockets, and process groups were cleaned up with 0 errors.

---

## 8. Quality Gate Checklist

1. **Strict Clippy**:
   - Command: `cargo clippy --manifest-path clients/rust/Cargo.toml -p erd-proto -p erd-host --no-deps -- -D warnings`
   - Result: 0 warnings, 0 errors. Exit code 0.
2. **Whole Target Check**:
   - Command: `cargo check --manifest-path clients/rust/Cargo.toml --all-targets`
   - Result: Finished in 4.13s with 0 errors. Exit code 0.
3. **Scope & Provenance Discipline**:
   - `serve_with_stop`, rewritten `serve` loop, and `test_serve_with_stop_exits_when_flag_set` are identified as unconfirmed concurrent host-management additions, preserved without deletion to avoid breaking `tauri-shell`'s concurrent `AppState::start_host`, and excluded from all R3 diff/commit claims.
   - `tauri-shell` Cargo/lib/page-harness changes were untouched.
   - Reverted unrelated `last_input_ack` edit in `erd-app/src/session.rs`.
   - `git diff` shows only assigned Phase B changes and test adaptations.
4. **Receipts & Deliverables**:
   - Scoped builds (`erd-client` and `erd-pairing-qa-host`) ready for coordinator live rerun.
   - Deliverable report `.omo/pairing-20260910/reports/b-udp.md` written and synced.
   - Append-only notepad `/var/folders/zh/7cc25lt91b1_dj577306nwdh0000gn/T/ulw-20260910-111421.XXXXXX.md.pNqwGP19PH` updated with all Phase B RED, GREEN, and hardening entries.
