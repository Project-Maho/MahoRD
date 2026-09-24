# Native VideoToolbox executor

You are the Gemini 3.8 Flash executor requested by the user. Read
`.omo/ios-device-20260908/contract.md`. Own only `clients/rust/erd-decode/**`.
Deliver ONE real native iOS HevcDecoder backend and its focused regression tests.
Do not edit other crates, app/UI, lockfile, or generated project files.

Inspect the complete current decoder implementation and tests before edits:
there are concurrent pre-existing performance changes to preserve. Read the
Rust programming reference and unsafe/FFI reference set before unsafe code.

Installed binding source directory:
`/Users/indo/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`.
CoreMedia/CoreVideo/CoreFoundation 0.3.2 bindings are already cached. You may add
target-specific binding dependencies or a small audited raw VideoToolbox FFI
module if the VideoToolbox crate itself is not cached. Use real SDK signatures,
not guessed function layouts. SDK headers can be read under Xcode's iPhoneOS SDK.

The lead has started a baseline real iOS-target compile to expose current
desktop dependency leakage. Add tests before implementation where a pure
behavior seam exists. Do not execute any cargo command, build, device action,
emulator or simulator. Use only read and apply_patch for implementation.

Preserve existing desktop FFmpeg and disabled-backend contracts. No test-only
fake native decoding. Ensure synchronous API calls account for asynchronous VT
callbacks and resource ownership. No success without actual decoded pixels.
Follow the exact public decoder contract in contract.md so the app can integrate.

Save handoff `.omo/ios-device-20260908/native-handoff.md` and stop.
