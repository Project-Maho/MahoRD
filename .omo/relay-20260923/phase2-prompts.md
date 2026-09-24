# Phase 2 prompts (not started)

Host and client stay serialized on Cargo.lock. Do not start until maho-relay tests are green.

## host-relay
category: unspecified-high
reason: integrates into the blocking accept loop and TLS session without a local pattern
write: clients/rust/maho-host/src/relay.rs, src/lib.rs, src/main.rs, src/session.rs, Cargo.toml
preserve dirty hunks in session.rs (audio queue drain) and main.rs (session_worker log) and Cargo.toml (objc2 audio features)
do not edit capture_macos.rs

Integration: handle_connection takes TlsPskStream<TcpStream> and calls set_read_timeout. Do not genericize it.
On relay Accept, loopback a TcpStream into accept_stream_until + handle_connection.
UDP: inject 0x02 datagrams toward the host udp_socket from a local peer so discover_udp_peer works.
CLI: --relay URL, env MAHO_RELAY_URL, secret env RELAY_AUTH_SECRET, host id flag or hostname.
Background thread with its own tokio runtime. Exponential reconnect, max 30s. Ping every 30s.
Do not commit.

## client-relay
depends on host-relay for Cargo.lock
write: clients/rust/maho-app/src/relay.rs, session.rs, pairing.rs, lib.rs, Cargo.toml
PairingRecord camelCase optional relayUrl and relayHostId, skip if none.
Legacy JSON without those fields must still deserialize.
Fallback: direct last_endpoint, then endpoint_aliases, then relay.
Client dials 127.0.0.1 bridge ports so existing connect_with_psk and udp.connect stay unchanged.
