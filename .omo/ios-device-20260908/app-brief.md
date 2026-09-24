# iOS app/runtime executor

You are the Gemini 3.8 Flash executor requested by the user. Read
`.omo/ios-device-20260908/contract.md`. Deliver ONE functional Rust/Tauri iOS
application runtime with manifests, build script and configuration, including
the complete command contract in that file.

Own new `clients/rust/ios-shell/**` EXCEPT `ui/**` and `DESIGN.md` (UI executor),
narrow portability/Keychain changes in `erd-app`, `erd-mobile`, `erd-render`,
and adding the `ios-shell` member to the Rust workspace. Do NOT edit erd-decode,
desktop tauri-shell, lockfile or generated Apple project.

Read source before editing; preserve the many existing dirty changes. New
package name `erd-ios`, library `erd_ios_lib`, bundle identifier
`com.eclipticrd.ios`. Create Tauri v2 Cargo.toml/build.rs/src/lib.rs or compatible
layout, tauri.conf.json, capabilities/default.json and iOS Info.plist additions
(local-network usage explanation, valid orientations). The lead runs Tauri init
and signs; do not invent a signing team or generated project.

Native decoder worker owns erd-decode and adds `ios-videotoolbox`, exposing the
existing HevcDecoder interface for iOS without FFmpeg. Enable it in target-specific
iOS dependencies. Remove transitive desktop FFmpeg activation from the mobile
path, while preserving desktop default features. Ensure the portable no-FFmpeg
erd-app library still compiles. Existing CpalAudioOutput should be used for real
iOS audio with error/status propagation.

Use ClientSession::pair_with_pin / connect_with_pairing and the existing runtime
for actual TCP/UDP/heartbeats. Use an explicit sandbox path; implement actual
iOS Keychain persistence without changing desktop file storage. No secret in JS
or source. Do not log keys/PINs. Honor debug-only QA provisioning in contract.md.
Add structured runtime logs for authenticated Ready, real decoded frame counts,
real audio callback consumption, input sent and frontend presentation reports.

Handle cancellation, disconnect, generations and bounded frame buffering properly.
The UI executor depends on the command contract, so do not silently change names
or payloads. Avoid unwrap/panic in application commands. Read Rust/FFI skills.

Write focused deterministic tests before behavior changes; do not execute builds,
tests, emulators, simulators or device commands yourself. The lead owns all such
execution and will return compiler failures. Save
`.omo/ios-device-20260908/app-handoff.md` and stop.
