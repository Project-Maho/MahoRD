# Phase C handoff: explicit credentials and scoped endpoints

Coordinator preparation while phase B is active. This is not implementation or evidence of completed R8/R9/R10. Start a separate phase-C workflow only after B is accepted.

## Current interfaces actually inspected

- `erd-app/src/pairing.rs` still has `PairingRecord { id, name, key, added_at_unix_ms }`. Key serialization is base64 and `addedAt` uses the existing date conversion. Preserve these names and formats; the contract's illustrative renamed record types do not override lead amendment 1.
- `PairingStore::load(id)` already provides exact lookup. Extend this storage rather than creating another credential database. Preserve filesystem and platform Keychain backend behavior.
- Desktop `commands::resolve_stored_pairing` still falls back from ID to name/host and discovered name. Its `authenticate_client_session` extraction and the extra R11 tests remain uncommitted, deliberately excluded from the isolated R4 commit `fa476b1`. Integrate them here without losing their no-bootstrap/error-preservation coverage.
- iOS has a real `src/commands.rs`: `connect` currently takes host, ports, PIN and stream options, calls `build_session_config`, then `AppState::connect_async`.
- iOS `connect_async` owns its existing lifecycle lock and runs `NativeSession::start` in `spawn_blocking`. Do not remove that serialization to implement credential selection.
- `NativeSession::start` uses the platform pairing store and currently falls back to `find_by_host(&host)` when PIN is absent. R9 must replace this with the explicitly selected ID.
- iOS `ui/connection-state.js` currently rejects a connection without a valid PIN and sends no credential ID from `connectDiscoveredHost`. Merely changing Rust lookup will not enable saved reconnect.
- `erd-net/src/discovery.rs` carries a published host address as a string. `validate_resolved_service` splits off `%scope`, parses `IpAddr`, then applies an IPv6 predicate that rejects link-local addresses regardless of scope. It subsequently normalizes through `ip.to_string()`, also losing any zone.
- The review plan identifies Apple address selection and `decide_service_state_action` as the parser-to-publication path. Follow current definitions rather than the plan's old line numbers.

## Shared contract to implement first

Read the opening lead amendments in `.omo/pairing-20260910/contracts.md`.

1. Keep the existing client and host record types and role separation. Add only the agreed optional client endpoint metadata, with backward-compatible serde defaults. Preserve old records without pretending they are already bound to an authenticated endpoint.
2. Carry `pairingId` separately from transport host/ports in the existing flat desktop/iOS command arguments. Missing selected ID is not permission to select a similarly named record or bootstrap.
3. An explicit stored-ID request loads exactly that record. A failed stored reconnect preserves its error and never tries a default PIN. A fresh pairing requires explicit valid PIN input.
4. Store/update the successful endpoint on the selected/new record after successful authentication. Endpoint aliases are connection hints, not proof that a discovered service is trusted.
5. Use the public key-free metadata response for credential selection. Never expose storage records or Keychain key bytes to JS.
6. Keep structured error codes/stages consistent across both shells. Do not label every generic TLS/network failure as a revoked credential, or branch on prose substrings.

Adding optional Rust struct fields still requires updating literal constructors. The common metadata owner must perform the mechanical initializer changes across affected fixtures before dependent shell workers start. Do not weaken tests while making them compile.

## Suggested phase-scoped ownership graph

- Common metadata/command-error contract producer: client store types, serde/backend regressions, exact lookup and successful endpoint persistence seams, plus necessary literal updates.
- Scoped-discovery producer in parallel: `erd-net` discovery validation/normalization and Apple publication/resolver path, with corresponding tests. No credential lookup or UI edits.
- Desktop identity producer after common contract: exact-ID command/UI flow, public credential selector, same-name untrusted discovery behavior, existing authentication helper/R11 test integration.
- iOS identity producer after common contract: explicit-ID native/Keychain path, public metadata command, PIN-free UI saved reconnect and restart behavior. Preserve the lifecycle-lock contract for phase D.
- Independent verifier after all producers: run actual identity/discovery targets and JS suites; report only executed commands and actual selected cases.
- Coordinator manually checks the real desktop/mobile flows and reviews the combined result before committing accepted increments.

Do not let metadata initializer edits race with dependent shell changes. Use Gemini 3.8 Flash for every producer/follow-up and the existing invocation route; never change model settings.

## R10 acceptance boundaries

Preserve scope as endpoint data through selection, validation, normalization, publication and actual resolver input. Do not simply allow every link-local `IpAddr` or fix the formatter alone.

- Valid TXT/SRV plus only `fe80::1` with scope 5 must reach `Publish`.
- Missing scope and scope 0 remain rejected.
- Valid IPv4 priority remains intact.
- The published address and actual `ToSocketAddrs`/client resolver result preserve scope.
- Cover numeric scope and any supported interface-name form according to existing platform resolver behavior, without inventing address aliases or hardcoded testbed trust.

Where Apple FFI prevents native Linux execution, test the shared pure decision path on Omarchy and verify the real Apple path through the physical-iOS build/runtime exception. No simulators, and no non-iOS compilation on the Mac.

## Evidence and unresolved concurrent work

Pin exact new regression names and RED invocations in the append-only notepad before executing them. Capture `identity-*` evidence for same-name rejection, exact selected ID, persisted metadata migration, restart/PIN-free reconnect, missing credential without fallback, scoped publication and real resolver scope. A successful key-free DTO test alone is not reconnect proof.

Keep `.omo/pairing-20260910/reports/concurrent-host-policy.md` in scope awareness. Another change adds automatic shell hosting with fixed PIN and automatic approval; ownership/policy clarification is pending. Preserve those controls and their `serve_with_stop` dependency until resolved, and do not include them in an identity commit by accident. This remains a separate tracked R11 blocker.

Preserve existing unrelated working-tree changes. Use only the isolated Omarchy workspace for Rust check/test/build, with the physical-iOS Mac exception. No publishing or permanent deployment. Final full-goal personal review remains the coordinator's last node after phase E.
