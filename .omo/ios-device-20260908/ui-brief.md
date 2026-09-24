# iPhone frontend executor

You are the Gemini 3.8 Flash executor requested by the user. Read
`.omo/ios-device-20260908/contract.md`. Deliver ONE actual iPhone client UI under
`clients/rust/ios-shell/ui/**`, plus `clients/rust/ios-shell/DESIGN.md`.
Do not edit Rust, manifests, desktop UI, icons or generated Apple projects.

Reuse existing EclipticRD design authority:
`clients/rust/tauri-shell/DESIGN.md`. Read its tokens and actual existing
`ui/index.html` rendering/input code as reference, but do not copy desktop-only
window APIs or unsafe viewport assumptions. This is an adaptation of the existing
design system, not a greenfield marketing redesign. Read applicable frontend,
interaction, layout and visual QA skills; no new framework or dependencies.

Implement the exact Tauri command and raw NV12 frame contract from contract.md.
Use genuine WebGL2/WebGL NV12 shaders and actual binary buffer responses,
aspect-fit video, pointer ownership/serialization, safe-area layout, real error
handling, accessible touch controls and live counters. Distinguish connected,
waiting-for-video and actual rendered video. No synthetic data or mock native
success. Preserve local versus remote input scope and release on interruptions.

Connect form: host text, optional 8-digit pairing PIN. PIN not stored in
localStorage. Backend validates authoritatively. Startup may provide debug QA
host/auto_connect but never a pairing key. Call actual connect in that case so
physical-device QA exercises the same UI/backend path. Report presented sequence
only after actual GL drawing.

Add small deterministic JS tests for the highest-risk pure input/frame parsing
seams; do not introduce a whole DOM mocking framework or snapshots of prose.
Do not run builds/tests, browser device emulation, simulators or physical device
commands. The lead owns all validation. Use read and apply_patch for edits.

Save `.omo/ios-device-20260908/ui-handoff.md` with file list and UI/test contract
notes, then stop.
