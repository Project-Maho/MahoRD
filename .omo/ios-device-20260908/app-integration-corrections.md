# Runtime integration corrections

Continue the same Gemini 3.8 Flash app/runtime session. Keep your original
ownership boundaries. Native VideoToolbox independently compiled successfully
for `aarch64-apple-ios`. The first whole-app check hit a host sccache file-descriptor
limit before producing a full compiler verdict; the lead is addressing that.

Code review found concrete incomplete behavior in the delivered runtime:

1. **Audio is not initialized.** `connect_async` contains an "Audio setup is
   optional here" comment but never constructs CpalAudioOutput, so audio_output
   remains None and audio_samples_played stays zero. Implement actual iOS output
   using existing AudioQueue/CpalAudioOutput and callback statistics. Initialize
   the iOS audio session appropriately, own the stream on a dedicated worker if
   CPAL's stream is not Send, keep it alive until stop, and drop/join correctly.
   Do not report queued samples as played samples. No success fallback when
   initialization actually fails. Keep platform errors visible.
2. **Avoid plaintext pairing persistence.** `ClientSession::pair_with_pin`
   persists via its PairingStore before your separate Keychain save. Check the
   full call path. Make the iOS PairingStore genuinely Keychain-backed, or provide
   a real non-persisting bootstrap store for this API. Do not leave the key in an
   Application Support JSON file and claim Keychain-only persistence. Unify the
   QA import and reconnect lookup with the same actual store. Preserve desktop
   persistence semantics and all existing tests.
3. **Trackpad units.** UI coordinates are normalized 0..1 and your TouchGestureHandler
   has a unit viewport. Host injectors handle InputEventType::RelativeMove using
   scroll_dx/scroll_dy rounded to PIXELS, not normalized x/y. Trace actual current
   protocol and handler definitions (not old aliases), and scale the emitted
   relative displacement using the current remote video dimensions before send.
   Keep direct-touch inverted-Y mapping unchanged. Add a regression proving that
   a meaningful normalized drag produces actual pixel movement.
4. **Cancellation validation.** handle_touch rejects nonfinite x/y before it
   reaches the handler even for Cancelled. The handler intentionally supports
   cancelling the held owner with invalid final coordinates. Preserve that
   recovery at the app boundary; reject invalid Began/Moved, not release signals.
5. **No blocking on async/UI threads.** commands::disconnect is async but calls
   disconnect_sync directly, which joins worker threads and stops TCP. connect_async
   also calls disconnect_sync inline. Move blocking teardown/input I/O off Tokio
   and the UI thread. Preserve command ordering and prevent concurrent connects
   or a cancelled pre-handshake task from reviving a later session.
6. **Generation isolation.** An active decoder thread can currently keep updating
   cloned counters/latest_frame/error after cancellation. Ensure old workers are
   completely joined before new session state is reused, and keep generation
   guards around all asynchronous completion/state writes. Do not discard join
   errors or silently recover a poisoned state mutex as healthy.
7. **Device stream telemetry.** Decoder/UDP failure must transition to a visible
   error/stop state instead of merely changing a string while stats reports Ready
   indefinitely. First presented-frame reporting should be tied to a real current
   decoded sequence, not accept arbitrary future IDs as evidence.

Read the exact current source/API before edits. Do not invent methods on
ClientSession/MediaEvent/AudioQueue. The lead will send actual compiler errors
separately. Keep the agreed frontend command payloads unchanged.

Write deterministic regressions first at pure/state seams and retain all prior
coverage. No builds, tests, device queries, simulators or emulator actions by you.
No unsafe Send/Sync to bypass type errors. No generic framework or unrelated
desktop refactor. Save the completed patch and a corrected app-handoff.md which
lists anything not yet implemented truthfully, then stop.
