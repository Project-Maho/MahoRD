# Lane: proto-wire

## Scope reviewed

All files below were read in full, including their inline tests (2,195 lines total):

- `clients/rust/maho-proto/src/packet.rs`: 116 lines.
- `clients/rust/maho-proto/src/framing.rs`: 209 lines.
- `clients/rust/maho-proto/src/codec.rs`: 152 lines.
- `clients/rust/maho-proto/src/media.rs`: 386 lines.
- `clients/rust/maho-proto/src/control.rs`: 647 lines.
- `clients/rust/maho-proto/src/input.rs`: 303 lines.
- `clients/rust/maho-proto/src/handshake.rs`: 152 lines.
- `clients/rust/maho-proto/src/pairing.rs`: 138 lines.
- `clients/rust/maho-proto/src/error.rs`: 59 lines.
- `clients/rust/maho-proto/src/lib.rs`: 33 lines.

Review method: static control-flow and malformed-byte tracing, not runtime fuzzing. All necessary internal imports are in the listed files. No transport consumers or reassembly implementation are imported by this scope, so downstream crashes, allocation behavior, and reassembly arithmetic are not claimed as verified. The supplied successful build/test baseline was not rerun. No source files were changed.

## Findings

### [P2] Video chunk indices outside every legal frame are accepted
- **Location**: `clients/rust/maho-proto/src/media.rs:154` (decode); `clients/rust/maho-proto/src/media.rs:129` (encode validation); `clients/rust/maho-proto/src/media.rs:6` (global chunk-count limit).
- **Evidence**:
```rust
pub const MAX_CHUNKS_PER_FRAME: u16 = 8192;
```
```rust
    fn validate(&self) -> Result<(), CodecError> {
        if self.data.len() > MAX_VIDEO_CHUNK_BYTES {
            return Err(CodecError::LengthLimit {
                field: "video chunk data",
                actual: self.data.len(),
                max: MAX_VIDEO_CHUNK_BYTES,
            });
        }
        Ok(())
    }
```
```rust
        let chunk_index = decoder.u16("frame chunk index")?;
        let data = decoder.take_remaining();
        if data.len() > MAX_VIDEO_CHUNK_BYTES {
            return Err(CodecError::LengthLimit {
                field: "video chunk data",
                actual: data.len(),
                max: MAX_VIDEO_CHUNK_BYTES,
            });
        }
        Ok(Self {
            frame_id,
            chunk_index,
            data: data.to_vec(),
        })
```
- **Impact**: The malformed seven-byte chunk `01 00 00 00 00 20 aa` decodes successfully as frame 1, index 8192, data `[0xaa]`. With at most 8192 chunks, the largest legal zero-based index is 8191; no valid frame can use this chunk. The same impossible chunk can be encoded successfully. The codec therefore passes invalid indexing metadata to reassembly instead of rejecting it at the wire boundary. This is a missing global bound, not a demonstrated downstream panic; a consumer may independently reject the index.
- **Fix**: Reject `chunk_index >= MAX_CHUNKS_PER_FRAME` in both decoding and encode validation, before copying data on decode. Reassembly must additionally check `chunk_index < header.total_chunks`; the codec can enforce the global limit without having a header.
- **Confidence**: high

### [P2] Frame headers can advertise more bytes than their chunks can carry
- **Location**: `clients/rust/maho-proto/src/media.rs:67` (header validation); `clients/rust/maho-proto/src/media.rs:8` (per-chunk byte limit).
- **Evidence**:
```rust
pub const MAX_VIDEO_CHUNK_BYTES: usize = 1382;
```
```rust
    fn validate(&self) -> Result<(), CodecError> {
        if self.total_chunks > MAX_CHUNKS_PER_FRAME {
            return Err(CodecError::LengthLimit {
                field: "frame chunks",
                actual: self.total_chunks as usize,
                max: MAX_CHUNKS_PER_FRAME as usize,
            });
        }
        if self.total_size > MAX_FRAME_BYTES {
            return Err(CodecError::LengthLimit {
                field: "frame bytes",
                actual: self.total_size as usize,
                max: MAX_FRAME_BYTES as usize,
            });
        }
        Ok(())
    }
```
- **Impact**: A complete 16-byte header with `total_chunks = 1` and `total_size = 1383` passes both encode and decode, although its only chunk is limited to 1382 bytes. So does a header with zero chunks and a positive total size. The independent 32 MiB size ceiling also permits headers larger than the 11,321,344 bytes that 8192 maximum-sized chunks can carry. Such a frame cannot be completed consistently with its advertised size; consumers must discover and discard the inconsistency later rather than relying on successful header validation. No unbounded wait or allocation exploit is asserted without reviewing the consumer.
- **Fix**: In `FrameHeader::validate`, additionally require `total_size <= u32::from(total_chunks) * (MAX_VIDEO_CHUNK_BYTES as u32)`, retaining the existing independent caps. Perform this multiplication in `u32` or a wider type, not `u16`. This also rejects a positive byte count with zero chunks without imposing an undocumented rule on completely empty frames.
- **Confidence**: high

### [P2] Bodyless control messages silently discard trailing payload bytes
- **Location**: `clients/rust/maho-proto/src/control.rs:561`.
- **Evidence**:
```rust
    fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let mut decoder = Decoder::new(input);
        let message_type = ControlMessageType::try_from(decoder.u8("control message type")?)?;
        let body = decoder.take_remaining();
        match message_type {
            ControlMessageType::RequestKeyFrame => Ok(Self::RequestKeyFrame),
            ControlMessageType::StartStream => Ok(Self::StartStream),
            ControlMessageType::StopStream => Ok(Self::StopStream),
            ControlMessageType::Disconnect => Ok(Self::Disconnect),
            ControlMessageType::Ping => Ok(Self::Ping),
            ControlMessageType::Pong => Ok(Self::Pong),
```
- **Impact**: `ControlMessage::decode(&[0x01, 0x03])` succeeds as `StartStream`, silently discarding the second byte, rather than rejecting a malformed complete control value. All six bodyless variants accept arbitrary suffixes, unlike the body-bearing variants whose subdecoders enforce complete consumption. A misframed or incorrectly concatenated control payload is thus accepted as an actionable command with the remainder lost. This is a strict-length validation defect, not evidence of an authentication bypass.
- **Fix**: Require an empty `body` for the six bodyless variants and otherwise return `CodecError::TrailingBytes`. For example, call `Decoder::new(body).finish("control message")?` in those arms. Calling `finish` on the original decoder after `take_remaining` would not fix this, because its offset already equals the input length.
- **Confidence**: high

## Non-findings checked

- Endianness: integer encoders and decoders, TCP prefixes, packet headers, frame metadata, audio headers, and timestamp fields consistently use little-endian bytes; floats use little-endian `u32` bit patterns.
- Shared indexing: `Decoder::take` compares the requested size with remaining bytes before advancing or slicing; its invariant `offset <= input.len()` prevents offset addition overflow and subtraction underflow on these paths.
- Packet header: a valid-magic 11-byte header fails reading flags; wrong magic and unknown packet types return errors, and a 13-byte header fails complete-consumption validation.
- Handshake: a declared name length of 1025 is rejected before allocation; a within-limit name longer than the remaining input returns `Truncated`; a 15-byte session salt fails before `copy_from_slice`; versions other than 3 and trailing bytes are rejected.
- HandshakeAck/Ping dispatch boundary: these packet-type tags are recognized, but this scope supplies no separate HandshakeAck/Ping payload dispatcher; no behavior of an out-of-scope dispatcher is inferred.
- FrameHeader: a 15-byte input fails the final `u32` read; a count of 8193 and a byte count above 32 MiB are rejected; the 8192 limit fits its `u16` wire field without truncation.
- FrameChunk: a five-byte input fails before reading the index; a chunk containing 1383 data bytes fails before `to_vec`; variable-length data is deliberately the complete remaining payload.
- CursorUpdate: an eight-byte input fails reading the cursor type; extra bytes are rejected. Arbitrary float bit patterns do not themselves cause slice panics in this codec.
- InputEvent: a 20-byte value fails reading the last float; unknown event tags and extra bytes are rejected. Packed gamepad IDs use a widened `u16` before shifting, so two `u8` IDs do not overflow the field.
- Control: empty input fails reading its tag; unknown tags fail conversion; truncated BitrateAdjust/InputAck/configuration/request/response bodies fail bounded reads; body-bearing control variants reject trailing bytes.
- Control strings: stream reject/error and clipboard error messages use bounded `u16` lengths and checked UTF-8 reads; a declared length exceeding remaining input errors before copying; clipboard update lengths above 4096 are rejected explicitly.
- Clipboard enum fields and color metadata: unknown direction/origin/range/matrix/chroma tags return typed errors; a two-byte color value fails before indexing a third byte.
- PairingRequest: a declared name length of 1025 is rejected, and a within-limit truncated or invalid-UTF-8 name returns an error rather than panicking.
- PairingGrant: ID and host-name lengths are checked against available bytes; a declared key length of 31 is rejected, and a declared length of 32 with only 31 bytes remaining fails before copying into the fixed-size key.
- PairingReject: empty input fails the bounded tag read; unknown reasons and extra bytes return errors.
- AudioFragmentHeader/AudioFragment: a seven-byte header fails bounded reads; count zero and index equal to count are rejected; fragment data above 1380 bytes is rejected before copying. No audio reassembly state is implemented here.
- Timestamp magic: `TIMESTAMP_STATS_MAGIC` is exactly `b"ERDTS1"`; decode requires exactly 42 bytes and uses short-circuit `||`, so even a zero-byte input returns `None` before `bytes[..6]` is evaluated. Wrong magic is rejected, and every subsequent timestamp slice is within the established 42-byte bound.
- TCP framing: fewer than four prefix bytes stay buffered without indexing; zero and lengths above 16 MiB emit `DroppedInvalidLength`; a valid incomplete frame waits without slicing past the buffer; payload slicing happens only after proving enough remaining bytes.
- TCP arithmetic: accepted lengths are at most 16 MiB, so `4 + length` fits the workstation's `usize`; `offset + frame_end` is bounded by the preceding remaining-length check. Coalesced complete frames advance monotonically and a partial suffix is compacted once per push.
- Reassembly arithmetic limit: the scoped code serializes counts and indices but contains no chunk accumulator or reassembly allocation; absence of overflow in these codecs is not proof of overflow safety in an out-of-scope reassembler.
- Inline tests: the read tests are deterministic, use no sleeps or polling, and their test-only `unwrap`/`expect` calls are not production findings.
