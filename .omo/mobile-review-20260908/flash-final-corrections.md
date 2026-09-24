# Final corrections after code review

Continue with Gemini 3.8 Flash only. Read current files before using apply_patch.
Do not run compilation, tests, devices or emulators. No dependency, workspace,
lockfile or unrelated source changes.

1. The lead observed 27 unit tests and 28 integration regressions passing. New
   cancellation coverage now exposes a relative-mode ownership bug: Cancelled
   must clear the matching owner's baseline even with NaN/Infinity coordinates.
   Handle cancellation for both modes before coordinate validation; unknown or
   secondary cancellation must not clear the primary.
2. Fix Clippy without suppressions: collapse the nested relative Began condition
   in touch.rs; construct Android/iOS test configs with struct update syntax
   instead of field assignment after Default. This also applies to the new
   integration test fixtures. Keep all assertions.
3. Remove the unused unsafe dereference of the handle in send_touch. There is no
   backend and no field is needed; validating event_type with is_err is enough,
   without binding and discarding evt_type. Keep documented live-handle ownership
   requirements and all status discriminants.
4. Use overflow-safe u32 quotient/remainder arithmetic for the 4/5 bitrate scaling
   rather than an unchecked narrowing cast. Preserve the same floor-and-cap
   policy and expected regression values.
5. The enlarged touch implementation duplicates full mouse event constructors.
   Reuse a small private constructor where it actually removes repetition. Keep
   direct/relative ownership logic explicit. Do not introduce a generic event
   system, public trait or broad refactor.
6. Format only touched Rust files through apply_patch; the lead will check the
   result. No formatting changes to untouched storage/keyboard/lifecycle/lib.
7. Correct the handoff's finding labels: F3 is app packaging, F4 is desktop
   dependency leakage, F8 is secure storage/lifecycle/IME integration. Real
   networking is the remaining portion of F1 and native media of F2.

Stop once the scoped patch and corrected handoff are saved. Do not claim tests
ran in this session. The lead performs the final checks.
