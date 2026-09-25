# Rust Clients Knowledge Base

<!-- Score: 22 | Domain: Cross-platform Rust workspace root, shared crates, and Tauri shell -->

## OVERVIEW
Cargo workspace managing cross-platform MahoRD client and host implementations, including protocol codecs, network transports, decoders, renderers, and the Tauri desktop shell.

## STRUCTURE
```
clients/rust/
├── Cargo.toml            # Workspace manifest declaring member crates
├── maho-proto/            # Wire protocol v3 serialization, packet definitions, crypto handshakes
├── maho-net/              # Tokio network transport (TLS-PSK TCP, UDP-GCM, STUN, signaling)
├── maho-relay/            # WebSocket relay server (HMAC host registration, frame forwarding)
├── maho-decode/           # Video and audio decoding engines (FFmpeg / hardware)
├── maho-render/           # Cross-platform GPU rendering (wgpu / Metal / DirectX / Vulkan)
├── maho-app/              # Client session, pairing store, headless CLI, MCP & HTTP automation
├── maho-host/             # Cross-platform host streaming daemon (macOS, Windows, Linux)
├── maho-mobile/           # Shared mobile input, lifecycle and storage abstractions
├── tauri-shell/           # Tauri v2 desktop client application GUI (React/Vite under `src/`)
└── ios-shell/             # iOS Tauri application (VideoToolbox, Keychain)
```

## WHERE TO LOOK
| Task | Location | Key Symbol / File |
|------|----------|-------------------|
| Wire Protocol & Codecs | `clients/rust/maho-proto` | `WireCodec`, `PacketHeader`, `CryptoSession` |
| Network Transport & NAT | `clients/rust/maho-net` | `TlsPskStream`, `DatagramCipher`, `StunClient` |
| Relay & Cross-Network | `clients/rust/maho-relay` | `main.rs`, `lib.rs` (relay token / HMAC) |
| Client Session & Automation | `clients/rust/maho-app` | `ClientSession`, `mcp_server.rs`, `agent_server.rs`, `relay.rs` |
| Video Decompression | `clients/rust/maho-decode` | `VideoDecoder`, `FfmpegDecoder` |
| GPU Surface Rendering | `clients/rust/maho-render` | `Renderer`, `WgpuRenderer` |
| Host Capture & Streaming | `clients/rust/maho-host` | `HostServer`, `ScreenCapture`, `VideoEncoder` |
| Client Desktop Application | `clients/rust/tauri-shell` | `src-tauri/src/lib.rs`, `src/lib/`, `src/features/` |

## CONVENTIONS
- Build and test commands run with `--manifest-path clients/rust/Cargo.toml`.
- Library crates (`maho-proto`, `maho-net`, `maho-decode`) use `thiserror` for typed errors; application binaries use `anyhow`.
- Zero-copy packet manipulation is enforced via `bytes::Bytes` and `bytes::BytesMut`.
- Binary network serialization strictly uses Big-Endian / Network Byte Order.

## ANTI-PATTERNS
- Do not allocate buffers per packet on high-frequency video/audio paths; reuse memory or slice `bytes::Bytes`.
- Never block Tokio asynchronous executor threads with hardware video encoding, decoding, or OS input injection.
- Unsafe FFI code must be strictly isolated to platform-specific modules with explicit safety contracts.
