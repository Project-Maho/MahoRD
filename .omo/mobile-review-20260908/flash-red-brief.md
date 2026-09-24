# Gemini 3.8 Flash mobile regression task

You are the implementation executor explicitly requested by the user. Use the selected Gemini 3.8 Flash model only. Do not delegate or change models.

Read `docs/mobile-client-review-20260908.md` and the current `clients/rust/erd-mobile` source. The workspace contains many pre-existing uncommitted changes; preserve all of them. Use read before edits and apply_patch for every file creation or edit.

This first stage is TESTS ONLY. Deliver one new integration test file at `clients/rust/erd-mobile/tests/mobile_regressions.rs`. Do not edit production code or existing tests yet. Do not run builds/tests locally. The lead will run these tests on Omarchy Linux to observe RED, then send a second instruction to implement fixes.

Write deterministic behavior regressions for:

1. Creating a mobile handle is not proof of network readiness: valid touch sending through the current unconnected handle must not return Ok. Use TCP 19730, not agent HTTP 19735. Destroy every successfully allocated handle.
2. Creating with an empty host fails. A valid output slot is set to null on failure, even when its initial value is a dangling non-null sentinel that is never dereferenced.
3. Nonempty data must not be reported decoded, rendered, or played by the missing Android/iOS native backends. Existing construction validation may still fail early (that is allowed); if construction succeeds, calls must explicitly fail or report zero actual output, and counters must not advance. Avoid creating tests that will fail solely because a new API does not exist: use existing API and observable results.
4. In DirectTouch, secondary touch lifecycle must not generate mouse button events belonging to the first active touch; an unknown Ended must not generate Up; after the primary ends it must have exactly one Up.
5. Invalid Began/Moved input must not corrupt the active touch or next relative delta. Relative NaN/Infinity must be rejected. Cancelling a held direct touch must release even when the platform supplies invalid coordinates (use last valid position).
6. `set_zoom` with nonfinite center must fail without changing state; invalid public viewport dimensions/zoom/offsets must not produce accepted host coordinates. Include finite arithmetic overflow if practical.
7. Fair thermal adjustment must never raise a low configured cap and must not overflow with u32::MAX.

Use independent Given/When/Then scenarios, no sleeps, no device/emulator/server actions, no tautological assertions, and no prose/prompt tests. Check only machine behavior. You may group value-class cases in a table. Read the Rust programming skill before writing Rust. Do not mask or delete existing failing tests.

Stop immediately after the regression file is saved. Report the exact file and the behaviors covered. Do not implement fixes, do not commit, do not access devices, do not install anything, do not touch other directories. The lead owns all remote sync and execution.
