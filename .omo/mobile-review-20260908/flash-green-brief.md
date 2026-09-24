# Gemini 3.8 Flash implementation task

Continue the mobile code review task using the selected Gemini 3.8 Flash model only. The lead will invoke this brief only after observing RED. Do not delegate or change models.

## Scope

Read `docs/mobile-client-review-20260908.md`, the current mobile source and the independent regression tests. Implement the review's current-fix scope in `clients/rust/erd-mobile` only. Preserve the existing dirty workspace. Use read before editing and apply_patch for all edits.

1. **Bridge:** Handle allocation is not a network handshake. Keep handle allocation usable, but eliminate the fake connected state and discarded-success input path. A valid unconnected handle cannot send successfully; use an explicit status appropriate to an unavailable backend. Reject empty/whitespace-only and invalid UTF-8 hosts. If the output pointer is valid, set it to null before any later validation can fail. Never dereference arbitrary invalid pointers to "check" them. Document `# Safety` contracts for C strings, writable output slots, exclusively owned live handles, and exactly-once destruction. Use explicit unsafe blocks with meaningful SAFETY comments. Preserve existing enum discriminants; add a status only if needed. Do not implement a pretend connection, silently use HTTP port 19735, or start a real remote session.
2. **Native media:** These are placeholders, not native codecs. Return typed backend-unavailable errors rather than counting arbitrary bytes as decoded/rendered/played. Preserve existing input/configuration validation. Ensure all publicly constructible/default audio paths also cannot report output or set a fake running state. Zero-return no-output methods may retain their return type; initialization/start methods should make unavailability explicit. If signatures change, update every actual caller and test in this crate. Do not write Android/iOS FFI or a new app shell in this correction task.
3. **Touch:** One active pointer owns remote mouse state; secondary IDs and unknown/duplicate Ended cannot issue someone else's down/up. Reject nonfinite input in both modes before state mutation. Failed Began/Moved must preserve active state and the last valid relative baseline. Finite arithmetic must not produce nonfinite deltas or host coordinates. Cancel a held direct touch using the last successfully emitted host coordinates, even if platform coordinates or the mutable viewport became invalid. Do not recalculate release coordinates from corrupted mutable viewport state. A mode change during a held DirectTouch drag must return one release event to its caller; changing to the same mode must preserve the gesture. Add deterministic coverage for this small API change before the production change; state clearly that this API initially fails to compile against the old signature. Do not manufacture multi-touch pinch/scroll support. Prefer one `Option` for the pointer owner over an unbounded collection if only one is actually used.
4. **Viewport:** Validate finite dimensions, zoom and offsets at its public boundary; validate zoom center and all calculated values before committing updates. Invalid input does not mutate state. Keep the established inverted-Y wire convention. Pan must not introduce infinities through finite arithmetic overflow; retain no-op behavior for rejected pan if preserving its current void API.
5. **Power:** Fair thermal adjustment must not exceed the previously computed bitrate cap and must not overflow for u32::MAX. Preserve ordinary defaults, battery/data caps and other thermal states. Use a small overflow-safe expression, no speculative policy framework.

## Tests and boundaries

- Retain all independent regression coverage and do not weaken assertions to get green.
- Existing unit tests that assert fake backend output must be corrected to assert explicit unavailability and no counters advancing; explain this semantic change. Do not delete or skip them.
- Keep tests deterministic: no sleeps, polling, external services, native devices or emulators.
- No changes outside `clients/rust/erd-mobile`, except a short implementation handoff at `.omo/mobile-review-20260908/flash-implementation.md`.
- No dependency changes, no Cargo workspace/lockfile changes, no unrelated formatting/refactors, no commits, no local/remote builds, no deployments, no device queries or actions. The lead owns Omarchy compilation.
- Load the Rust programming reference and the Rust unsafe reference set before editing the FFI file. If tools for Miri/compilation are unavailable, report that honestly; do not claim a run.
- Do not add broad panic catchers, pointer registries, feature fallbacks, trait hierarchies or mocks that conceal the missing platform integration.

## Deliverable

The scoped production patch with updated/additional tests. In the handoff, list touched files, public API changes, specific invariants preserved, and remaining product-level blockers F3/F4/F8. Explicitly say that real mobile networking/native media and APK/IPA packaging remain unimplemented; these fixes do not make a working mobile app.

Stop after saving the patch and handoff. The lead will compile, test and review the exact changes and return concrete failures if any.
