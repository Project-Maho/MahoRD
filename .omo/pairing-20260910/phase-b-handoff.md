# Phase B handoff: authenticated UDP registration

Prepared by the coordinator from current sources while phase A generation 5 is running. This is a handoff, not a phase-B execution or completion claim. Start the phase-B workflow only after the corrected phase-A evidence is accepted.

## Source-backed boundaries

- `erd-host/src/session.rs`, `HostServer::discover_udp_peer`: a plaintext `ff` packet sets `udp_peer`; other packets also set it even when `open_datagram` fails. The authenticated session loop calls this method before starting the sender.
- The sender thread captures `peer` by value. Keep that immutable endpoint contract rather than adding endpoint migration or a shared lock.
- `erd-app/src/session.rs`, `ClientSession::begin_handshake`: derives the directional ciphers, connects UDP, sends `[0xff]`, stores the ciphers, then marks the session Ready.
- `erd-proto/src/handshake.rs`: capabilities currently occupy bits 0 through 6; `Capabilities::all()` explicitly combines them. Add the authenticated-registration capability at bit 7 and preserve the v3 codec/layout.
- `erd-app/src/session.rs`, `SessionConfig::direct`: explicitly advertises stream configuration and text clipboard capabilities.
- **The actual CLI is another required call site:** `erd-app/src/bin/erd_client.rs:383` constructs `SessionConfig` directly with its own explicit capability mask. Updating only `SessionConfig::direct` would leave the CLI incompatible.
- Structural searches found no direct `.capabilities = ...` assignment in desktop, iOS, or app source. The app's own tests also construct `SessionConfig`; update only fixtures representing current compatible peers, preserving intentionally unsupported-peer cases.
- `erd-net/src/udp_gcm.rs`: `seal_datagram` authenticates the 12-byte encoded header as AAD. `open_datagram` returns `(PacketHeader, Vec<u8>)`; it does not know the registration message contract. Successful AEAD verification alone is insufficient to select an endpoint.

## Frozen contract

Read `.omo/pairing-20260910/contracts.md`; its opening lead amendments override the later illustrative pseudocode.

1. Reuse `PacketType::Ping` with empty payload and the existing header/seal/open APIs. Validate registration header and payload after authentication. Do not invent a second framing protocol or change byte order.
2. Reuse the current client-to-host cipher instance for registration and subsequent sends. Registration consumes its nonce; do not rederive/reset that cipher after sending.
3. Both handshake directions advertise authenticated UDP registration. A missing capability fails explicitly before Ready, and the host rejects an unsupported peer before starting media. No plaintext fallback.
4. Plaintext, wrong-key, malformed, tampered, prior-session, and non-registration messages cannot select an endpoint. After valid registration, forged or alternate-source traffic cannot move it.
5. Preserve TCP admission/consent checks, source-address restrictions, fairness, and teardown. Do not introduce an unbounded invalid-datagram drain, new sender polling, or unrelated capture/encode changes.

## Phase-scoped workflow

Use the currently working `quotio/gemini-3.8-flash-high` invocation, without changing model settings.

Producer node: one coherent R3 change plus its failing-first tests and proof. Production scope is `erd-proto/src/handshake.rs`, `erd-app/src/session.rs`, `erd-app/src/bin/erd_client.rs`, `erd-host/src/session.rs`, and `erd-net/src/udp_gcm.rs` only if the existing API demonstrably cannot express the contract. Related protocol/app/host/net test fixtures may change where mandatory capability negotiation requires it. No identity/discovery/UI/lifecycle implementation or unrelated cleanup. Deliver `.omo/pairing-20260910/reports/b-udp.md`.

Verifier node: depends on the producer, reads the full delta, executes the actual relevant commands, and writes `.omo/pairing-20260910/reports/b-verify.md`. It must distinguish old fixture adaptation from unsupported-peer coverage and examine the real CLI's capability mask. No production edits.

Lead then reruns the existing live scenario and reviews the result before accepting the phase and committing the verified increment.

## Required proof

Before each new RED action, the producer pins its exact test names and invocations in the append-only notepad. The contract's proposed test names are not execution evidence.

Run compilation/tests only on `indo@100.91.254.71`, from `/home/indo/projects/erd-pairing-20260910`, with:

```text
PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig
LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib:$LD_LIBRARY_PATH
```

Required target:

```bash
cargo test --manifest-path clients/rust/Cargo.toml -p erd-host -p erd-app udp
```

Also run the exact new capability/registration tests and affected protocol/net suites. Count actual selected tests, not zero-test exits. Use real UDP sockets and an observable consume/completion boundary to prove invalid packets were processed before valid registration. A timeout alone is not proof of rejection. Confirm a controlled frame reaches the legitimate socket, and prove missing capabilities on both handshake sides are refused before Ready/media start.

Existing coordinator real-surface RED is already captured:

- `.omo/pairing-20260910/evidence/r3-live-red.json`
- `.omo/pairing-20260910/evidence/r3-live-scenario.md`
- `.omo/pairing-20260910/qa/r3-live.mjs`
- `.omo/pairing-20260910/qa/native-host/`

The baseline decoded one actual 3840x1600 HEVC frame. With a three-byte malformed sender injected before legitimate registration, the wrong socket received 517 packets/492514 bytes while the real CLI decoded zero frames. This proves the endpoint defect, not plaintext disclosure.

After the fix, rebuild the actual CLI and QA wrapper on Omarchy and run the same controller with `XDG_RUNTIME_DIR=/run/user/1000`, `WAYLAND_DISPLAY=wayland-1`, and `ERD_OUTPUT=HDMI-A-2` after verifying that output still exists. Expected result: baseline and adversarial native clients decode a frame, rogue packet count is zero, controller prints `R3_SURFACE_PASS`, exit 0, and cleanup receipts are clean. Keep RED and GREEN artifacts separate.

The prior run had unavailable CUDA/NVIDIA driver access but a successful real-frame fallback baseline. Do not attribute R3 to the encoder or claim hardware performance from this scenario.
