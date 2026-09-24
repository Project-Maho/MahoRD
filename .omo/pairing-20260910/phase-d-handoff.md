# Phase D: lifecycle implementation handoff

## Coordinator review correction: native acceptance withdrawn

The first native producer report is NOT accepted. The coordinator read actual source/tests and found: cancellation does not interrupt a locally held TLS socket or a handshake read holding the TCP mutex; its test sleeps 5 ms against fixed ports and accepts any error; shutdown ignores stop/join errors; startup may lose worker handles and return ready after terminal failure; tests mutate global HOME/XDG despite store injection; and a new Clippy suppression was added.

`native-lifecycle` was revived through workflow send on `dag_32be32cb-6687-441b-aa9a-73dd233a5a77`. The attempted `amend` was refused while the run remained active; `phase-d-v2.json` records the desired correction prompt, not a successfully applied graph generation. The native worker is now additionally authorized to change the minimal cancellation seams in `erd-app/src/session.rs` and their direct tests. No unrelated core changes are permitted.

The independent verifier MUST read the `LEAD REVIEW CORRECTIONS` section in `phase-d-v2.json` and re-check every finding against the corrected code and new event-driven tests. Do not accept the first native report's green counts or run a final integration gate while native follow-up edits are in progress. If scheduling starts verification before the revived worker settles, report that dependency as pending; the coordinator will resume verification once the actual changes settle.

## Entry evidence and remaining gates

Phase C workflow `dag_22b672bb-b828-4928-9ed3-123b076d184b` is settled at generation 2. Its shared credential/error types and desktop/iOS exact-ID implementation passed the coordinator's scoped Rust and current-source UI checks. Physical-device, visual, commit and concurrent hosting-policy acceptance remain OPEN. Starting D is not acceptance of C; section 5 of the review plan explicitly permits C/D integration in parallel after interfaces are agreed.

Do not repeat A/B or redesign the established identity contract. Preserve optional flat `pairingId`, key-free metadata, explicit PIN authorization, authenticated UDP registration, exact selected IDs and endpoint persistence. Do not modify unrelated desktop hosting, capture, rendering or input improvements.

## Topology and boundaries

One phase-scoped workflow with four nodes:

1. `native-lifecycle`: R5 plus native cancellation/cleanup ownership in `ios-shell/src/state.rs`, `commands.rs`, `lib.rs` and native tests. This is indivisible session/concurrency work; use hephaestus with the existing invocation-only Gemini 3.8 Flash route.
2. `ui-lifecycle`: R6 in `ios-shell/ui/connection-state.js` and its state/discovery tests. Independent of native implementation, using the contract below. Same explicit Flash route for behavioral implementation.
3. `connecting-modal`, after `ui-lifecycle`: R7 in `ios-shell/ui/app.js`, `index.html`, `styles.css`, and dedicated real-page harness/tests. Depends on the actual finished UI state contract. Same explicit Flash route with frontend/visual QA skills.
4. `verify-lifecycle`, after all producers: independent verification and report only, using exact named tests and source hashes, not implementation assertions.

The coordinator owns integration decisions, device QA, source/version accounting, commits and the final direct review. No worker commits or installs/deploys applications.

## Contract

- The native owner consumes TCP terminal events and routes TCP, audio/media failure, disconnect and cancellation through one generation-aware shutdown. It must stop/reap its owned workers and runtime, release held input and retain terminal reason.
- Cancellation must signal pending native connect without waiting behind the lifecycle lock that connect owns. Keep serialization for ownership transfer; do not simply remove the lock. A stale completion cannot install its session over a newer generation.
- Existing JS native commands remain `connect`, `disconnect`, `stats`; add fields only where necessary. A JS disconnect during pending connect must trigger native cancellation promptly, await that connect's settlement and cleanup before permitting another connect.
- JS ownership is independent of display phase. Error/dismiss paths still clean up owned native resources. Concurrent cleanup callers share one pending operation. Failed cleanup remains `cleanup-failed`, keeps the connection lock and offers retry; never report idle or permit another connect until cleanup succeeds.
- Preserve original connection/terminal errors alongside any cleanup error. Existing structured errors use `erd_app::IpcError`; no message substring branching and no automatic bootstrap.
- Connecting/disconnecting UI is a top-level modal independent of hidden page views. Cancel stays reachable during connect; during cleanup, show progress and prevent duplicate actions. Trap/restore focus and block background input. Clear the PIN field after submission, never retain it in state snapshots/logs.
- Discovery failure must not disable direct connect; saved credentials stay separate from untrusted discovery.

## Failing-first evidence

Append exact regression names, literal commands, expected RED reasons and artifact paths to the existing notepad BEFORE each new RED action:

`/var/folders/zh/7cc25lt91b1_dj577306nwdh0000gn/T/ulw-20260910-111421.XXXXXX.md.pNqwGP19PH`

Use apply_patch to append after reading the current end, respecting other appenders. Existing R7 RED scenario and evidence are in `evidence/r7-scenario.md` and `evidence/r7-red-results.md`. They prove hidden-ancestor/zero-rect failure, not physical touch.

Native tests must use the production terminal-consumer/owner path and real loopback authenticated TCP session where required. Subscribe before closing the server side; prove worker exits with completion events and bounded timeouts, not sleeps or polling. Cover late generation events, connect cancellation, audio error cleanup and duplicate termination.

UI tests must load the real module and preserve deferred invoke behavior; cover error disconnect/dismiss, shared cleanup, rejected cleanup, pending connect cancellation and stale completion. Page tests must use the actual UI with a bridge only at IPC. Exercise `#connect-host`, `#btn-connect-submit`, `#btn-cancel-connect` by real browser actions at 430x932 and 1280x800. Capture screenshots, action logs, focus/hit-testing and cleanup receipts. Never count fixture TLS/frame success as native or physical proof.

## Execution constraints

Gemini 3.8 Flash invocation route: `quotio/gemini-3.8-flash-high`, as used by the completed C run. Never edit model settings.

Rust check/test/build exclusively on `indo@100.91.254.71` in `/home/indo/projects/erd-pairing-20260910`, with `PKG_CONFIG_PATH=/home/indo/erd-ffmpeg7/lib/pkgconfig` and `LD_LIBRARY_PATH=/home/indo/erd-ffmpeg7/lib`. Sync only owned changed source paths; never broad-sync a stale tree over another worker. Physical-iOS-target compilation/signing alone has the Mac exception. No simulator/emulator.

Use monitors for builds/tests; no wait children or foreground polling. Use apply_patch for edits and read for inspection. Do not suppress failures or weaken tests. Preserve all pre-existing work. No publishing, permanent deployment, production host restarts or public messages.

Artifacts: `.omo/pairing-20260910/reports/d-native.md`, `d-ui.md`, `d-modal.md`, `d-verify.md` and `evidence/lifecycle-*`. Each report must distinguish actual RED/GREEN, direct surface evidence, unavailable checks and outstanding physical-device acceptance. Stop each node when its scoped implementation and evidence are delivered, not after claiming the overall goal complete.
