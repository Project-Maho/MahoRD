# Mobile code validation evidence

Date: 2026-09-08

The lead executed these checks. Production fixes and behavioral regressions were
delegated to Gemini 3.8 Flash. Scope: `clients/rust/erd-mobile`, not full mobile
app implementation, Android/iOS target compilation, installation or native QA.

## Final Omarchy command

Only the mobile crate's source was refreshed in the isolated source snapshot.
The existing remote main source tree and deployed daemons were not changed.
The Cargo target directory was reused as a compilation cache.

```sh
cd /home/indo/projects/erd-mobile-review-20260908-f716b57e
export PATH=/home/indo/.cargo/bin:$PATH
export PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig
export LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib
export CARGO_TARGET_DIR=/home/indo/projects/EclipticRD-Rewrite/clients/rust/target

cargo test --manifest-path clients/rust/Cargo.toml -p erd-mobile &&
cargo clippy --manifest-path clients/rust/Cargo.toml \
  -p erd-mobile --all-targets --no-deps -- -D warnings &&
cargo build --manifest-path clients/rust/Cargo.toml -p erd-mobile &&
printf 'MOBILE_CODE_GATE_PASS\n'
```

Monitor: `mon_MZSYTF4SMQV8P8BJ`, terminal `bash_15`.
**Final command exit code: 0.**

Captured decisive output:

```text
Finished `test` profile [unoptimized + debuginfo] target(s) in 1.67s
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 32 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.38s
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
MOBILE_CODE_GATE_PASS
```

## Code-only QA matrix

The integration test binary imports the actual public `erd_mobile` library. It
does not replace the library with a mock and does not open mobile device sessions.

| Scenario | Evidence | Verdict |
|---|---|---|
| C ABI allocation, valid input rejection before networking, cleanup | `unconnected_handle_send_touch_must_not_return_ok`, bridge unit tests | PASS |
| Empty host/output handle failure behavior | `create_with_empty_host_must_return_error`, `create_failure_with_sentinel_must_reset_out_handle_to_null` | PASS |
| No fictitious video/audio output | Android/iOS native-backend regression tests and typed-error unit tests | PASS |
| Direct touch down/drag/up | `gesture_handler_direct_touch_emits_mouse_down_drag_up` | PASS |
| Secondary/unknown/repeated touch cannot steal or release primary | Direct touch ownership regressions | PASS |
| Mode switch releases once; same mode preserves gesture | Mode-switch unit and integration tests | PASS |
| Invalid relative move preserves last valid baseline | `trackpad_relative_delta_recovers_after_rejected_invalid_input` | PASS |
| Relative cancellation with NaN admits next owner | `trackpad_relative_cancel_with_nan_clears_baseline_and_allows_new_owner` | PASS |
| Cancellation after invalid viewport mutation | `direct_touch_cancel_after_viewport_mutated_to_nan_releases_last_valid_coordinates` | PASS |
| Invalid viewport, zoom centers, arithmetic overflow | Viewport table, pan and relative-delta overflow regressions | PASS |
| Fair thermal low caps and maximum u32 | Fair thermal regression and unit tests | PASS |
| Adjacent keyboard, lifecycle, mock storage policies | Existing unit tests, without claiming native integration | PASS |
| Clippy all mobile targets, warnings as errors | Final command above | PASS |
| Linux lib/cdylib/staticlib build | Final command above | PASS |

All PASS rows refer to the final test/build output above and to
`clients/rust/erd-mobile/tests/mobile_regressions.rs` or in-crate unit tests.

## Formatting and limitations

The following non-compiling format check ran on the Mac, with exit code **0**:

```sh
rustfmt --edition 2021 --check \
  clients/rust/erd-mobile/src/bridge.rs \
  clients/rust/erd-mobile/src/android.rs \
  clients/rust/erd-mobile/src/ios.rs \
  clients/rust/erd-mobile/src/touch.rs \
  clients/rust/erd-mobile/src/power.rs \
  clients/rust/erd-mobile/tests/mobile_regressions.rs
```

LSP calls failed because the local daemon at
`/Users/indo/.omo/lsp-daemon/v0.1.0/daemon.sock` was unreachable. No LSP-clean
claim is made; compiler and Clippy diagnostics were used on Omarchy instead.
Omarchy's enumerated toolchains were stable, 1.88.0 and 1.98.1; nightly/Miri
was absent. Miri was not run, so this is not a Miri soundness certification.

No physical device, emulator, mobile app installation, native codec runtime,
APK/IPA creation, deployment or git commit was part of this verification.
The unresolved product blockers remain documented in
`docs/mobile-client-review-20260908.md`.

## Independent review

Reviewer: `omo-senpi-gate-reviewer`, task `st_01a080db`, model
`mahoquot/gemini-3.8-flash-high`.

Verdict: **APPROVE, zero blockers** for the scoped remediation and report.
The reviewer separately states that the mobile product is **NOT READY**.
Full report: `.omo/evidence/mobile-client-review-gate-review.md`.

Terminology clarification: Apple Personal Team's documented seven-day limit
applies to provisioning profiles and associated registration limits; it is not
a general statement that every signing certificate expires after seven days.
The primary review links the official Apple account documentation.
