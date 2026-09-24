# Lane: app-session

## Scope reviewed

All eight scoped files were read in full, including their inline tests:

| File | Lines read |
| --- | ---: |
| `clients/rust/maho-app/src/session.rs` | 1-1734 (1734) |
| `clients/rust/maho-app/src/media.rs` | 1-584 (584) |
| `clients/rust/maho-app/src/frame_queue.rs` | 1-339 (339) |
| `clients/rust/maho-app/src/receiver_stats.rs` | 1-664 (664) |
| `clients/rust/maho-app/src/receiver_trace.rs` | 1-154 (154) |
| `clients/rust/maho-app/src/latency.rs` | 1-616 (616) |
| `clients/rust/maho-app/src/error.rs` | 1-222 (222) |
| `clients/rust/maho-app/src/clipboard.rs` | 1-285 (285) |

Necessary directly imported implementations:

| File | Lines read |
| --- | ---: |
| `clients/rust/maho-proto/src/media.rs` | 1-320 (320 of 386) |
| `clients/rust/maho-net/src/udp_gcm.rs` | 160-289 (130 of 657) |
| `clients/rust/maho-app/src/audio.rs` | 1-94 (94) |
| `clients/rust/maho-app/src/abr.rs` | 1-64 (64) |

This is a static, read-only review. The supplied successful build/test baseline was accepted without rerunning it. Interleavings and packet sequences below are code-derived counterexamples, not claims of executed integration tests.

## Findings

### [P1] TCP runtime discards stream-configuration results
- **Location**: `clients/rust/maho-app/src/session.rs:1251` (secondary: `clients/rust/maho-app/src/session.rs:1112`)
- **Evidence**:
```rust
            Ok(
                SessionEvent::Ignored
                | SessionEvent::Frame(_)
                | SessionEvent::Audio(_)
                | SessionEvent::Cursor(_)
                | SessionEvent::StreamConfig(_)
                | SessionEvent::InputAck { .. },
            ) => {}
```
```rust
                msg @ (ControlMessage::StreamConfigResponse(_)
                | ControlMessage::StreamConfigReject(_)
                | ControlMessage::StreamConfigError(_)) => Ok(SessionEvent::StreamConfig(msg)),
```
- **Impact**: After `spawn_tcp_runtime` becomes the sole TCP reader, a response, rejection, or error for `request_stream_config` is decoded successfully and then silently discarded. A caller waiting on `SessionRuntime::events()` cannot learn whether its request succeeded or failed. Input acknowledgements are also absent from that event stream, although their separate latest-value accessor partially mitigates that case; no equivalent stream-configuration result accessor exists here.
- **Fix**: Retain and deliver stream-configuration results through `RuntimeEvents`, with a bounded queue keyed or correlated by request ID. Update `take` and both wait predicates to recognize these results; do not coalesce distinct outstanding request results into the clipboard/ping slots. Either deliver input-ack events too or explicitly make their separate accessor the documented runtime contract.
- **Confidence**: high

### [P1] Clipboard polling races remote-write echo suppression
- **Location**: `clients/rust/maho-app/src/clipboard.rs:134` (secondary: `clients/rust/maho-app/src/clipboard.rs:176`, `clients/rust/maho-app/src/clipboard.rs:143`)
- **Evidence**:
```rust
    pub fn apply_remote(&self, text: &str) -> Result<(), ClipboardError> {
        let change_count = self.clipboard.set_text(text)?;
        self.synchronizer
            .lock()
            .map_err(|_| ClipboardError::Unavailable("clipboard state lock poisoned".to_owned()))?
            .record_remote_write(text, change_count);
        Ok(())
    }
```
```rust
            let Ok(snapshot) = clipboard.snapshot() else {
                continue;
            };
            let type_refs = snapshot
                .types
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let decision = synchronizer.lock().ok().map(|mut state| {
                state.poll(
                    snapshot.change_count,
                    snapshot.text.as_deref().unwrap_or_default(),
                    &type_refs,
                )
            });
```
- **Impact**: The background worker can snapshot and classify the new remote text after `set_text` but before `record_remote_write`; it then calls `on_local_change` with remote-origin text instead of suppressing the echo. The opposite ordering also fails: a worker can capture old local text, let `apply_remote` record the newer remote write, and then classify the stale snapshot as a fresh local change, sending obsolete clipboard content back to the host. The `PlatformClipboard: Send + Sync` contract and the built-in worker make these ordinary concurrent operations.
- **Fix**: Serialize the entire clipboard snapshot-plus-classification operation and the entire remote set-plus-record operation with the same synchronizer/operation mutex, including `poll_once`. Acquire it before platform I/O on every path and release it before invoking `on_local_change`.
- **Confidence**: high

### [P2] Repeated frame headers erase already received chunks
- **Location**: `clients/rust/maho-app/src/media.rs:97`
- **Evidence**:
```rust
        let (chunks, started) = self.orphans.remove(&header.frame_id).map_or_else(
            || (BTreeMap::new(), now),
            |orphan| (orphan.chunks, orphan.started),
        );
        let frame_id = header.frame_id;
        let started = self
            .frames
            .get(&frame_id)
            .map_or(started, |assembly| assembly.started.min(started));
        let assembly = FrameAssembly {
            header,
            chunks,
            started,
            timestamp_ms,
        };
        self.frames.insert(frame_id, assembly);
```
- **Impact**: For a two-chunk frame, the sequence header, chunk 0, repeated header, chunk 1 never completes: the second header replaces the existing chunk map with an empty orphan/default map. The same applies when initial chunks arrived as orphans before the first header. This concerns a peer retransmitting a header in a newly authenticated datagram, not an identical encrypted datagram replay, which the cipher rejects. The retained original start time further shortens the already damaged frame's remaining assembly window.
- **Fix**: Make an identical header idempotent: preserve the existing assembly and its chunks/start time, updating only explicitly permitted metadata. Reject conflicting headers for an existing frame ID instead of silently replacing an in-progress frame.
- **Confidence**: high

### [P2] Orphan promotion bypasses chunk-index validation and the per-frame chunk bound
- **Location**: `clients/rust/maho-app/src/media.rs:97` (secondary: `clients/rust/maho-app/src/media.rs:182`, `clients/rust/maho-app/src/media.rs:261`)
- **Evidence**:
```rust
        let (chunks, started) = self.orphans.remove(&header.frame_id).map_or_else(
            || (BTreeMap::new(), now),
            |orphan| (orphan.chunks, orphan.started),
        );
```
```rust
        orphan.chunks.entry(chunk.chunk_index).or_insert(chunk.data);
```
```rust
    fn complete(assembly: &FrameAssembly) -> bool {
        assembly.chunks.len() == assembly.header.total_chunks as usize
    }
```
- **Impact**: Headerless chunks accept arbitrary `u16` indices, and promotion never checks them against the newly declared `total_chunks`. For example, orphans at indices 2 and 3 followed by a one-chunk header and valid chunk 0 remain permanently over the equality-based completion threshold and time out instead of rejecting the invalid input. More importantly, a peer can preload 8192 distinct out-of-range orphan indices, promote them with `total_chunks = 8191`, then append all 8191 valid indices: that single assembly retains 16383 chunks, defeating the apparent 8192-chunk bound. At the wire decoder's 1382-byte chunk limit this is about 21.6 MiB of payload for one incomplete frame, even if its declared total size is one byte. Frame-count limits and expiry still bound overall retention; this is not a claim of unbounded growth or a demonstrated remote crash.
- **Fix**: Reject orphan indices at or above `MAX_CHUNKS_PER_FRAME` before storage. On promotion, validate every retained index against the actual header, reject inconsistent orphan sets, and enforce a checked cumulative payload-byte budget against the declared size before retaining more data. Completion must require exactly the valid declared index set.
- **Confidence**: high

### [P2] Assembler frame ordering breaks across u32 wrap
- **Location**: `clients/rust/maho-app/src/media.rs:303` (secondary: `clients/rust/maho-app/src/media.rs:85`)
- **Evidence**:
```rust
    fn track_loss(&mut self, frame_id: u32, now: Instant) {
        let lost = self
            .expected_frame_id
            .map_or(0, |expected| frame_id.saturating_sub(expected) as u64);
        self.recent_loss.push_back((now, lost, lost + 1));
        self.expected_frame_id = Some(
            self.expected_frame_id
                .map_or(frame_id.wrapping_add(1), |expected| {
                    expected.max(frame_id.wrapping_add(1))
                }),
        );
        self.trim_loss(now);
    }
```
```rust
                let keep = *frame_id >= header.frame_id || Self::complete(assembly);
```
- **Impact**: Observing IDs `u32::MAX - 1`, `u32::MAX`, and `0` leaves `expected_frame_id` stuck at `u32::MAX` because numeric `max` rejects the wrapped successor. Subsequent missing headers, such as receiving 2 without 1, are recorded as zero loss. `ClientSession::evaluate_abr` consumes this value, so actual loss stops driving bitrate reduction after wrap. Numeric keyframe pruning is likewise inverted at the boundary: a reordered pre-wrap keyframe header can delete an already assembling post-wrap frame, while keyframe 0 fails to prune incomplete pre-wrap frames.
- **Fix**: Use half-range serial-number comparisons and wrapping distances, as `FrameQueue` already does. Advance the expected ID only for forward serial progress, and use the same serial ordering when pruning assemblies older than a keyframe.
- **Confidence**: high

### [P2] Explicit-clock UDP ingress mixes replay time with wall execution time
- **Location**: `clients/rust/maho-app/src/session.rs:1067` (secondary: `clients/rust/maho-app/src/session.rs:608`)
- **Evidence**:
```rust
                if let Some(started_at) = state.frames.take_completed_started_at() {
                    state
                        .receiver
                        .record_assembly(started_at, std::time::Instant::now());
                }
```
- **Impact**: `receive_udp_event_at` supplies the receive clock used for assembly start, expiry, and packet statistics, but assembly completion samples the process's real current time. A deterministic replay using future logical instants produces a reversed interval that `record_assembly` silently omits; a replay using past instants records time since that artificial epoch instead of its fragment-arrival interval. Assembly telemetry therefore changes with execution speed even when the supplied packet timeline is identical.
- **Fix**: Preserve the explicit-clock override through packet handling and use a completion time from that same clock. For replay ingress, use the completing packet's supplied receive instant; for ordinary ingress, retain a real completion-time sample if measuring local assembly processing is intended. Do not subtract a supplied logical start from an unrelated real-time end.
- **Confidence**: high

## Non-findings checked

- `FrameQueue` admits at most four pending frames; overflow clears the whole dependent chain rather than passing a P-frame whose queued reference was dropped.
- `FrameQueue` uses `wrapping_sub` and the half-range rule correctly for duplicates, late frames, `u32::MAX -> 0`, and the ambiguous half-range boundary.
- Recovery suppresses dependent frames and coalesces keyframe requests while recovering; a subsequently admitted keyframe resumes delivery. `decode_failed` preserves an already queued independent keyframe and its successors.
- `FrameQueue::stop` clears retained frames and notifies all condition-variable waiters; receive predicates inspect queue/stop state under the same mutex.
- Known-header duplicate chunk indices use `entry(...).or_insert(...)`, so duplicates neither replace the first payload nor falsely advance the distinct-chunk count.
- UDP frame headers/chunks reach the assembler only after successful authenticated decryption; a repeated encrypted datagram is rejected by replay protection before reassembly.
- The wire decoder caps video chunk payloads at 1382 bytes, frame headers at 8192 chunks and 32 MiB, and the assembler additionally rejects zero chunk counts and zero declared sizes.
- The assembler uses `u16` keys and a bounded range for concatenation, not incrementing an attacker-controlled `u16` index; no reachable chunk-index arithmetic panic was found.
- Incomplete-frame/orphan maps have frame-count limits and one-second activity-driven expiry. Loss history has a 4096-entry cap; the orphan-promotion finding is a validation/budget bypass, not evidence that these maps grow without bound.
- Concatenated frame length is checked before emitting `AssembledFrame`, preventing a mismatched declared size from being delivered as a successful assembly.
- `LatencyRecorder` normalizes capacity zero to one, saturates duration conversion and lifetime counts, handles empty samples before percentile indexing, and bounds percentile ranks.
- Receiver packet-loss division is guarded by a nonzero expected count; throughput requires a positive observed interval. UDP byte sums are bounded by the 4096-entry ring and actual datagram sizes.
- Host timestamp stages use checked subtraction, and host-ID deduplication plus the telemetry rings are bounded. Huge sequence gaps are accounted arithmetically rather than allocating one entry per missing packet.
- Receiver trace storage is capped at 65536 records; overflow and nanosecond conversion saturate, and receive/decode recording does not write files.
- `IpcErrorCode`/`IpcErrorStage` serialization retains their machine categories, including the explicit `tls-psk` spelling, and `IpcError` retains code, stage, message, and retryability. `SessionError` keeps typed protocol/TLS/datagram causes. Media/clipboard-to-I/O conversion does stringify the underlying cause, but no category-dependent recovery consumer was established in this allowed scope, so lossiness alone was not escalated into a production-behavior finding.
- Clipboard concealment checks precede outgoing text creation, oversized text is rejected before cloning for `Send`, and the ordinary non-racing remote-write path suppresses its echo.
