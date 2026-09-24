# Physical iOS client implementation contract

The user approved actual iOS app implementation by Gemini 3.8 Flash, and
Mac/Xcode compilation specifically for physical-device iOS builds. No simulator
or emulator may be launched, booted, built for, or used. Other compilation/tests
remain on Omarchy Linux. No git commits or App Store publishing.

## Architecture and ownership

Preserve the existing Rust core and desktop Tauri app. Create a small dedicated
Tauri v2 application in `clients/rust/ios-shell` (package `erd-ios`) using
`erd-app::ClientSession`, native iOS decoding from `erd-decode`, existing
`erd-render::CpalAudioOutput` and `erd-mobile::TouchGestureHandler`. Do not restore
the retired Swift app. A Tauri-generated Apple wrapper is platform scaffolding,
not a replacement implementation of the Rust core.

Three independent Flash executors own disjoint paths:

1. Native decoder: `clients/rust/erd-decode/**` only.
2. App/runtime: new `clients/rust/ios-shell` files EXCEPT `ui/**` and `DESIGN.md`;
   narrowly scoped portability/Keychain changes in `erd-app`, `erd-mobile` and
   `erd-render`; add only the new member in `clients/rust/Cargo.toml`.
3. Mobile UI: `clients/rust/ios-shell/ui/**` and `DESIGN.md` only.

All pre-existing dirty changes must be preserved. Read before editing, use
apply_patch for all edits. Never overwrite another executor's owned file.
Do not modify the lockfile or generated Apple project; the lead owns dependency
resolution, Tauri generation, compilation, signing and installation.

## Decoder contract

Add feature `ios-videotoolbox` to `erd-decode` without pulling in FFmpeg. For
`target_os = "ios"` with this feature, expose the existing `HevcDecoder` public
surface backed by actual VideoToolbox:

- `new`, `new_h264`, `from_keyframe`, `from_keyframe_auto`
- `decode(&mut self, access_unit: &[u8], timestamp_ms: i64)`
  returning actual `Vec<Nv12Frame>`
- `flush`, `acceleration` with VideoToolbox reported

Reuse existing AVCC parsing and codec detection. Preserve existing desktop
FFmpeg behavior and disabled-backend behavior on other targets. Decode both
HEVC and H.264 from actual parameter sets, not fabricated samples or counters.
Input is four-byte big-endian length-prefixed NALUs, not Annex-B.

Use real CoreMedia format/sample descriptions and VideoToolbox decompression
sessions. Prefer installed typed Apple bindings when practical. Own/release all
CF and callback resources, honor lifetimes across async callbacks, safely
invalidate/drain on drop, and copy actual NV12 planes with stride awareness.
Do not add unsafe Send/Sync without a precise synchronization/lifetime proof.
Use typed decoder errors; native failures must propagate.

The app/runtime worker will enable `ios-videotoolbox` ONLY in its iOS target
dependencies and keep `erd-app`/`erd-mobile` from enabling FFmpeg transitively.

## App command contract for the independent UI executor

Tauri global bridge is available as `window.__TAURI__.core.invoke`.
Use the following commands, with camelCase outer arguments and snake_case fields
inside struct values unless explicitly specified:

- `connect({host, pin})`: PIN string or null; real pairing/reconnect and handshake.
  Returns safe session stats after Ready, not after local allocation.
- `disconnect()`: releases held input, cancels pending connect/media, joins workers,
  stops audio, clears stale frames, and returns only after teardown completes.
- `stats()`: `{state, host, frames_received, frames_decoded, audio_packets_received,
  audio_samples_played, width, height, last_error}`.
  `state` is `"idle" | "connecting" | "ready" | "error"`.
  A successful handshake is separate from first decoded/presented video.
- `poll_frame()`: binary `tauri::ipc::Response`. Empty if no NEW frame.
  Otherwise header of width:u32 LE, height:u32 LE, sequence:u64 LE (16 bytes),
  followed by contiguous NV12 Y (`width*height`) and interleaved UV
  (`width*ceil(height/2)`) planes. The backend repacks native stride.
- `touch({event:{id, x, y, phase}})`: id is a safe nonnegative integer;
  x/y normalized TOP-LEFT coordinates relative to the aspect-fit video rectangle;
  phase `"began" | "moved" | "ended" | "cancelled"`.
  The Rust touch handler owns inverted-Y wire normalization, not the UI.
  Rust uses a unit viewport to map these normalized positions.
- `set_touch_mode({mode})`: `"direct" | "trackpad"`; sends any release returned by
  the actual handler before accepting input in the new mode.
- `send_key({keyCode, down, modifiers})`: physical wire key code and u16 modifier
  flags; reject invalid values. For soft keyboard accessory keys use existing
  mobile key mappings, not Unicode pretending to be a physical key code.
- `set_muted({muted})`: actual audio queue mute.
- `presented({sequence})`: called after an actual WebGL draw, once for the first
  frame and then periodically, not for fabricated frames. Records client
  presentation progress in logs for physical-device QA.
- `startup()`: safe default preferences and DEBUG-ONLY QA auto-connect request:
  `{host: string|null, auto_connect: bool}`. Must never return a pairing key/PIN.

Keep all network/codec/audio blocking work off the UI and Tokio executor. One
active session only; serialize input ordering and generation-guard stale workers.
Use actual desktop session runtime for TCP control and heartbeats, UDP receive,
keyframe requests and cancellation. Do not implement a fake network path or
proxy video through a laptop.

## Pairing and mobile lifecycle

Use existing TLS-PSK pairing and connect_with_pairing; no auth bypass.
Store pairing keys in real iOS Keychain, not localStorage or source code.
Keep desktop PairingStore behavior unchanged. Any iOS Keychain-specific
implementation must propagate access errors and support reconnection after
process restart.

Use actual iOS audio output and report callback consumption, not packet counts
as proof of playback. Activate the appropriate iOS audio session if CPAL does
not do so. Handle background/visibility transition by releasing inputs and
disconnecting or explicitly suspending the current stream; do not silently
continue stale input on resume. Preserve aspect fit and orientation changes.

For headless launch automation on a PHYSICAL phone only, debug builds may read
`Documents/erd-device-qa.json` inside their own sandbox. It can contain a host
and explicit PairingRecord provisioned by the lead. Import the key to Keychain,
delete the provisioning file, and expose only the safe host/auto_connect flag to
the UI. Do not embed test keys or PINs, and never log secrets. The lead owns
provisioning and any host-side access. Normal builds/UI use user-entered pairing.

## Mobile UI

Reuse the charcoal/coral design tokens from the existing
`clients/rust/tauri-shell/DESIGN.md`, with 44+ px touch targets, safe-area insets,
portrait/landscape support and accessible connection controls. No redesign of
the desktop app and no new JS framework.

Required real surfaces: host/PIN form, connect/cancel/error state, live aspect-fit
NV12 WebGL video, direct/trackpad touch modes, Escape/Tab/arrows/modifier accessory
controls, audio mute, actual counters, disconnect/back. No fake host availability
badges or prefilled secrets. Missing Tauri must disable native actions honestly.
Pointercancel, blur and visibility loss release input before teardown. Local
buttons must never inject remote input. Buffer-poll scheduling must be single
flight and stopped on teardown, with stale responses ignored.

## Testing and handoff

Write focused deterministic regressions before changing existing behavior.
No sleeps/polling in tests; no skipped or weakened tests. Unit-test parsing,
frame packing, input ownership and lifecycle serialization at their public seams.
Record tests not run honestly. Native decoder execution must eventually be
proven on the real iPhone; no mock result substitutes.

Each executor saves a short handoff under `.omo/ios-device-20260908/` listing
changed files, commands/tests to run, API deviations, resource invariants and
remaining integration concerns. Stop when its owned implementation and tests
are saved. Lead performs compilation and physical-device QA.
