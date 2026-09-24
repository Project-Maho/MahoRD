# Concurrent host-management changes: unresolved R11 policy conflict

Observed after phase A was accepted and R3 work began. These additions are not in the original baseline or the R3 write scope. Their author has not yet been established; do not revert or stage them as R3 work.

## Actual source observations

- `clients/rust/tauri-shell/Cargo.toml` adds a production `erd-host` dependency.
- `src-tauri/src/lib.rs`, `HostRuntime::default`, initializes PIN `12345678`.
- `AppState::start_host` also falls back to `12345678` for an empty PIN.
- Its consent worker calls `prompt.respond(true)` for every request.
- Its TCP listener binds `0.0.0.0` at the standard host port.
- The shell `run()` path invokes `state.start_host()`.
- `get_host_status` reports `auto_approve: true`.
- `erd-host/src/session.rs` adds `serve_with_stop`, rewrites `serve`, and polls nonblocking accept with a 100 ms sleep.
- The new shell host path calls that API. Removing the host API in isolation would break this concurrent dependency.
- `tests/page-harness.mjs` adds host start/stop/status fixtures with fixed PIN and automatic approval.

The R11 objective requires no default fixed PIN and explicit pairing policy. The new automatic host path conflicts with that objective even though the previously verified daemon default-PIN fix remains committed.

## Ownership boundary and investigation

The R3 producer was asked whether it authored these hunks and told not to remove another session's work. It was also informed of the shell's dependency on `serve_with_stop`; it should continue only the R3 registration paths.

Session-finder searches for `serve_with_stop` and `get_host_status` across today's Senpi, Codex and Claude records returned no matches. Direct project-log enumeration found only the current parent session for today, not a separate author transcript. This is insufficient evidence to assign ownership.

## Required resolution

Track `동시 호스트 관리 변경의 R11 충돌 해결` in Identity. Preserve the new controls and files pending ownership/policy clarification. Proposed integration is to retain host management while bringing its default PIN and consent behavior into the same R11 policy as the daemon. Do not claim full R11 acceptance against this later working-tree state or run the new shell entry point as a secure configuration before resolution.

The independent R3 workflow remains active; its implementation does not require changing this concurrent host-management policy.
