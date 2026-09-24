# Finish UI regression gate with actual test execution

Continue the same Gemini 3.8 Flash UI session. You now have Bash specifically to
run the following scoped remote code test. This supersedes the earlier
lead-only TEST EXECUTION rule, not the no-device/no-emulator rules.

```sh
rsync -az clients/rust/ios-shell/ui/ indo@100.91.254.71:/home/indo/projects/erd-ios-device-20260908/ui/ &&
ssh indo@100.91.254.71 "bash -lc 'cd /home/indo/projects/erd-ios-device-20260908/ui && bun test test'"
```

The lead's latest full suite still fails input-queue FIFO/stale-drop tests and
three lifecycle callback assertions. Run this exact command, inspect the actual
assertions and fix the root cause in owned UI files. Do not guess from summaries.
Use read and apply_patch for all source/test changes; never shell-write files.
Do not run any local test/build or browser/device action.

Important test discipline: `await Promise.resolve()` is not an observable
invocation signal and is not a reliable way to await an arbitrary promise chain.
Tests of input queue ordering must subscribe to a deferred signal resolved
inside the fake native invoke callback, await that exact signal, then assert
queue state before releasing the gated invocation. Lifecycle tests must likewise
await a precisely observable callback/state transition if the callback is async.
No sleeps, polling delays, repeated generic microtask flushes, retries for luck,
prose matching, skipped tests or weakened assertions. Preserve actual native
disconnect callback coverage, stale input rejection, FIFO ordering and visible
blur behavior.

Keep native command names/contracts unchanged. Do not edit Rust, config, icons,
generated Apple files or anything outside your UI ownership. Stop only after one
complete test run passes reliably and the handoff records the exact command,
test counts and output; include any production/test changes made.
