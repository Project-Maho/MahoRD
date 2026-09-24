# Phase A pre-commit review: remaining test work

This is a coordinator pre-commit finding, not the final personal review or a replacement for the skipped `verify-trust` node.

## Required Flash follow-up

Read `clients/rust/tauri-shell/src-tauri/src/pairing_tests.rs` before editing.

1. `test_failed_reconnect_preserves_original_cause_and_never_falls_back_to_bootstrap_pin` obtains a free TCP port by binding a listener, then drops that listener before connecting. Another process can bind the port in between. Replace this with an owned, deterministic failing peer or other deterministic transport failure seam. Preserve the assertion that the original reconnect error survives and no bootstrap fallback occurs.
2. `test_loopback_reconnect_failure_does_not_trigger_bootstrap_request` starts a blocking accept and ends with `let _ = server_handle.join()`. Acceptance and completion have no explicit test deadline, and the join result is ignored. Subscribe to the exact worker completion before triggering the client, bound completion, check the worker result, and ensure cleanup on assertion failure. Do not use sleeps, polling delays, or an unbounded join after a timeout.
3. The same loopback test checks `bootstrap_attempted` before joining its worker. Check the outcome after synchronized worker completion rather than depending on scheduling.

Use the existing test names above. Pin any additional regression name and its exact RED invocation in the append-only notepad before running it. Keep all implementation and follow-up changes on Gemini 3.8 Flash. Run compilation/tests only in the isolated Omarchy workspace. Preserve every existing assertion's behavioral protection; do not remove a test or substitute a prose assertion.

Earlier passing commands remain historical evidence. They do not remove these test-quality concerns. Re-run the affected target once after correction, with a reliable single-run pass, then let the phase A independent verifier audit the result.

## Commit boundary

The current shell `lib.rs` diff contains both owned IPC/authentication changes and pre-existing discovery changes. In particular, the `connect_session` hunk combines discovery lookup with the extracted authentication helper; the new test module also depends on that helper. Do not stage the entire file to make a convenient commit. Keep discovery/UI/input edits owned by other work intact, and select only the verified owned change when the dependency boundary is resolved.

## Resume

Existing workflow: `dag_7726271d-2591-4fdf-a559-fba9e38cc31d`, generation 2. Resume the failed `public-ipc` node with this report added to its brief; its skipped `verify-trust` dependent must subsequently run. Do not rerun completed `store-isolation` or `host-consent` nodes. Provider reset-window monitor remains `bash_14`; the reset timestamp is a retry opportunity, not proof that quota has recovered.
