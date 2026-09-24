# UI lifecycle correction

Continue the same Gemini 3.8 Flash UI session. Initial 20 pure JS tests passed
on Omarchy. Code review found a real lifecycle wiring defect in `ui/app.js`:

- On document.hidden, current code releases input and stops frame polling but
  does not call native disconnect. The network/decoder/audio session continues
  in the background, contrary to the agreed iOS lifecycle contract.
- The same handler is bound to window.blur. A transient blur while the document
  remains visible stops frame polling, but there may be no visibilitychange
  event to restart it. A native permission prompt can therefore freeze video.

Own only `clients/rust/ios-shell/ui/**` and DESIGN.md. Write a deterministic
regression at the lifecycle event-to-action seam FIRST and stop after the test
stage. The lead must observe RED before the fix. Cover:

1. Hidden/pagehide during connecting or streaming invokes input release and
   actual disconnect, invalidates the pending generation and stops polling.
2. Blur while still visible releases held inputs but does NOT stop/disconnect
   the active stream.
3. Returning to foreground does not resurrect the disconnected session or
   accept stale connection/frame completions.

Use built-in EventTarget or a small injected event source if needed. Test the
actual lifecycle binding logic, not a parallel reimplementation or prose
source search. Avoid a whole fake DOM or new npm dependency. If the existing
IIFE needs a narrow testable module boundary, extract only that boundary while
preserving current behavior for RED; do not fix it yet.

No tests/builds/browser emulation/device actions by this executor. Use read and
apply_patch. Save the tests and report the exact RED command, then stop.
