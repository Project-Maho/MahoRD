# MahoRD full code review — 2026-09-14

**Repository:** `/Volumes/T9-Mac/project/EclipticRD-Rewrite` · branch `main` · HEAD `69a29ac` (+12 uncommitted files)
**Scope:** ~64k lines of Rust across 9 crates, ~9.5k lines of TypeScript/React, build/CI config, test suites, and the uncommitted working tree.
**Method:** 17 parallel adversarial review lanes (mass-ulw DAG), each producing a findings file under `.omo/code-review-20260914/findings/`. **131 findings: 10 P0, 56 P1, 55 P2, 10 P3.** Every P0 and P1 was re-opened by the lead at its cited `path:line` and byte-compared against the source before publication — 66/66 CONFIRMED, zero dropped.

---

## Baseline: what actually builds and passes right now

Captured from real runs on this workstation, not inferred:

| Command | Exit | Result |
|---|---|---|
| `cargo check --manifest-path clients/rust/Cargo.toml --workspace --all-targets` | **0** | clean |
| `bun test src` (clients/rust/tauri-shell) | **0** | 95 pass, 0 fail, 640 assertions, 10 files |
| `cargo test -p maho-net -p maho-proto` | **101** | **45 pass, 5 FAILED** |

**The workspace compiles but its tests are RED on `main`.** See P0-1 — this is the single most important finding in this review, and it is not a test-fixture problem.

---

## P0 — Critical

### P0-1. The rebrand silently changed every cryptographic key derivation (protocol-breaking)

**Location:** `clients/rust/maho-net/src/udp_gcm.rs:28-29,130`, `signaling.rs:18,93,95`, `tls_psk.rs:35,634`
**Introduced by:** commit `60b02ca` *"refactor(rebrand)!: rename brand EclipticRD/erd-\* to MahoRD/maho-\*"*

The mechanical rename rewrote HKDF **salts and info labels**, which are key-derivation *inputs*, not identifiers:

| Before (`60b02ca^`) | After (HEAD) |
|---|---|
| `b"erd/udp-c2h/v3"` / `b"erd/udp-h2c/v3"` | `b"maho/udp-c2h/v3"` / `b"maho/udp-h2c/v3"` |
| `b"erd/udp-ikm/v3"` | `b"maho/udp-ikm/v3"` |
| `b"erd/signaling/v3"`, `b"erd/topic"`, `b"erd/payload-key"` | `b"maho/signaling/v3"`, `b"maho/topic"`, `b"maho/payload-key"` |
| `b"erd/bootstrap/v3"`, `b"erd/tls-psk"` | `b"maho/bootstrap/v3"`, `b"maho/tls-psk"` |

**Impact.** Every session key, every direction-separated nonce prefix, and the bootstrap PSK changed value. Consequences, in order of severity:

1. **maho-\* builds are cryptographically incompatible with erd-\* builds.** A client and host across this commit cannot complete a handshake — this is the mechanism behind the `unknown psk identity` / PIN-reprompt symptoms seen after deploying renamed daemons.
2. **The frozen golden vectors that seal the wire format are now failing**, so the tests designed to catch exactly this class of change are red instead of gating:
   - `signaling::tests::topic_is_stable_and_within_ntfy_limit` (`signaling.rs:273`) — actual `erd3-5cb37a6711bb98f44cf615c77173`, expected `erd3-a1ac9ca40211ed7a122586e77c18`
   - `udp_gcm::tests::direction_derivation_matches_v3_vectors` (`udp_gcm.rs:306`)
   - `udp_gcm::tests::exact_wire_bytes_match_baseline` (`udp_gcm.rs:517`)
   - `tls_psk::tests::bootstrap_key_is_deterministic_per_pin` (`tls_psk.rs:828`)
   - `discovery::apple::tests::construct_full_name_produces_valid_escaped_fullname` (`apple.rs:1136`) — unrelated: the renamed service type `_maho-rd._tcp` contains no space, but the fixture name `"My Host"` does, and the assertion still expects an unescaped `My Host.` while the code correctly emits `My\032Host.`
3. **This violates the rebrand's own stated invariant.** Commit `3d36630` explicitly restored the `b"ERDTS1"` wire magic because "wire protocol v3 bytes stay byte-identical; only code identifiers are renamed." The KDF labels are equally wire-bytes and were missed.
4. **Both CI workflows run `cargo test --workspace`** (`rust-client.yml` on 3 OSes, `rust-matrix.yml` on 3 targets), so `main` is failing CI.

**Fix.** Decide explicitly, then make it explicit in code:
- **To preserve compatibility (recommended):** revert all eight KDF salt/info literals to their `erd/...` values and add a comment at each one stating that these bytes are protocol constants that must never be renamed. Keep the identifier renames.
- **To intentionally break v3:** bump the protocol version, regenerate every golden vector, and document the incompatibility in the release notes — but then the `ERDTS1` magic restoration in `3d36630` is inconsistent and should be revisited too.
Either way, fix the `apple.rs:1136` fixture separately (assert the escaped `My\032Host._maho-rd._tcp.local.`), and add a test that fails if any KDF label string changes.

### P0-2. Signaling topic is derived from the raw PIN, giving an offline brute-force oracle

**Location:** `clients/rust/maho-net/src/signaling.rs:93` (vs. `tls_psk.rs:622`)

```rust
let topic_seed = hkdf_sha256(pin.as_bytes(), SIGNALING_SALT, b"maho/topic", 32);
let topic = format!("erd3-{}", lower_hex(&topic_seed[..14]));
```

The TLS bootstrap path stretches the PIN with 600,000 PBKDF2 rounds. The signaling path does not: the rendezvous topic is a plain HKDF of the 8-digit PIN. Anyone who can see the topic — the ntfy operator, a compromised signaling service, a network observer of the URL — can enumerate all 10⁸ PINs with cheap HMAC and recover it, bypassing both the stretching cost and `BootstrapLockout` entirely. Recovering the PIN also yields the signaling payload key.

**Fix.** Derive both signaling outputs from the *stretched* PIN (`bootstrap_psk`'s PBKDF2 output), keeping the distinct topic/payload labels. Eliminating the oracle entirely requires a high-entropy rendezvous secret or a PAKE rather than a deterministic PIN-derived public topic.

### P0-3. Signaling buffers an unbounded HTTP body before any authentication

**Location:** `clients/rust/maho-net/src/signaling.rs:145`

```rust
if let Ok(Ok(response)) = timeout(remaining, self.http.get(&poll_url).send()).await {
    if let Ok(body) = response.bytes().await {
```

A time deadline is not a memory bound. A hostile or compromised ntfy endpoint streams an arbitrarily large body and the client retains all of it before checking a single GCM tag — no PIN, no valid ciphertext required.

**Fix.** Stream chunks under the deadline with a hard byte budget (e.g. 256 KiB response / 16 KiB per JSONL envelope) and error out when exceeded.

### P0-4. Oversized UDP datagram from any sender kills an authenticated Windows session

**Location:** `clients/rust/maho-host/src/session.rs:2775`

```rust
let mut buffer = [0_u8; 2_048];
for _ in 0..MAX_UDP_DISCOVERY_BURST {
    match self.udp_socket.recv_from(&mut buffer) {
        Ok((length, peer)) if peer.ip() == tcp_peer.ip() => {
```

On Winsock, a datagram larger than the buffer returns WSAEMSGSIZE (10040). The error arm tolerates only WSAECONNRESET/10054; everything else propagates as `SessionError` and tears down the live TCP/media session. The peer-IP and cipher checks live only in the `Ok` arm, so the attacker needs neither the client's address nor a PSK, and discovery keeps running after registration — so an established stream is killable too.

**Fix.** Treat WSAEMSGSIZE as a discarded datagram and `continue` within the bounded burst; never propagate malformed-datagram errors as session failures.

### P0-5. mDNS `DNSServiceProcessResult` return value ignored; hung-up socket spins at 100% CPU

**Location:** `clients/rust/maho-net/src/discovery/apple.rs:822-828` (advertiser: `:1072-1078`)

```rust
unsafe { DNSServiceProcessResult(handle) };
```

The error code is discarded at every call site, and `POLLHUP`/`POLLERR`/`POLLNVAL` are never checked anywhere in the file (verified: zero occurrences). When mDNSResponder restarts or closes a query, `poll()` reports the fd ready forever and the loop spins.

*Lead correction:* the browser loop at `:779` does poll with a 250 ms timeout and breaks `'event_loop` on a non-EINTR `poll` error, so this is not an unconditional 100%-CPU spin on *poll* failure — the defect is the ignored `ProcessResult` status combined with absent hangup handling. Severity retained at P0 for the advertiser loop (`:1059`, infinite `-1` timeout).

**Fix.** Check the `DNSServiceErrorType`; on any error deallocate the ref, drop it from `dispatch_handles`, and include `POLLHUP | POLLERR | POLLNVAL` in the revents test.

### P0-6. One failed resolve permanently bricks all future discovery snapshots

**Location:** `clients/rust/maho-net/src/discovery/apple.rs:683-691`

Per-service resolve errors are copied into the browser's shared `last_error`, which is never reset to `None`. Any third-party device on the LAN advertising a `_maho-rd._tcp` service that fails to resolve poisons `LanDiscovery::snapshot()` for the rest of the process lifetime.

**Fix.** Log and discard per-service resolve failures; reserve `last_error` for fatal browser-level errors, and clear it on recovery.

### P0-7. Linux encoder treats `EAGAIN` from `send_frame` as fatal

**Location:** `clients/rust/maho-host/src/encode_linux.rs:323-328`

```rust
self.open.encoder.send_frame(&hardware)?;
```

`EAGAIN` from `avcodec_send_frame` is the normal "drain the output queue first" signal — `encode_vt.rs:331` and `encode_windows.rs:1003` both handle it. Here `?` propagates it and kills the video pipeline. Compounded by a VAAPI `initial_pool_size` of 4, which makes queue-full frequent rather than rare.

**Fix.** On `EAGAIN`, call `receive_packets()`, retry `send_frame`, and merge the packet batches; raise the surface pool to ≥16.

### P0-8. Clipboard child process is waited on with no timeout, deadlocking the host

**Location:** `clients/rust/maho-host/src/clipboard_linux.rs:235-254`

```rust
let output = child.wait_with_output()?;
```

`wl-paste`/`xclip` block until the *selection owner* responds. A frozen or suspended owner application hangs this call forever, and it runs synchronously on the session thread.

**Fix.** Run the invocation under a bounded deadline (the `poll()`-based pattern in `LinuxAudioCapture::query_source` is the in-repo precedent) and kill/reap the child when it expires.

### P0-9. `stop_host` blocks forever while a client is connected

**Location:** `clients/rust/tauri-shell/src-tauri/src/lib.rs:784` (root cause `clients/rust/maho-host/src/session.rs:2094`)

```rust
pub fn stop_host(&self) -> Result<HostStatus, String> {
    self.host_runtime.stop_flag.store(true, Ordering::SeqCst);
    if let Ok(mut guard) = self.host_runtime.thread_handle.lock() {
        if let Some(handle) = guard.take() {
            let _ = handle.join();
```

The stop flag is read only by the outer `accept()` loop; `handle_connection` never sees it and runs as long as the peer answers heartbeats. The synchronous Tauri command therefore blocks its invocation thread indefinitely while streaming and input injection continue.

**Fix.** Thread the cancellation token into the connection loop so it interrupts socket reads and stops media workers; make the Tauri command async and join on a blocking worker.

### P0-10. MCP stdin reader has no line-length bound

**Location:** `clients/rust/maho-app/src/mcp_server.rs:179-188`

`reader.read_line(&mut line)` appends peer bytes without limit before any JSON parse. Unbounded growth in a long-lived process; `String::clear` also retains the peak allocation.

**Fix.** Use a capped incremental reader; on overflow, error or discard through the next newline.

---

### P0-11. Desktop UI deadlocks permanently when cleanup reports an error

**Location:** `clients/rust/tauri-shell/src/lib/connection.ts:208-216`

```ts
      if (errors.length) {
        state.cleanupError = errors.join('\n');
        state.phase = 'error';
      } else {
        state.busy = false;
```

When teardown or input release fails, the error branch sets `phase = 'error'` but **never clears `state.busy`**. `connect()` opens with `if (!nativeAvailable || state.busy) return false;`, so a single failed cleanup wedges the client: no reconnect, no recovery, restart required.

**Fix.** Clear `busy` and `host` on both branches; surface `cleanupError` as a dismissible banner rather than a latched state.

---

## P1 — High (56 findings, all verified)

Grouped by area. Full evidence, impact, and fix for each is in the lane file named in brackets.

### Session lifecycle and teardown
- **Host stop flag cannot cancel an active connection** — `maho-host/src/session.rs:2094` [host-session]
- **Media failure kills only the sender thread, leaving a live-but-frozen session** — `session.rs:2345`; the owner never learns, heartbeats keep answering, and the serial host cannot accept a replacement [host-session]
- **Disconnect never releases held keys/buttons** — `session.rs:2753`; `inject_windows.rs:175` Drop releases modifiers only, `inject_macos.rs:138` releases only on an explicit peer `Reset` that a dropped connection cannot send [host-session, host-io]
- **macOS injector has no `Drop` at all** — `inject_macos.rs:30-44` [host-io]
- **CLI decode loop ignores fatal TCP disconnection and hangs forever** — `maho_client.rs:731-746` [app-cli]

### Input correctness
- **Agent key-up never clears its modifier bits** — `agent_input.rs:511`; `ctrl↓ ctrl↑ c↓` still emits Ctrl+C [app-agent]
- **F2–F12 silently become the letter A** — `agent_input.rs:313` parses them, but `input.rs:76` maps only F1 (`0x70 => 0x7a`) and callers use `unwrap_or(0)`, which is the A keycode [app-agent]
- **Partial HTTP dispatch leaves an untracked key held** — `agent_server.rs:653` returns 500 mid-sequence with no rollback [app-agent]
- **Auto-release watchdog discards release errors and forgets the held state** — `agent_server.rs:184` [app-agent]
- **Server shutdown aborts the watchdog without releasing input** — `agent_server.rs:233` [app-agent]
- **macOS rate limiter throttles KeyUp/MouseUp/Reset** — `inject_macos.rs:99-104`; the events that *end* a hold are the ones dropped [host-io]
- **Windows injector ignores right-side modifiers and CapsLock** — `inject_windows.rs:145-155`; breaks AltGr [host-io]
- **Linux negative multi-monitor offsets clamp to zero** — `inject_linux.rs:62-65` [host-io]
- **Input injection mixes logical desktop coords with physical pixels under DPI scaling** — `windows_logic.rs:136` [host-capture]

### Capture and encode
- **Wayland `Flags::YInvert` dropped** because the `Flags` event precedes `BufferDone` — `capture_linux.rs:691` [host-capture]
- **`wl_buffer`/`wl_shm_pool` leak on every resolution change** — `capture_linux.rs:205` [host-capture]
- **Unsupported buffer format is fatal and violates protocol on `zwlr_screencopy_v1` < 3** — `capture_linux.rs:661` [host-capture]
- **High-DPI capture truncates the buffer and crashes the encoder** — `capture_windows.rs:149` [host-capture]
- **Multi-adapter enumeration resets the output index**, so secondary-GPU displays are uncapturable — `capture_windows.rs:259` [host-capture]
- **`FrameTimes::take` drops multi-packet PTS and halts the pipeline** — `native_pipeline.rs:356` [host-capture]
- **Odd width panics the Linux NV12 conversion** — `encode_linux.rs:275`; Windows validates evenness at `encode_windows.rs:95`, Linux does not [host-encode]
- **VideoToolbox parameter sets accumulate unboundedly**, emitting conflicting VPS/SPS/PPS each keyframe — `encode_vt.rs:421` [host-encode]
- **MFT dynamic stream change sets properties after `SetOutputType`**, silently dropping the low-latency constraints — `encode_windows.rs:546` [host-encode]
- **`IMFActivate` COM objects and task memory leak on MFT enumeration** — `encode_windows.rs:742` [host-encode]
- **Zero-length frame produces an invalid `FrameHeader`** the client must reject — `session.rs:2969` [host-encode]

### Decode
- **`copy_nv12` has no stride/plane-length guard** — `maho-decode/src/lib.rs:437`; the VideoToolbox path guards this exactly (`vt/mod.rs:123`), the FFmpeg path does not, so a peer-supplied odd-height frame can index past the plane and panic the decode thread [decode-render]

### Client reassembly and state
- **Stream-configuration results are silently discarded** by the TCP runtime — `maho-app/src/session.rs:1251` [app-session]
- **Clipboard echo suppression races the polling worker** both ways — `clipboard.rs:134` [app-session]
- **Host config response claims dimensions/fps that were never applied** — `maho-host/src/session.rs:2683` only submits a bitrate but echoes `req.desired` as `active` [host-session]

### Discovery
- **Advertiser index desync dispatches events to the wrong handles** — `apple.rs:1046-1078` [net-discovery]
- **Premature retraction erases active hosts in multi-interface setups** — `apple.rs:716-730` [net-discovery]
- **Unresponsive services retained forever, starving peers** — `apple.rs:620-625` [net-discovery]
- **`DiscoveryTracker` peer map has no capacity limit or TTL** — `tracker.rs:16-18` [net-discovery]

### Signaling
- **Stale/replayed candidate accepted** — `signaling.rs:137,199`; a 10-minute poll window with `candidate.role != own_role` as the only freshness test [net-crypto]

### Pairing and credentials
- **Credential migration misses the legacy `EclipticRD` directory and Keychain service** — `pairing.rs:360-376`; directly explains post-rebrand PIN re-prompts [app-cli]
- **One malformed record bricks the whole pairing store** — `pairing.rs:384-398` [app-cli]
- **Hardcoded developer IP/username and broken prefix matching in CLI reconnect** — `maho_client.rs:414-427` [app-cli]

### Desktop shell
- **The shell shows a pairing PIN but can never approve a new client** — `tauri-shell/src-tauri/src/lib.rs:194`; `auto_approve` defaults false with no approval path, so a clean install rejects every first-time client with `DeniedByHost` [tauri-rust]
- **Audio device list frozen after the first enumeration** — `lib.rs:332` [tauri-rust]

### Frontend hot path [frontend-core]
- **WebGL context-loss handler only calls `preventDefault()`** — `renderer.ts:281-288`; there is no `webglcontextrestored` listener, so the canvas never re-initializes and rendering freezes permanently after a GPU reset
- **NV12 parse framing mismatch on odd heights** — `renderer.ts:98-107`; `uvHeight` uses `ceil` while the Rust IPC packer truncates with `floor` (`src-tauri/src/lib.rs:1418`), so the planes disagree and cursor metadata bleeds into pixel data
- **UV texture upload ignores row stride on odd widths** — `renderer.ts:289-290`

### iOS / mobile [mobile-ios]
- **No legacy Keychain migration after the rebrand** — `maho-app/src/pairing.rs:338` queries only `com.projectmaho.mahord.pairing`; the file-based store has an explicit migration branch, Keychain has none, so every iOS user loses all pairings on upgrade and must re-enter PINs
- **Stale teardown generation overwrites `done_generation`** — `ios-shell/src/state.rs:274`; a late worker from an older generation claims the teardown slot and breaks idempotency for the current one
- **Failed `supervisor_worker` spawn leaks the TCP runtime and leaves the session in a stale `Ready` state** — `ios-shell/src/state.rs:1073`

### Frontend (React/TS) [frontend-ui]
- **Canvas dimensions hardcoded to `width={3840} height={1600}` in JSX** — `SessionCanvas.tsx:188`; every re-render resets the WebGL drawing buffer, corrupting mouse mapping and remote-cursor projection whenever the real stream is not exactly 3840×1600
- **Triplicated height-priority styling** — `index.html:2`, `globals.css:79`, and per-component inline styles all set the same rules; the global `user-select: none` this adds kills text selection across the whole shell
- **Forwarded keys never call `e.preventDefault()`** — `useRemoteInput.ts:390`; Tab moves focus out of the session and some keys trigger webview navigation
- **Unconditional `if (e.repeat) return`** — `useRemoteInput.ts:401`; held-key typematic repeat never reaches the host, so holding a key types one character
- **No pointer capture; mousemove is bound to the viewport** — `useRemoteInput.ts:462-466`; a drag that leaves the canvas loses its move events

### Working-tree diff [tests-diff]
- **Windows input injection and cursor query mix logical and physical metrics** — `maho-host/src/session.rs:2277`, `:1337`; scaled displays mismap
- **`lib.rs:27,31` declares `service_windows` and `windows_session`, both untracked files** — the diff is not self-contained and would not build from a clean checkout of the tracked tree

### Build and CI
- **No clippy, no rustfmt, no frontend test, no `tsc` in any workflow** — `.github/workflows/rust-client.yml:18`; verified independently (`grep` for clippy/fmt/bun test/tsc across `.github/workflows` returns nothing). `package.json` defines `typecheck` and `test`; CI runs neither [build-ci]
- **Linux matrix builds `tauri-shell` without GTK3/WebKitGTK dev packages** — `.github/workflows/rust-matrix.yml:25` [build-ci]

---

## P2 / P3 — Medium and low (55 + 10 findings)

Highlights; the rest are in the lane files.

**Wire protocol validation** [proto-wire] — `chunk_index >= MAX_CHUNKS_PER_FRAME` is accepted by both encode and decode (`media.rs:154`); a header may advertise `total_size` larger than its chunks can carry (`media.rs:67`); bodyless control messages silently discard trailing bytes, so `[0x01, 0x03]` decodes as a valid `StartStream` (`control.rs:561`).

**Client reassembly** [app-session] — a repeated frame header erases already-received chunks (`media.rs:97`); orphan promotion bypasses the per-frame chunk bound, letting a peer hold ~21.6 MiB in one incomplete frame; frame ordering breaks across `u32` wrap so loss accounting silently stops driving ABR (`media.rs:303`).

**Crypto/transport** [net-crypto] — STUN returns a mapping for a socket it immediately closes (`stun.rs:60`), so the advertised port has no listener.

**Secrets** [app-cli] — PSK keys and the bootstrap PIN are held in plain `Vec<u8>`/`String`, exposed via `derive(Debug)`, never zeroized (`pairing.rs:32-38`); the store's temp file is a static name with no Windows ACL (`pairing.rs:520-525`); `endpoint_aliases` grows without bound (`pairing.rs:529-537`); the `ctrlc_handler` drops its callback so graceful teardown never runs (`maho_client.rs:983-999`).

**Build** [build-ci] — the repo-root `Cargo.toml` points at `crates/spike-sck`, so every workspace command needs `--manifest-path clients/rust/Cargo.toml`; `rand` is specified incompatibly across manifests; `tauri.conf.json:6` has no `beforeBuildCommand`, so release packaging silently ships a stale `dist/`.

**Test quality** [tests-diff] — `maho-render/tests/audio_api.rs:85` asserts an enum variant equals itself; `receiver_telemetry_tests.rs:20` pins every latency percentile to the same constant (20), so rank inversion cannot fail the test; `mobile_regressions.rs:95` wraps assertions in `if let Ok`, silently passing when the constructor fails; `SessionSettingsPanel.test.tsx:117` tests extracted helpers instead of the slider UI it names.

**Frontend** [frontend-ui] — effect dependency omissions leave stale element refs and orphan held inputs on disconnect (`useRemoteInput.ts:69`); `handleWheel` ignores letterbox offset and strips modifiers (`:365`); the unreferenced `SessionSettingsPanel.tsx:147` monkey-patches `document.createElement` at production module scope.

**Rendering** [tauri-rust] — odd-height frames lose a chroma row and pull cursor bytes into pixel data (`lib.rs:1418`); `latest_cursor` survives disconnect, so a reconnect briefly shows the previous host's cursor (`lib.rs:658`).

---

## Uncommitted working tree

> **Snapshot caveat.** This section was reviewed against the working tree as of ~22:00 on 2026-09-14 (12 files, +193/−62). The tree kept moving during the review: by 22:32 it had grown to 13 modified files (+366/−75) plus two new untracked modules, `clients/rust/maho-host/src/service_windows.rs` and `windows_session.rs` (a LocalSystem Windows service supervising a SYSTEM capture worker), now declared in `lib.rs:27,31`. **Those two new modules and the later `main.rs`/`session.rs` growth are NOT covered by this review.**

Reviewed independently by the lead.

**`maho-host` (Windows cursor tracking + DPI):**
- **Regression:** `session.rs:1348-1351` divides by `frame.width`/`frame.height` with **no** zero guard — the code this hunk replaced had `if frame.width > 0 && frame.height > 0`. A zero-dimension frame yields NaN, and `f32::clamp` propagates NaN into the wire `CursorUpdate`.
- **Inconsistency:** two normalization bases now coexist — the DXGI branch divides by *frame* dimensions, the new `query_cursor()` fallback divides by *configured monitor* dimensions. If they ever differ, the cursor jumps between branches.
- **Dead API:** `capture_windows.rs:110-113` adds `pub fn selected_output_metadata(display_index)` with zero callers repo-wide.
- **Redundant:** both `main.rs::init_windows_dpi()` and `HostConfig::detect()` call `SetProcessDpiAwarenessContext`; the second fails once set (return discarded, so harmless).
- **Gap:** `DisplayInfo` gains `desktop_x`/`desktop_y` but macOS and Linux hardcode `0, 0`, so multi-monitor offsets remain unhandled off-Windows.

**`tauri-shell` (height-priority viewport sizing):** the rule is implemented three times over — inline styles on `<html>/<body>/#root` in `index.html`, the same rules again in `globals.css` `@layer base`, and inline style objects on `SessionView`/`SessionCanvas` that duplicate their own Tailwind classes. On `SessionCanvas` the inline `style` wins, making `max-w-full max-h-full object-contain` dead noise. Consolidate to one source of truth — the stylesheet — and drop the inline duplicates.

**Verdict:** the DPI/cursor work is directionally right but should not be committed until the zero-guard is restored and the dead `pub fn` is removed or wired up. The newer `service_windows.rs` / `windows_session.rs` work needs its own review pass before it lands.

---

## Test-suite health

- Exactly one `#[ignore]` in the entire Rust tree: `tauri-shell/src-tauri/src/discovery_tests.rs:292`, deliberately gated on an installed Tailscale — legitimate.
- Zero `.only` / `.skip` / `xit` in the TypeScript suites.
- Zero `thread::sleep` in any Rust test file; the frontend tests were converted to real-signal waits in commit `32ff84a`. Timing-luck tests are not a problem here.
- The one real gap is the opposite of flakiness: **the golden-vector tests that exist precisely to catch P0-1 are failing and nothing gates on them**, because CI has no lint/test enforcement beyond `cargo test --workspace`, which is itself red.

---

## Recommended order of work

1. **P0-1** — decide compatibility vs. intentional break, restore or re-freeze the KDF labels, get `cargo test --workspace` green. Nothing else can be validated until the suite is trustworthy.
2. **P0-2, P0-3, P0-4, P0-10** — the network-reachable security and DoS set.
3. **P0-9, P0-11 + the held-input P1 cluster** — `stop_host` hanging, the UI wedging on a failed cleanup, and keys stuck after disconnect are the defects a user meets first.
4. **P0-5, P0-6** — discovery robustness on macOS.
5. **P0-7, P0-8** — Linux host stability.
6. Add clippy + rustfmt + `bun test` + `tsc` to CI so this class of regression is caught mechanically.

---

## Evidence

- Per-lane findings: `.omo/code-review-20260914/findings/*.md` (17 files, 4,207 lines)
- Verification method: every P0/P1 finding's `Location` and first substantive quoted line were re-read from the source file and required to match within ±60 lines of the cited line; **66/66 passed, 0 unverified**. 20 of them were additionally hand-inspected line by line.
- Lane recovery: 4 lanes failed at spawn with provider `401 Authentication Failed`; 3 were recovered in a second run, and `decode-render` was reviewed directly by the lead after failing twice.
- Baseline logs: `/tmp/ulw-cargo-check.log`, `/tmp/ulw-bun-test.log`, `/tmp/ulw-net-test.log`
