# MahoRD Relay Server — Implementation Plan

**Date:** 2026-09-23  
**Status:** Draft  
**Goal:** Enable MahoRD connections across arbitrary networks without Tailscale or LAN, by deploying a personal Rust relay server.

---

## 1. Problem

| Scenario | Current State |
|----------|--------------|
| Same LAN (192.168.x.x) | ✅ Works |
| Tailscale (100.x.x.x) | ✅ Works, but corporate VPN (GlobalProtect) kills Tailscale |
| Different networks, no Tailscale | ❌ No path exists |

Corporate VPN captures all traffic and tears down the Tailscale WireGuard tunnel. A relay server on port 443 (WSS) bypasses VPN/firewall restrictions because HTTPS is never blocked.

## 2. Architecture

```
┌──────────┐    WSS (443)    ┌───────────────┐    WSS (443)    ┌───────────┐
│  Client   │───────────────▶│ Relay Server  │◀───────────────│   Host    │
│ (MahoRD)  │   TLS/HTTPS    │ (Rust, VPS)   │   TLS/HTTPS    │(maho-host)│
└──────────┘                 └───────────────┘                └───────────┘
```

### Data flow

1. **Host registers** — `maho-host` opens a persistent WebSocket to the relay (`wss://relay.example.com/register`), sends a registration frame containing `host_id` and an auth token.
2. **Client connects** — `maho-app` connects to `wss://relay.example.com/connect/{host_id}` with a client auth token.
3. **Relay bridges** — the relay pairs the two WebSocket connections and forwards binary frames bidirectionally. It never inspects payload; existing ChaCha20-Poly1305 / AES-GCM encryption provides end-to-end security.
4. **Teardown** — either side closing the WebSocket tears down both halves.

### Security model

- The relay is a **dumb pipe** — it forwards encrypted bytes without access to plaintext.
- Existing TLS-PSK pairing keys authenticate both ends; the relay cannot impersonate either party.
- Registration auth token prevents unauthorized hosts from squatting on `host_id` values.
- WSS (TLS 1.3) protects the relay transport layer itself.

## 3. Components

### 3.1 Relay Server (new crate: `maho-relay`)

**Location:** `clients/rust/maho-relay/`  
**Estimated size:** ~500 lines  
**Dependencies:** `tokio`, `axum`, `tokio-tungstenite`, `dashmap`, `tracing`

#### Endpoints

| Path | Method | Purpose |
|------|--------|---------|
| `GET /health` | — | Health check (load balancer / monitoring) |
| `GET /register` | WebSocket upgrade | Host registration; persistent connection |
| `GET /connect/{host_id}` | WebSocket upgrade | Client connection request |

#### Registration protocol

```
Host → Relay:  { "type": "register", "host_id": "<uuid>", "token": "<hmac>" }
Relay → Host:  { "type": "registered", "host_id": "<uuid>" }
```

- `host_id` is the machine's pairing-stable identifier (e.g., hostname hash or explicit config).
- `token` is an HMAC-SHA256 of `host_id` with a shared relay secret, preventing squatting.
- The relay keeps a `DashMap<String, WebSocketSender>` mapping `host_id` → host connection.

#### Bridge protocol

```
Client → Relay:  { "type": "connect", "host_id": "<uuid>", "token": "<hmac>" }
Relay → Host:    { "type": "incoming", "session_id": "<uuid>" }
Host → Relay:    { "type": "accept", "session_id": "<uuid>" }
Relay → Client:  { "type": "connected" }
--- binary frames forwarded bidirectionally ---
```

After the `connected` handshake, all subsequent WebSocket frames are binary and forwarded verbatim. The relay does not parse, buffer, or modify them.

#### Multiplexing

A single host WebSocket carries both control messages (JSON text frames) and multiple bridged sessions (binary frames prefixed with a 16-byte session UUID). This avoids per-session WebSocket overhead on the host side.

#### Limits & safeguards

- Max concurrent sessions per host: 4
- Max registration idle timeout: 60s (heartbeat ping/pong)
- Max session duration: 24h
- Max frame size: 2 MB (accommodates keyframes)
- Rate limiting: 100 connect attempts per minute per source IP

### 3.2 Host changes (`maho-host`)

**Files:** `session.rs`, new `relay.rs` module  
**Estimated diff:** ~200 lines

- **Startup:** if `--relay <URL>` is provided (or `MAHO_RELAY_URL` env), spawn a background `tokio` task that maintains a persistent WebSocket to the relay with auto-reconnect (exponential backoff, max 30s).
- **Incoming session:** when the relay signals an incoming client, create a `RelayTransport` that wraps the multiplexed WebSocket channel as a `AsyncRead + AsyncWrite`, then feed it into the existing `run_session()` logic identically to a direct TCP connection.
- **LaunchAgent / service config:** add `--relay` to the plist / service args.
- **Registration heartbeat:** ping every 30s; reconnect on pong timeout.

### 3.3 Client changes (`maho-app`)

**Files:** `session.rs`, new `relay.rs` module  
**Estimated diff:** ~150 lines

- **Connection fallback chain:**
  1. Direct TCP to `lastEndpoint` (LAN / Tailscale)
  2. Direct TCP to each `endpointAliases`
  3. **Relay via `relayUrl`** (new)
- **Pairing record extension:** add optional `relayUrl` and `relayHostId` fields to `client-pairings.json`.
- **`RelayTransport`:** WebSocket client that connects to `wss://relay/connect/{host_id}`, completes the bridge handshake, then provides `AsyncRead + AsyncWrite` to the existing session logic.

### 3.4 Tauri desktop UI changes (`tauri-shell`)

**Estimated diff:** ~30 lines

- Direct connect dialog: add optional "Relay URL" field.
- Host card metadata: show "via relay" badge when connected through relay.
- Settings: persistent relay URL configuration.

## 4. Transport Mapping

MahoRD currently uses TCP (control/handshake) + UDP (media). Over a WebSocket relay:

| Original | Over Relay |
|----------|-----------|
| TCP control frames | WebSocket binary frames (tagged `0x01`) |
| UDP media datagrams | WebSocket binary frames (tagged `0x02`) |

Each relayed binary frame has a 17-byte header:

```
[session_id: 16 bytes] [channel: 1 byte] [payload: N bytes]
```

- `channel = 0x01`: TCP-equivalent (ordered, reliable — inherent in WebSocket)
- `channel = 0x02`: UDP-equivalent (media; ordering preserved by WebSocket, but loss is not emulated — acceptable for a relay path where reliability > minimal latency)

The host and client `RelayTransport` demultiplex channels internally and present the same `TcpStream`-like + `UdpSocket`-like interface to the session layer.

## 5. Deployment

### Option A: Fly.io (recommended for speed)

```bash
cd clients/rust/maho-relay
fly launch --name maho-relay --region nrt --internal-port 8080
# → wss://maho-relay.fly.dev

# Config
fly secrets set RELAY_AUTH_SECRET="<random-64-hex>"
```

- **Region:** `nrt` (Tokyo) — lowest latency to Seoul/Korea
- **Cost:** Free tier (1 shared CPU, 256MB) is sufficient for personal use
- **TLS:** Automatic via Fly.io edge (client connects to `wss://`, Fly terminates TLS, forwards plain WS to the app)

### Option B: Personal VPS

```bash
# Build static musl binary
cargo build --release --target x86_64-unknown-linux-musl -p maho-relay

# Deploy
scp target/x86_64-unknown-linux-musl/release/maho-relay user@vps:~/
ssh user@vps 'sudo systemctl enable --now maho-relay'

# TLS via Caddy / nginx reverse proxy on port 443
```

### Option C: Omarchy server (100.91.254.71)

Use the existing Linux testbed as the relay. Downside: only reachable via Tailscale, which defeats the purpose when VPN kills Tailscale. Only useful as a fallback when both ends have Tailscale active.

## 6. Implementation Phases

### Phase 1: Relay server (standalone)
- [ ] Create `maho-relay` crate with Cargo.toml
- [ ] Implement WebSocket registration endpoint
- [ ] Implement WebSocket connect/bridge endpoint
- [ ] Implement multiplexed session forwarding
- [ ] Add health check, rate limiting, auth token validation
- [ ] Unit tests for registration, bridging, teardown, limits
- [ ] Dockerfile / Fly.io config

### Phase 2: Host integration
- [ ] Add `relay.rs` module to `maho-host`
- [ ] Implement `--relay <URL>` CLI flag and env var
- [ ] Background WebSocket registration with auto-reconnect
- [ ] `RelayTransport` adapter implementing session trait
- [ ] Integration test: relay ↔ host registration

### Phase 3: Client integration
- [ ] Add `relay.rs` module to `maho-app`
- [ ] Implement relay fallback in connection chain
- [ ] `RelayTransport` client-side adapter
- [ ] Extend `client-pairings.json` schema with `relayUrl` / `relayHostId`
- [ ] Integration test: client → relay → host full session

### Phase 4: Deploy & verify
- [ ] Deploy relay server to Fly.io (or VPS)
- [ ] Configure maho-mac host with `--relay wss://maho-relay.fly.dev`
- [ ] Configure maho-win host with `--relay wss://maho-relay.fly.dev`
- [ ] End-to-end verification: connect from a different network with VPN active
- [ ] Measure added latency vs direct connection

### Phase 5: Desktop UI (optional)
- [ ] Relay URL field in direct connect dialog
- [ ] "via relay" connection badge
- [ ] Relay URL in settings persistence

## 7. Performance Expectations

| Metric | Direct (LAN) | Relay (Tokyo VPS) |
|--------|-------------|-------------------|
| Handshake RTT | ~46ms | ~100-150ms |
| Added frame latency | 0 | ~2-5ms |
| Throughput | ~13 Mbps+ | Limited by VPS bandwidth (typically 1-10 Gbps) |
| Packet loss | 0% | 0% (TCP-backed) |

The relay path trades minimal latency increase for universal reachability. For a remote desktop use case, the added 2-5ms is imperceptible.

## 8. Future Enhancements

- **STUN/TURN upgrade:** attempt UDP hole-punching first via the relay as signaling server; fall back to relay forwarding only when hole-punching fails. This would recover direct P2P latency in ~80% of NAT configurations.
- **Multiple relay regions:** deploy to multiple Fly.io regions and use anycast or client-side RTT probing to pick the nearest relay.
- **Relay discovery via DNS:** `_maho-relay._tcp.mahord.dev` SRV record pointing to the active relay, eliminating hardcoded URLs.
- **Bandwidth metering:** per-host transfer accounting for multi-user deployment.
