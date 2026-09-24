# Relay implementation review

Date: 2026-09-23

## Criteria

1. Happy path bridge: PASS. `.omo/relay-20260923/relay-test.log` shows `bridge_forwards_tcp_and_udp_both_directions` ok and `cargo test -p maho-relay --lib` exit 0 (5 passed).
2. Auth and limits: PASS in the same log. `register_rejects_wrong_hmac`, `fifth_session_is_rejected`, `payload_over_2mib_is_rejected`, and `empty_relay_auth_secret_fails_config` are ok.
3. Regression: unfiltered `cargo test -p maho-proto --lib` passed in `.omo/relay-20260923/proto-lib-test.log` (19 passed, 0 failed, exit 0). Legacy pairing JSON passed in `.omo/relay-20260923/legacy-pairing.log` (`test_legacy_record_readability` ok, and that test now asserts `relay_url` and `relay_host_id` are none). `cargo test -p maho-host --lib` passed in `.omo/relay-20260923/host-lib-test.log` (133 passed, 0 failed, exit 0).
4. Wiring: PASS. `.omo/relay-20260923/host-help.txt` contains `--relay`. `.omo/relay-20260923/pairing-test.log` shows `pairing_round_trip_keeps_relay_url` ok.
5. UI: PASS. `.omo/relay-20260923/ui-test.log` shows `bun test src` 109 pass, 0 fail. DirectConnect has `#direct-relay`. SessionView renders `#session-via-relay` when `maho-relay-url` is set. ComputersPage persists that key.
6. This file is the review.

## Findings

### P1 — client relay fallback thread is not stopped

Fixed. `open_relay_fallback` waits on a stop channel instead of `pending`. `disconnect` and a replacement bridge send that stop, which drops the runtime and the bridge task. `cargo test -p maho-app --lib pairing_round_trip_keeps_relay_url` still exits 0 after the change.

### P2 — host relay client has no byte-level integration test

Fixed. `host_relay_writes_tcp_and_udp_payloads_to_local_sockets` starts a local relay, registers the host client, and checks that a TCP payload and a UDP payload arrive on the host's local sockets. Log: `.omo/relay-20260923/host-bridge-test.log`.

### P2 — empty-secret assertion is split

`empty_relay_auth_secret_fails_config` asserts `config_from_secret(None)` and `config_from_secret(Some(""))` fail. `config_from_env` is only required to fail when the variable is unset. That still covers the named parse failure.

Declined. The named test is ok in the relay log.

### Note — failing-first RED was incomplete for the server crate

The proto file was observed as tests-only before the implementation landed. The relay server tests were added with the implementation, so there is no captured missing-route RED for `maho-relay`. The first `cargo test -p maho-relay` failed to compile (`E0277` on test helpers) and the rerun was green. That is recorded here rather than treated as a product defect.

## Cleanup

No `maho-relay` or `cargo test` process was left running after the suites exited. The tests bound `127.0.0.1:0` and exited with the process.

## Not done, by the goal's own exclusion

Fly deploy and production host restarts were excluded because `fly auth whoami` has no token.

## P1 (found during live pairing, fixed) — client relay bridge kills the whole WS on first UDP frame

Live pairing on the Omarchy host exposed a defect the unit tests missed: the client bridge in `maho-app/src/relay.rs` bound its UDP socket without `connect`, then used `udp.send` (connected-only) on downlink 0x02 frames. The first UDP media frame returned Err, which broke the entire select loop, closed the WS, and surfaced to the client TLS as "unexpected EOF" right after "Handshake completed". The host then waited 30 s and logged heartbeat timeout.

Sequence proof (server debug logs): bridge established -> handshake completed -> CLIENT EOF 240 ms later -> client ws loop exited, with no sink-fail / rx-closed / idle / bad-frame diagnostics, so the client side closed first.

Fixed by learning the peer with `udp.recv_from` and replying with `udp.send_to(peer)`, dropping downlink frames until a peer is known. Re-verified live: relay E2E via wss://maho-relay.fly.dev with host_id omarchy-indo decoded 5/5 frames, packet_loss_ratio 0.0, 177 authenticated UDP datagrams, exit 0. Regression: `cargo test -p maho-relay --lib` 5 passed; pairing_round_trip ok. Server debug secret RUST_LOG unset after diagnosis.
