# Physical iOS linker integration

Continue the same Gemini 3.8 Flash app session. The full device build compiled
Rust successfully (3 minutes) and generated/signing-prepared the real
debug-iphoneos app, but Xcode failed with "Undefined symbols for architecture
arm64" and clang++ linker exit 1. This is not a simulator/runtime failure.

Your ownership is extended to the generated native build INPUT
`clients/rust/ios-shell/gen/apple/project.yml` for required SDK framework
dependencies, and any corresponding persistent iOS config that is actually
supported. The lead owns final signed app build and device actions. Do not edit
the generated .pbxproj manually; regenerate it with xcodegen after fixing YAML.

Diagnose the frameworks required by the Rust static library rather than guessing
or bypassing undefined symbols. Inspect the actual `#[link(..., kind="framework")]`
dependencies in native decoder/audio/Keychain code and existing project.yml.
If needed, this iOS-only command prints native static library linker requirements:

```sh
ulimit -n 8192
RUSTC_WRAPPER= CARGO_BUILD_JOBS=4 CARGO_TERM_COLOR=never \
  IPHONEOS_DEPLOYMENT_TARGET=16.0 \
  CARGO_TARGET_DIR=/Volumes/T9-Mac/project/EclipticRD-Rewrite/clients/rust/target-ios-device \
  cargo rustc --manifest-path clients/rust/Cargo.toml \
  -p erd-ios --lib --target aarch64-apple-ios -- --print native-static-libs
```

Tauri's default generated project may not include extra VideoToolbox/CoreMedia/
CoreVideo and CPAL/AVFAudio/AudioToolbox/CoreAudio frameworks. Link precisely the
required system SDKs. No weak-link fallback, ignored symbols, dead-code hiding,
or removing the actual media implementation to get green.

Use read before every file edit and apply_patch for source/YAML edits. xcodegen
is permitted to regenerate the project from that YAML. Do not rerun Tauri init
and overwrite the custom project. Do not build/run any simulator or emulator;
all target commands must remain aarch64-apple-ios/iphoneos. Do not change team,
bundle ID or signing credentials.

Stop after the corrected project inputs and regenerated project are saved, and
record required frameworks plus exact commands in app-handoff.md. The lead will
rerun the complete device build and capture full linker errors if any remain.
