# Physical iPhone test attempt

Date: 2026-09-08. User authorized real iPhone testing and absolutely prohibited
emulators/simulators. No emulator or simulator was used.

## Observed device access

`xcrun devicectl --timeout 25 list devices` exited 0.

| Device | Details query | App query for `com.eclipticrd.shell` | Transport |
|---|---|---|---|
| iPhone 16 Plus, iOS 26.6.1 | Exit 0; paired, developer mode enabled | Exit 2; command timeout after 25 seconds | localNetwork |
| iPhone 12 Pro, iOS 26.6 | Exit 0 but reported DDI mounting failure; paired, developer mode enabled | Exit 1; `kAMDMobileImageMounterDeviceLocked`, CoreDeviceError 12040 | localNetwork |

Commands used `device info details --device <identifier>` and
`device info apps --device <identifier> --bundle-id com.eclipticrd.shell`.
App installation state is UNKNOWN because neither app query succeeded.

## Application blocker

A fresh repository scan found no IPA, iOS Xcode project or XCArchive. The only
EclipticRD app bundle found was the macOS bundle under
`clients/rust/target/debug/bundle/macos/EclipticRD.app`. Its Info.plist identifies
`tauri-shell`, `com.eclipticrd.shell` and macOS minimum system version 10.13.
It is not an iPhone application and was not sent to the phone.

`clients/rust/erd-mobile/Cargo.toml` still defines lib/cdylib/staticlib only.
`src/bridge.rs` still allocates only a local handle and returns BackendUnavailable
for valid send requests; native mobile integration is not complete.

No app was installed or launched, and no remote-desktop session was tested.
Continuing requires a real iOS app implementation/build/signing path. The prior
Omarchy-only compilation rule has not been explicitly lifted; the iOS/macOS
build exception must be resolved before compiling. Physical device access also
needs the intended phone reachable and unlocked; USB can avoid the observed
wireless access problem.
