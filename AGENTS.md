# PROJECT KNOWLEDGE BASE

**Generated:** 2026-09-03 06:55:00Z
**Branch:** main

## OVERVIEW
Ultra-low latency Remote Desktop system built 100% in Rust with a Tauri v2 desktop client (`tauri-shell`), cross-platform host daemon (`maho-host`), and modular protocol/decode/render crates.

## STRUCTURE
```
.
└── clients/rust/     # Cross-platform Rust workspace
    ├── maho-proto/    # Pure v3 wire codec, packet envelopes, ChaCha20-Poly1305 handshakes
    ├── maho-net/      # Tokio async networking (TLS-PSK TCP, UDP-GCM, STUN, signaling)
    ├── maho-relay/    # WebSocket relay server: HMAC host registration, pre-encrypted frame forwarding
    ├── maho-decode/   # Video/audio decoding pipeline (FFmpeg / hardware)
    ├── maho-render/   # GPU renderer (wgpu / Metal / Vulkan / DirectX)
    ├── maho-app/      # Client session coordinator, pairing store, input & latency tracking
    ├── maho-host/     # Multi-platform host daemon (DXGI, Hyprland, SCK capture; MF, VAAPI, VT encode)
    ├── maho-mobile/   # Shared mobile input, lifecycle and storage abstractions
    ├── tauri-shell/   # Tauri v2 desktop GUI client (React/Vite under `src/`)
    └── ios-shell/     # iOS Tauri application (VideoToolbox, Keychain)
```

## WHERE TO LOOK
| Task | Location | Notes |
|------|----------|-------|
| Rust Multiplatform Host | `clients/rust/maho-host/` | `session.rs`, `capture_macos.rs`, `capture_windows.rs`, `capture_linux.rs` |
| Rust Protocol & Framing | `clients/rust/maho-proto/` | `packet.rs`, `framing.rs`, `handshake.rs`, `control.rs` |
| Rust Async Network Layer | `clients/rust/maho-net/` | `tls_psk.rs`, `udp_gcm.rs`, `stun.rs`, `signaling.rs` |
| Rust Relay Server | `clients/rust/maho-relay/` | `main.rs` (HTTP health + WS bridge), `lib.rs` (token/HMAC) |
| Rust Client Session & Pairing | `clients/rust/maho-app/` | `session.rs`, `pairing.rs`, `input.rs`, `relay.rs` |
| Rust Tauri Desktop Client | `clients/rust/tauri-shell/` | `src-tauri/src/lib.rs`, `src/lib/`, `src/features/`, `tests/` |

## CODE MAP
| Symbol | Type | Location | Role |
|--------|------|----------|------|
| `HostServer` | struct | `clients/rust/maho-host/src/session.rs` | Rust cross-platform streaming daemon orchestrator |
| `ClientSession` | struct | `clients/rust/maho-app/src/session.rs` | Rust client session manager |
| `WireCodec` | trait | `clients/rust/maho-proto/src/lib.rs` | Rust wire protocol serialization/deserialization contract |
| `DatagramCipher` | struct | `clients/rust/maho-net/src/udp_gcm.rs` | Direction-separated AES-GCM packet encryptor/decryptor |
| `TlsPskStream` | struct | `clients/rust/maho-net/src/tls_psk.rs` | Authenticated TCP control stream with replay defense |
| `HevcDecoder` | struct | `clients/rust/maho-decode/src/lib.rs` | Hardware/FFmpeg video decoder |

## CONVENTIONS
- Pure Rust architecture: Swift / Xcode projects are completely retired.
- Workspace commands use `--manifest-path clients/rust/Cargo.toml`.
- Strict byte-endian consistency on wire; no blocking calls on Tokio.
- UI mutations and state are managed via Tauri v2 commands and events.

## COMMANDS
```bash
# Rust Workspace Build & Test
cargo build --manifest-path clients/rust/Cargo.toml
cargo test --manifest-path clients/rust/Cargo.toml

# Run Tauri Desktop Client
cargo run --manifest-path clients/rust/Cargo.toml -p tauri-shell

# Headless Client E2E Test
cargo run --manifest-path clients/rust/Cargo.toml -p maho-app --bin maho-client -- --host <IP> --frames 10
```

## REMOTE BUILD & VERIFICATION (Omarchy Linux)
Linux desktop (Wayland/wlroots, X11, Hyprland) and FFmpeg-dependent components compile natively on the Omarchy remote builder without Mac CPU/memory pressure:
- **Remote Host**: `indo@100.91.254.71` (Tailscale SSH, Ryzen 5 5600X, Arch Linux).
- **Remote Workspace**: `/home/indo/projects/EclipticRD-Rewrite` (local NVMe).
- **FFmpeg 7 Pre-built**: `/home/indo/maho-ffmpeg7` (required by `ffmpeg-next` 8.1.0 to avoid FFmpeg 8 non-exhaustive pattern errors).

### Fast Sync & Remote Check Pattern
```bash
# 1) Sync modified sources (excludes target & node_modules)
rsync -az \
  --exclude 'target' \
  --exclude 'target/**' \
  --exclude 'target-tauri' \
  --exclude 'node_modules' \
  --exclude '*.tar.gz' \
  --exclude '.cache' \
  --exclude '*.log' \
  ./ indo@100.91.254.71:~/projects/EclipticRD-Rewrite/

# 2) Execute remote check / test with FFmpeg 7 PKG_CONFIG_PATH
ssh indo@100.91.254.71 "cd ~/projects/EclipticRD-Rewrite && bash -lc 'PKG_CONFIG_PATH=/home/indo/maho-ffmpeg7/lib/pkgconfig:\$PKG_CONFIG_PATH cargo check --manifest-path clients/rust/Cargo.toml'"
ssh indo@100.91.254.71 "cd ~/projects/EclipticRD-Rewrite && bash -lc 'PKG_CONFIG_PATH=/home/indo/maho-ffmpeg7/lib/pkgconfig:\$PKG_CONFIG_PATH cargo test --manifest-path clients/rust/Cargo.toml'"
```

### Resuming omo directly on Omarchy
```bash
ssh indo@100.91.254.71
cd ~/projects/EclipticRD-Rewrite
omo --continue
```
