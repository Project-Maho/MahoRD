<p align="center">
  <img src="clients/rust/tauri-shell/icons/icon-1024.png" alt="MahoRD icon" width="112">
</p>

<h1 align="center">MahoRD</h1>

<p align="center">
  <strong>Agent-first remote desktop for automation.</strong><br>
  CLI with stdio MCP and HTTP API. Rust host with native capture/encoding, encrypted streaming.<br>
  Desktop and iOS clients. Performance tuned, not guaranteed.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/source_license-MIT-blue" alt="Source license: MIT"></a>
  <img src="https://img.shields.io/badge/status-early_development-orange" alt="Status: early development">
  <img src="https://img.shields.io/badge/core-Rust-dea584" alt="Core: Rust">
</p>

MahoRD connects a host daemon to a desktop or mobile client. The host captures
the screen, encodes video, and accepts input; the client decodes frames, plays
audio, and presents a computer library and in-session controls.

The project is under active development. Windows and Hyprland host streams have
been exercised with the macOS release CLI/API. This is not a blanket claim of
production readiness, complete platform support, or a guaranteed latency figure.

## Run with an agent

Start the host inside the desktop session you want to share:

```sh
maho-host --pin generate
```

Pair from the agent computer, approve at the host, and let the client save
the record. Replace `HOST` and `PIN` with the actual values:

```sh
mkdir -p "$HOME/.config/mahord"
maho-client --host HOST --pin PIN \
  --pairing-store "$HOME/.config/mahord/pairings.json" \
  --frames 10 --timeout-secs 60
jq -r '.[] | [.id, .name] | @tsv' "$HOME/.config/mahord/pairings.json"
```

Use the saved ID and the same private store to register MCP:

```sh
# Claude Code, project scope
claude mcp add --transport stdio --scope project mahord \
  -- /absolute/path/to/maho-client --host HOST --pairing-id PAIRING-ID \
    --pairing-store /absolute/path/to/pairings.json --mcp

# Codex
codex mcp add mahord \
  -- /absolute/path/to/maho-client --host HOST --pairing-id PAIRING-ID \
    --pairing-store /absolute/path/to/pairings.json --mcp
```

Install [`skills/mahord-remote-control/SKILL.md`](skills/mahord-remote-control/SKILL.md)
into your project's `.claude/skills/mahord-remote-control/` (Claude Code) or
`.agents/skills/mahord-remote-control/` (Codex). Clients without a skill
loader can receive that file as task context; tools must still be registered.

Ask the agent to **inspect a screenshot, move the pointer, inspect again, and
release input**. Eleven MCP tools cover screenshots, screen geometry, waiting
for a screen change, pointer movement/click/drag/scroll, keys, hotkeys, text,
and release. MCP uses
newline-delimited JSON-RPC on stdin/stdout; diagnostic logs go to stderr.
Closing stdin ends the session.

For HTTP automation, use `--agent-server 19735` instead of `--mcp`:

```sh
maho-client --host HOST --pairing-id PAIRING-ID \
  --pairing-store /absolute/path/to/pairings.json --agent-server 19735
```

The loopback API exposes `/api/v1/health`, `/api/v1/screen/info`,
`/api/v1/screen/screenshot`, `/api/v1/input/action`, and
`/api/v1/session/disconnect`. Agent modes have no default overall timeout;
`--timeout-secs` sets an explicit limit.

See the [complete setup guide](docs/agent-setup.md) for skill installation,
Claude Desktop JSON, Codex TOML, exact HTTP requests, pairing, and limitations.

## What is implemented

- **Desktop client:** Tauri application with discovered and paired computers,
  search, favorites, direct connection, session controls, and receiver statistics.
- **Video:** H.264/HEVC pipelines, hardware-backed host paths, bounded frame
  queues, keyframe recovery, and NV12 presentation through WebGL in the clients.
- **Audio and input:** host audio capture, client audio controls, keyboard,
  mouse, relative pointer input, held-input release, and clipboard integration.
  Availability and verification differ by platform.
- **Discovery and pairing:** Bonjour/DNS-SD on Apple platforms, mDNS on
  Linux/Windows, PIN bootstrap, and persisted pairing records.
- **Transport:** TLS-PSK control connections and authenticated UDP media with
  replay protection.
- **Relay:** a standalone WebSocket relay that forwards pre-encrypted frames when
  no direct route exists, with HMAC-authenticated host registration and automatic
  client fallback.
- **Windows secure desktop:** the `MahoRDHost` LocalSystem service spawns a
  console session worker so the login/lock screen and the UAC secure desktop can
  be streamed and driven remotely.
- **Automation:** a headless client, loopback HTTP/WebSocket API, and stdio MCP
  interface for screen capture and input.
- **Mobile:** shared Rust lifecycle/input code and an iOS Tauri application with
  Keychain integration and VideoToolbox decoding. Android application packaging
  is not implemented.

## Platform status

| Platform | Host | Client / verification |
| --- | --- | --- |
| Windows | DXGI capture, Media Foundation H.264 encoding, input and audio paths | Host streaming verified, including the login screen and UAC secure desktop through the `MahoRDHost` service; desktop client code exists, but native Windows GUI QA is not complete |
| Linux / Hyprland | wlr-screencopy, FFmpeg encoding, uinput, PipeWire/PulseAudio | Host streaming verified; Linux desktop GUI QA is not complete |
| Other Wayland compositors | Requires `zwlr_screencopy_v1`; verify compositor support | Not covered by the Hyprland test results |
| KDE/GNOME Wayland and X11 | Capture backends are planned, not implemented | Do not infer host support from uinput support |
| macOS | ScreenCaptureKit, VideoToolbox, native input paths | Host daemon deployed and streaming verified; desktop app launch and release CLI/API streaming verified; native GUI session QA remains incomplete |
| iOS | Not a host | App built and installed on a physical iPhone; full on-device stream/audio/input QA remains incomplete |
| Android | Not a host | Shared Rust support code only; no runnable Android application yet |

See the [Linux support matrix](docs/linux-desktop-support.md) and
[iOS implementation report](docs/ios-device-implementation-20260909.md).

## Build from source

The active workspace is **`clients/rust/Cargo.toml`**. The root Cargo workspace
contains an earlier ScreenCaptureKit experiment, not the desktop application.

```sh
git clone https://github.com/Project-Maho/MahoRD.git
cd MahoRD
rustup toolchain install stable
```

### Dependencies

Use a current stable Rust toolchain and the native build tools for your platform.
The lockfile's transitive dependencies have not been certified against the
workspace's declared minimum Rust version.

| Component | Requirements |
| --- | --- |
| FFmpeg-backed host/client paths | FFmpeg **7.0.2** development headers and shared libraries, plus `pkg-config`; the Rust bindings are `ffmpeg-next` 8.1.0 |
| Native dependencies | A C/C++ toolchain, Make and Perl for vendored OpenSSL; Clang/libclang where required by bindings |
| Linux desktop client | Tauri v2's GTK/WebKitGTK 4.1 development dependencies |
| Linux media/input | ALSA, udev, Wayland, VAAPI/DRM development dependencies; PipeWire or PulseAudio tools for host audio |
| macOS | Command Line Tools and the appropriate Apple SDK |
| iOS | Tauri mobile tooling, Xcode, signing configuration, and a physical device |

Do not mix headers and runtime libraries from different FFmpeg versions. Point
`PKG_CONFIG_PATH` at the selected FFmpeg prefix; make its shared libraries
available to the loader when running the binaries. Installing a distribution's
latest FFmpeg package is not necessarily compatible with this lockfile.

For the project's pinned Linux build, see
[FFmpeg on Omarchy](docs/ffmpeg-omarchy-build.md). **That private build enables
`--enable-nonfree` and is not a redistributable binary recipe.**
General Tauri prerequisites are documented
[upstream](https://v2.tauri.app/start/prerequisites/).

### Desktop and headless binaries

With the native dependencies available:

```sh
cargo build --manifest-path clients/rust/Cargo.toml --locked --release \
  -p maho-host -p maho-app

cargo build --manifest-path clients/rust/Cargo.toml --locked --release \
  -p tauri-shell --features tauri/custom-protocol
```

This builds binaries, not a fully packaged or notarized installer. If
`CARGO_TARGET_DIR` is unset, native-target outputs are under
`clients/rust/target/release/`; Windows executables have an `.exe` suffix.

## Connect to a computer

### 1. Start the host

Run inside the desktop session you intend to share:

```sh
clients/rust/target/release/maho-host --pin generate
```

The host prints a bootstrap PIN and prompts for pairing approval. The bootstrap
window lasts five minutes; established pairings are used for later connections.
`--auto-approve` is intended for controlled automation, not required for normal use.

- **Linux:** grant the user access to `/dev/uinput`, keep the Wayland session
  environment available, and optionally select a display with `--output NAME`.
  See [permission setup](docs/linux-desktop-support.md).
- **Windows:** elevated applications require a host running with appropriate
  privileges for input injection.
- **macOS:** grant Screen Recording and Accessibility permissions.

Running the host without a terminal (launchd, systemd, the Windows service
manager): the daemon has no console, so read the PIN from wherever its
standard output lands. The line is `MahoRD bootstrap PIN: <8 digits>` and it
is regenerated on every daemon restart until the first pairing persists.

- **launchd / systemd:** the service definition redirects stdout to a log
  (`StandardOutPath`, `StandardOutput=`); read it, for example
  `grep "bootstrap PIN" ~/Library/Logs/maho-host.log` or
  `journalctl --user -u maho-host | grep "bootstrap PIN"`.
  `--bootstrap-pin <8 digits>` fixes the PIN for scripted onboarding.
- **Windows service (`MahoRDHost`, LocalSystem):** the session worker appends
  the PIN to `%ProgramData%\MahoRD\service.log`, readable by SYSTEM and
  Administrators only. Migrating an already-paired store instead of
  re-pairing is covered in
  [docs/windows-login-screen.md](docs/windows-login-screen.md).

`--list-paired` and `--revoke <ID>` manage established pairings.

### 2. Open the desktop client

```sh
clients/rust/target/release/tauri-shell
```

Choose a discovered computer or enter its address, supply the host's PIN, and
approve the request on the host. Discovery uses `_maho-rd._tcp.local.`. Multicast
filtering, guest Wi-Fi isolation, firewall rules, and subnet boundaries can
prevent discovery; a directly reachable address can still be used.

| Port | Purpose |
| --- | --- |
| TCP 19730 | Authenticated control connection |
| UDP 19731 | Encrypted media and related packets |
| UDP 5353 | Local mDNS discovery |
| TCP 19735 on loopback | Optional local automation API |
| Outbound WSS (443) to a relay | Optional fallback when no direct route exists |

### 3. Run a headless smoke test

Replace the address and example PIN with your host's values:

```sh
clients/rust/target/release/maho-client \
  --host 192.168.1.50 --pin 12345678 --frames 30 \
  --stats-json receiver-stats.json
```

The client exits successfully after decoding the requested frames. For an
existing pairing, use `--pairing-id` instead of a fresh PIN. Decode timings in
the statistics are not end-to-end display latency.

Discovery can also be inspected independently:

```sh
cargo run --manifest-path clients/rust/Cargo.toml --locked \
  -p maho-net --bin maho-discover -- --timeout-secs 3
```

## Reach a host on any network (relay)

Direct connections need a reachable address: the same LAN, a tailnet, or port
forwarding. When no direct route exists, both ends fall back to the WebSocket
relay. The host registers on it, and the client and host then exchange the same
pre-encrypted session frames over one outbound WSS connection; the relay only
forwards bytes and cannot read the stream.

Start the host with a relay and a stable host id. `MAHO_RELAY_URL` and
`MAHO_RELAY_HOST_ID` provide the same values as environment variables:

```sh
export RELAY_AUTH_SECRET="$(openssl rand -hex 32)"  # shared with the client
maho-host --relay wss://maho-relay.fly.dev --relay-host-id my-desktop
```

Pairings persist the relay URL and host id, so an established client retries the
relay automatically after a direct-connect failure. A headless client can also
request it explicitly:

```sh
maho-client --host HOST --pairing-id PAIRING-ID \
  --pairing-store /absolute/path/to/pairings.json \
  --relay-url wss://maho-relay.fly.dev --relay-host-id my-desktop
```

`RELAY_AUTH_SECRET` is required to register a host: it becomes an HMAC-SHA256
token over the host id, so a third party cannot squat it. A client without the
environment variable reads the same secret from `~/.maho-relay-secret`. Each
relay frame carries a 17-byte `[session id][channel]` header, and payloads over
2 MB are rejected. A public relay runs at `wss://maho-relay.fly.dev`; the
`maho-relay` crate can also be self-hosted. See the
[relay server plan](docs/relay-server-plan.md).

## Performance and verification limits

Performance is the second priority after reliable agent operation. The transport
uses bounded decode queues, keyframe recovery, native capture/encode paths, and
an MTU-safe 1200-byte UDP datagram budget to eliminate IP fragmentation across
encapsulated tunnels like Tailscale. The Windows encoder reselects the latest raw
frame after blocked control work; repeated static content retains its original
capture time while diagnostics separate content age from time spent waiting for
encoding.

Receiver sequence gaps are not automatically network loss. Optional bounded
endpoint traces distinguish send failures, authenticated arrivals, late
packets, assembly failures, and queue recovery. While MTU-safe packetization
verified zero fragmentation and 100% packet arrival in candidate testing,
historical sequence gaps are not retrospectively attributed to fragmentation alone;
do not infer that diagnostic instrumentation eliminates network loss.

Decode time excludes capture, encoding, network transit, assembly, and display
presentation. A static repeated frame's old capture timestamp is also not proof
that a newly captured frame waited that long in a queue. Latency measurements
from distinct workloads and trial conditions (such as a historical 417 ms active
trace versus a preliminary 14 ms window) are not comparable and do not constitute
a causal optimization proof. No screenshot or end-to-end latency guarantee is made,
streaming frame rate is not guaranteed at 60 fps, and GPU zero-copy is not claimed.
Linux hardware encoding uses VA-API on AMD Radeon graphics (not NVIDIA/NVENC).
Windows physical DXGI capture restores frame streaming across DPI-scaled displays,
though full stage attribution remains preliminary pending zero-overflow trace reruns.
Portrait display rotation is not supported for streaming.

See the [agent-first verification report](docs/agent-first-verification-20260909.md)
for initial test results and native MCP/API checks, the [follow-up performance verification report](docs/remaining-performance-verification-20260909.md)
for MTU, Clippy, and preliminary stage attribution findings, and
[deployment evidence](docs/release-deployment-20260909.md) for the earlier deployed release.
Native GUI/audio playback and full on-device iOS session QA remain incomplete;
CLI/API stream verification does not substitute for those checks.

## Architecture

```text
Host desktop
  capture -> encode -> authenticated UDP media
                              |
Client                        v
  reassemble -> bounded frame queue -> decode -> NV12 presentation
      |
      +-- input / clipboard / session control over TLS-PSK
```

| Crate | Responsibility |
| --- | --- |
| `maho-proto` | Wire types, framing, handshake and protocol limits |
| `maho-net` | TCP/UDP transport, replay protection, discovery, signaling and STUN |
| `maho-relay` | Standalone WebSocket relay: HMAC host registration and pre-encrypted frame forwarding |
| `maho-decode` | FFmpeg and iOS VideoToolbox decoding |
| `maho-render` | Presentation and audio infrastructure |
| `maho-app` | Sessions, pairing, frame queues, statistics, CLI and automation |
| `maho-host` | Platform capture, encoding, audio and input injection |
| `maho-mobile` | Shared mobile input, lifecycle and storage abstractions |
| `tauri-shell` | Desktop application |
| `ios-shell` | iOS application and native integration |

## Build and test

CI enforces these gates for every change under `clients/rust/`
([workflow](.github/workflows/rust-client.yml)):

```sh
cd clients/rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --locked --workspace --exclude maho-ios

cd tauri-shell
bun install --frozen-lockfile
bun test src
bun test tests
bunx tsc -b --noEmit
```

Frontend tests run with `bun test`; `node --test` is not part of the workflow.
The workflow additionally builds and tests the workspace on macOS, Ubuntu, and
Windows. Earlier live streaming evidence — the macOS release CLI decoding frames
from Linux and Windows hosts with screenshot, input, and disconnect API
verification — is recorded in
[deployment evidence](docs/release-deployment-20260909.md).

## License

MahoRD's original source is licensed under the [MIT License](LICENSE).
Dependencies retain their own licenses.

**The currently documented nonfree Linux FFmpeg build must not be redistributed.**
Publishing this source repository does not approve redistributing existing
application bundles, codecs, SDKs, or CI artifacts.

Read [third-party notices and binary distribution requirements](THIRD_PARTY_NOTICES.md)
and the [locked dependency license inventory](docs/dependency-licenses.md).
