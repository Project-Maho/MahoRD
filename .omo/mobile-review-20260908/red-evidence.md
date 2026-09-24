# Mobile regression RED evidence

Date: 2026-09-08

Executor: lead session. Test author: Gemini 3.8 Flash
(`mahoquot/gemini-3.8-flash-high`, CLI fallback disabled).

All compilation and test execution took place on Omarchy Linux. No physical
mobile device, emulator, native mobile app, remote desktop host session or
deployment was used by this test command.

## Source and command

The original production files in `clients/rust/erd-mobile/src` were compared with
the lead's pre-edit snapshot before execution; none had changed. Only the new
`tests/mobile_regressions.rs` file had been added.

Isolated remote source:
`/home/indo/projects/erd-mobile-review-20260908-f716b57e`

```sh
cd /home/indo/projects/erd-mobile-review-20260908-f716b57e
env \
  PATH=/home/indo/.cargo/bin:$PATH \
  PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig \
  LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib \
  CARGO_TARGET_DIR=/home/indo/projects/EclipticRD-Rewrite/clients/rust/target \
  cargo test --manifest-path clients/rust/Cargo.toml \
    -p erd-mobile --test mobile_regressions
```

Monitor: `mon_ZZG34K3AB0EY5J52`, terminal `bash_6`.

Exit code: **101**, expected regression failure.

## Captured output

```text
Finished `test` profile [unoptimized + debuginfo] target(s) in 15.98s
running 24 tests
test direct_touch_invalid_began_must_be_rejected_and_preserve_state ... ok
test result: FAILED. 1 passed; 23 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
error: test failed, to rerun pass `-p erd-mobile --test mobile_regressions`
```

Selected exact failure observations:

| Scenario | Observed failure |
|---|---|
| Unconnected touch | `left: Ok`, `right: Ok` in the `assert_ne!` |
| Empty host | `left: Ok`, `right: Ok` in the `assert_ne!` |
| Android video | `left: Ok(true)`, `right: Ok(true)` |
| iOS video | `left: Ok(true)`, `right: Ok(true)` |
| Android audio | `left: 4`, `right: 0` |
| iOS audio | `left: 4`, `right: 0` |
| Secondary touch release | `left: Some(LeftMouseUp)`, `right: Some(LeftMouseUp)` in `assert_ne!` |
| Cancel with invalid position | `NonFiniteCoordinates(NaN, NaN)` / `NonFiniteCoordinates(inf, inf)` |
| Relative baseline pollution | `left: NaN`, `right: 20.0` |
| Invalid viewport | `case 'nan view_width' should return Err` |
| Fair thermal with `u32::MAX` | `attempt to multiply with overflow` at `power.rs:84:35` |
| Fair thermal below 1,000 kbps | `assertion failed: bitrate <= 800` and `assertion failed: bitrate <= 500` |

The one passing invalid-Began test is adjacent behavioral coverage, not evidence
that the state-pollution defect was reproduced by that particular test.
Compilation succeeded before assertion failures; these are behavior regressions,
not missing imports, missing APIs or toolchain failures.

## Integration review and second RED

The first implementation passed 27 unit tests and 28 integration tests, but its
Clippy run failed. The new nested relative-Began condition triggered
`clippy::collapsible_if` at `touch.rs:246`. Four existing default-then-assign test
fixtures in the touched Android/iOS files triggered
`clippy::field_reassign_with_default`. Neither warning was suppressed.

Code review then found a new ownership interaction: relative mode now preserved
one owner, but Cancelled with NaN coordinates returned before clearing that owner.
A new independent regression was written before the correction, together with
three adjacent edge cases.

The same remote command above was rerun against this intermediate implementation
and the expanded integration tests. Monitor: `mon_CZ6MWCNZZ13VFJJX`, `bash_11`.
Exit code: **101**.

```text
Finished `test` profile [unoptimized + debuginfo] target(s) in 0.59s
running 32 tests
test trackpad_relative_cancel_with_nan_clears_baseline_and_allows_new_owner ... FAILED
assertion failed: cancel_res.is_ok()
test result: FAILED. 31 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The adjacent mutated-viewport cancellation, finite pan overflow and finite
relative-delta overflow cases passed. The relative cancellation failure was
returned to the same Gemini 3.8 Flash session for correction.
