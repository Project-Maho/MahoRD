# Native decoder warning cleanup

Continue the same Gemini 3.8 Flash native-decoder session. Own erd-decode only.
The lead verified that `cargo check -p erd-decode --no-default-features --features
ios-videotoolbox --target aarch64-apple-ios` exits 0. Integration compilation
reports dead-code warnings for unused FFI declarations/constants such as
kVTDecodeFrame_EnableAsynchronousDecompression, CFRetain and
CVPixelBufferGetWidthOfPlane. Remove genuinely unused declarations rather than
suppressing warnings. Preserve real SDK signatures and active resource logic.

You may now execute only this physical iOS-target check (no host or simulator
builds), with Bash available for that command and read-only SDK inspection:

```sh
ulimit -n 8192
RUSTC_WRAPPER= CARGO_BUILD_JOBS=4 CARGO_TERM_COLOR=never \
  IPHONEOS_DEPLOYMENT_TARGET=16.0 \
  CARGO_TARGET_DIR=/Volumes/T9-Mac/project/EclipticRD-Rewrite/clients/rust/target-ios-device \
  cargo check --manifest-path clients/rust/Cargo.toml \
  -p erd-decode --no-default-features --features ios-videotoolbox \
  --target aarch64-apple-ios
```

Use read before edits and apply_patch for all edits. No warning suppressions,
no feature fallbacks, no removal/weakening of tests and no unrelated cleanup.
No device/simulator/emulator/signing commands. Stop after this check is clean
and native-handoff.md records the real result.
