# Finish physical-iOS target compilation

Continue the same Gemini 3.8 Flash app/runtime session. You now have Bash only
for the specific iOS-target checks below and read-only SDK/source inspection.
This supersedes the earlier lead-only compilation rule for this target.
No simulator/emulator commands, no Mac-target compilation, no device operations,
no signing, no git commits, no remote host changes.

The native backend compiles for the real iOS target. The app integration check
has not passed yet. The lead generated `ios-shell/icons` from the existing brand
icon and ran Tauri iOS init; do not modify those generated assets/projects.
Use this command and fix the real errors in your owned paths:

```sh
ulimit -n 8192
RUSTC_WRAPPER= CARGO_BUILD_JOBS=4 CARGO_TERM_COLOR=never \
  IPHONEOS_DEPLOYMENT_TARGET=16.0 \
  CARGO_TARGET_DIR=/Volumes/T9-Mac/project/EclipticRD-Rewrite/clients/rust/target-ios-device \
  cargo check --manifest-path clients/rust/Cargo.toml \
  -p erd-ios --lib --target aarch64-apple-ios
```

After the lib check passes, type-check the tests for that same iOS target:

```sh
RUSTC_WRAPPER= CARGO_BUILD_JOBS=4 CARGO_TERM_COLOR=never \
  IPHONEOS_DEPLOYMENT_TARGET=16.0 \
  CARGO_TARGET_DIR=/Volumes/T9-Mac/project/EclipticRD-Rewrite/clients/rust/target-ios-device \
  cargo check --manifest-path clients/rust/Cargo.toml \
  -p erd-ios --tests --target aarch64-apple-ios
```

Read and use apply_patch for every source/test edit. Never suppress errors or
add unsafe Send/Sync to bypass ownership problems. No fake implementations,
omitted audio, plaintext pairing persistence, or commented-out failing paths.
The previous integration-corrections brief remains binding. No edits outside
your original app/runtime ownership, and specifically no edits to erd-decode or
UI files. If an error belongs to the native decoder, report its exact diagnostic
and stop that branch; the lead has a separate owner for it.

Use actual APIs, not guessed names. Preserve pre-existing tests/behavior. Stop
when both checks pass and app-handoff.md records the real command results, or
when a concrete out-of-ownership blocker is reported.
