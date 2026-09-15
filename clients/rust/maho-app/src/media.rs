use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    time::{Duration, Instant},
};

use maho_proto::{CursorUpdate, FrameChunk, FrameHeader, MAX_CHUNKS_PER_FRAME, MAX_FRAME_BYTES};
use thiserror::Error;

/// Half of the u32 serial-number ring: wrapping distances below this mean "forward".
const SERIAL_HALF_RANGE: u32 = 1 << 31;
const MAX_ORPHAN_FRAMES: usize = 16;
const MAX_INCOMPLETE_FRAMES: usize = 16;
const MAX_LOSS_SAMPLES: usize = 4096;
const ASSEMBLY_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledFrame {
    pub header: FrameHeader,
    pub data: Vec<u8>,
    pub timestamp_ms: u32,
}

#[derive(Debug)]
struct FrameAssembly {
    header: FrameHeader,
    chunks: BTreeMap<u16, Vec<u8>>,
    started: Instant,
    timestamp_ms: u32,
    payload_bytes: usize,
}

#[derive(Debug)]
struct OrphanAssembly {
    chunks: BTreeMap<u16, Vec<u8>>,
    started: Instant,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MediaAssemblyError {
    #[error("frame header exceeds protocol caps")]
    InvalidHeader,
    #[error("frame chunk index is outside the declared frame")]
    InvalidChunkIndex,
    #[error("assembled frame size differs from its header")]
    SizeMismatch,
}

#[derive(Debug, Default)]
pub struct FrameAssembler {
    trace: crate::ReceiverTrace,
    frames: HashMap<u32, FrameAssembly>,
    orphans: HashMap<u32, OrphanAssembly>,
    expected_frame_id: Option<u32>,
    recent_loss: VecDeque<(Instant, u64, u64)>,
    completed_frames: u64,
    completed_started_at: Option<Instant>,
}

/// Serial-number ordering on the u32 frame-id ring: is `a` at or after `b`?
fn serial_not_before(a: u32, b: u32) -> bool {
    a.wrapping_sub(b) < SERIAL_HALF_RANGE
}

/// Admits chunks into an assembly, rejecting anything outside the declared
/// frame or past the declared payload size: headerless chunks and retransmits
/// must never push a single assembly past its per-frame bounds.
fn admit_chunks(
    chunks: &mut BTreeMap<u16, Vec<u8>>,
    payload_bytes: &mut usize,
    incoming: BTreeMap<u16, Vec<u8>>,
    header: &FrameHeader,
) {
    use std::collections::btree_map::Entry;
    for (index, data) in incoming {
        if index >= header.total_chunks || *payload_bytes + data.len() > header.total_size as usize
        {
            continue;
        }
        match chunks.entry(index) {
            Entry::Vacant(entry) => {
                *payload_bytes += data.len();
                entry.insert(data);
            }
            Entry::Occupied(_) => {}
        }
    }
}

impl FrameAssembler {
    pub fn set_receiver_trace(&mut self, trace: crate::ReceiverTrace) {
        self.trace = trace;
    }
    pub fn push_header(
        &mut self,
        header: FrameHeader,
        timestamp_ms: u32,
        now: Instant,
    ) -> Result<Option<AssembledFrame>, MediaAssemblyError> {
        self.completed_started_at = None;
        if header.total_chunks == 0
            || header.total_chunks > MAX_CHUNKS_PER_FRAME
            || header.total_size == 0
            || header.total_size > MAX_FRAME_BYTES
        {
            Self::trace_frame(
                &self.trace,
                now,
                header.frame_id,
                crate::ReceiverTraceEvent::AssemblyInvalid,
            );
            return Err(MediaAssemblyError::InvalidHeader);
        }
        self.expire(now);
        self.track_loss(header.frame_id, now);
        if header.is_key_frame {
            self.frames.retain(|frame_id, assembly| {
                let keep =
                    serial_not_before(*frame_id, header.frame_id) || Self::complete(assembly);
                if !keep {
                    Self::trace_frame(
                        &self.trace,
                        now,
                        *frame_id,
                        crate::ReceiverTraceEvent::AssemblyKeyframe,
                    );
                }
                keep
            });
        }
        let frame_id = header.frame_id;
        let (chunks, started) = self.orphans.remove(&frame_id).map_or_else(
            || (BTreeMap::new(), now),
            |orphan| (orphan.chunks, orphan.started),
        );
        // A retransmitted header must not discard chunks already collected for
        // this frame: replacing the assembly with an empty map means a frame
        // whose header arrives twice mid-stream can never complete. An
        // identical header is idempotent and refreshes only its metadata; a
        // conflicting header cannot be trusted to describe the in-progress
        // assembly and is rejected outright.
        if let Some(existing) = self.frames.get_mut(&frame_id) {
            if existing.header != header {
                Self::trace_frame(
                    &self.trace,
                    now,
                    frame_id,
                    crate::ReceiverTraceEvent::AssemblyInvalid,
                );
                return Ok(None);
            }
            existing.started = existing.started.min(started);
            existing.timestamp_ms = timestamp_ms;
            admit_chunks(
                &mut existing.chunks,
                &mut existing.payload_bytes,
                chunks,
                &existing.header,
            );
        } else {
            let mut payload_bytes = 0;
            let mut admitted = BTreeMap::new();
            admit_chunks(&mut admitted, &mut payload_bytes, chunks, &header);
            let assembly = FrameAssembly {
                header,
                chunks: admitted,
                started,
                timestamp_ms,
                payload_bytes,
            };
            self.frames.insert(frame_id, assembly);
        }
        let completed = self.finish_if_complete(frame_id, now)?;
        if self.frames.len() > MAX_INCOMPLETE_FRAMES {
            let oldest = self
                .frames
                .iter()
                .min_by_key(|(id, assembly)| (assembly.started, **id))
                .map(|(&id, _)| id)
                .expect("over-capacity frame map is nonempty");
            self.frames.remove(&oldest);
            Self::trace_frame(
                &self.trace,
                now,
                oldest,
                crate::ReceiverTraceEvent::AssemblyCapacity,
            );
        }
        Ok(completed)
    }

    pub fn push_chunk(
        &mut self,
        chunk: FrameChunk,
        now: Instant,
    ) -> Result<Option<AssembledFrame>, MediaAssemblyError> {
        self.completed_started_at = None;
        self.expire(now);
        if let Some(assembly) = self.frames.get_mut(&chunk.frame_id) {
            if chunk.chunk_index >= assembly.header.total_chunks {
                Self::trace_frame(
                    &self.trace,
                    now,
                    chunk.frame_id,
                    crate::ReceiverTraceEvent::AssemblyInvalid,
                );
                return Err(MediaAssemblyError::InvalidChunkIndex);
            }
            if !assembly.chunks.contains_key(&chunk.chunk_index)
                && assembly.payload_bytes + chunk.data.len() > assembly.header.total_size as usize
            {
                Self::trace_frame(
                    &self.trace,
                    now,
                    chunk.frame_id,
                    crate::ReceiverTraceEvent::AssemblyInvalid,
                );
                return Ok(None);
            }
            if !assembly.chunks.contains_key(&chunk.chunk_index) {
                assembly.payload_bytes += chunk.data.len();
            }
            assembly
                .chunks
                .entry(chunk.chunk_index)
                .or_insert(chunk.data);
            return self.finish_if_complete(chunk.frame_id, now);
        }

        if self.orphans.len() >= MAX_ORPHAN_FRAMES && !self.orphans.contains_key(&chunk.frame_id) {
            Self::trace_frame(
                &self.trace,
                now,
                chunk.frame_id,
                crate::ReceiverTraceEvent::AssemblyCapacity,
            );
            return Ok(None);
        }
        if chunk.chunk_index >= MAX_CHUNKS_PER_FRAME {
            Self::trace_frame(
                &self.trace,
                now,
                chunk.frame_id,
                crate::ReceiverTraceEvent::AssemblyInvalid,
            );
            return Ok(None);
        }
        let orphan = self
            .orphans
            .entry(chunk.frame_id)
            .or_insert_with(|| OrphanAssembly {
                chunks: BTreeMap::new(),
                started: now,
            });
        if orphan.chunks.len() >= MAX_CHUNKS_PER_FRAME as usize {
            self.orphans.remove(&chunk.frame_id);
            Self::trace_frame(
                &self.trace,
                now,
                chunk.frame_id,
                crate::ReceiverTraceEvent::AssemblyCapacity,
            );
            return Ok(None);
        }
        orphan.chunks.entry(chunk.chunk_index).or_insert(chunk.data);
        Ok(None)
    }

    pub fn loss_ratio(&mut self, now: Instant) -> f64 {
        self.trim_loss(now);
        let (lost, total) = self.recent_loss.iter().fold(
            (0_u64, 0_u64),
            |(lost, total), (_, entry_lost, entry_total)| (lost + entry_lost, total + entry_total),
        );
        if total == 0 {
            0.0
        } else {
            lost as f64 / total as f64
        }
    }

    pub fn completed_frames(&self) -> u64 {
        self.completed_frames
    }

    pub(crate) fn take_completed_started_at(&mut self) -> Option<Instant> {
        self.completed_started_at.take()
    }

    pub fn clear(&mut self) {
        self.frames.clear();
        self.orphans.clear();
        self.expected_frame_id = None;
        self.recent_loss.clear();
        self.completed_frames = 0;
        self.completed_started_at = None;
    }

    fn finish_if_complete(
        &mut self,
        frame_id: u32,
        now: Instant,
    ) -> Result<Option<AssembledFrame>, MediaAssemblyError> {
        let Some(assembly) = self.frames.get(&frame_id) else {
            return Ok(None);
        };
        if !Self::complete(assembly) {
            return Ok(None);
        }
        let assembly = self.frames.remove(&frame_id).expect("frame existed above");
        let mut data = Vec::with_capacity(assembly.header.total_size as usize);
        for index in 0..assembly.header.total_chunks {
            data.extend_from_slice(
                assembly
                    .chunks
                    .get(&index)
                    .ok_or(MediaAssemblyError::SizeMismatch)?,
            );
        }
        if data.len() != assembly.header.total_size as usize {
            Self::trace_frame(
                &self.trace,
                now,
                frame_id,
                crate::ReceiverTraceEvent::AssemblyInvalid,
            );
            return Err(MediaAssemblyError::SizeMismatch);
        }
        self.completed_frames += 1;
        Self::trace_frame(
            &self.trace,
            now,
            frame_id,
            crate::ReceiverTraceEvent::AssemblyComplete,
        );
        self.completed_started_at = Some(assembly.started);
        Ok(Some(AssembledFrame {
            header: assembly.header,
            data,
            timestamp_ms: assembly.timestamp_ms,
        }))
    }

    fn complete(assembly: &FrameAssembly) -> bool {
        assembly.chunks.len() == assembly.header.total_chunks as usize
    }

    fn expire(&mut self, now: Instant) {
        self.frames.retain(|id, assembly| {
            let keep = now.saturating_duration_since(assembly.started) < ASSEMBLY_TIMEOUT;
            if !keep {
                Self::trace_frame(
                    &self.trace,
                    now,
                    *id,
                    crate::ReceiverTraceEvent::AssemblyTimeout,
                );
            }
            keep
        });
        self.orphans.retain(|id, orphan| {
            let keep = now.saturating_duration_since(orphan.started) < ASSEMBLY_TIMEOUT;
            if !keep {
                Self::trace_frame(
                    &self.trace,
                    now,
                    *id,
                    crate::ReceiverTraceEvent::AssemblyTimeout,
                );
            }
            keep
        });
    }

    fn trace_frame(
        trace: &crate::ReceiverTrace,
        now: Instant,
        id: u32,
        event: crate::ReceiverTraceEvent,
    ) {
        let mut record = crate::ReceiverTraceRecord::new(event);
        record.frame = Some(id);
        trace.record(now, record);
    }

    fn track_loss(&mut self, frame_id: u32, now: Instant) {
        // Only forward serial progress counts a gap; reordered or wrapped IDs
        // are retransmissions, not losses.
        let lost =
            self.expected_frame_id
                .map_or(0, |expected| match frame_id.wrapping_sub(expected) {
                    forward if forward < SERIAL_HALF_RANGE => forward as u64,
                    _ => 0,
                });
        self.recent_loss.push_back((now, lost, lost + 1));
        let successor = frame_id.wrapping_add(1);
        self.expected_frame_id = Some(match self.expected_frame_id {
            None => successor,
            Some(expected) => {
                if successor != expected && serial_not_before(successor, expected) {
                    successor
                } else {
                    expected
                }
            }
        });
        self.trim_loss(now);
    }

    fn trim_loss(&mut self, now: Instant) {
        while self.recent_loss.len() > MAX_LOSS_SAMPLES
            || self.recent_loss.front().is_some_and(|(timestamp, _, _)| {
                now.saturating_duration_since(*timestamp) > Duration::from_secs(5)
            })
        {
            self.recent_loss.pop_front();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CursorState {
    pub x: f32,
    pub y: f32,
    pub cursor_type: u8,
}

impl Default for CursorState {
    fn default() -> Self {
        Self {
            x: -1.0,
            y: -1.0,
            cursor_type: 0,
        }
    }
}

impl CursorState {
    pub fn update(&mut self, update: CursorUpdate) {
        self.x = update.x.clamp(0.0, 1.0);
        self.y = update.y.clamp(0.0, 1.0);
        self.cursor_type = update.cursor_type;
    }
}

#[cfg(test)]
mod receiver_timing_tests {
    use super::*;

    fn header() -> FrameHeader {
        FrameHeader {
            frame_id: 1,
            width: 1,
            height: 1,
            is_key_frame: false,
            total_chunks: 2,
            total_size: 2,
        }
    }

    fn chunk(index: u16) -> FrameChunk {
        FrameChunk {
            frame_id: 1,
            chunk_index: index,
            data: vec![index as u8],
        }
    }

    #[test]
    fn frame_retention_bounds_header_bursts() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        for frame_id in 0..8192 {
            frames
                .push_header(
                    FrameHeader {
                        frame_id,
                        ..header()
                    },
                    42,
                    now + Duration::from_nanos(frame_id.into()),
                )
                .unwrap();
        }
        eprintln!(
            "8192 incomplete headers retain {} frames and {} loss samples",
            frames.frames.len(),
            frames.recent_loss.len()
        );
        assert!(
            frames.frames.len() <= 16,
            "incomplete frame state is unbounded"
        );
        assert!(
            frames.recent_loss.len() <= 4096,
            "header loss telemetry state is unbounded"
        );
        assert!(!frames.frames.contains_key(&0));
        assert!(frames.frames.contains_key(&8191));
        for index in 0..2 {
            let completed = frames
                .push_chunk(
                    FrameChunk {
                        frame_id: 8191,
                        ..chunk(index)
                    },
                    now + Duration::from_millis(1),
                )
                .unwrap();
            if index == 1 {
                assert_eq!(completed.unwrap().data, [0, 1]);
            } else {
                assert!(completed.is_none());
            }
        }
        frames.expire(now + ASSEMBLY_TIMEOUT + Duration::from_millis(1));
        assert!(frames.frames.is_empty());
    }

    #[test]
    fn frame_retention_duplicate_and_invalid_headers_preserve_other_frames() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        for frame_id in 0..16 {
            frames
                .push_header(
                    FrameHeader {
                        frame_id,
                        ..header()
                    },
                    42,
                    now + Duration::from_nanos(frame_id.into()),
                )
                .unwrap();
        }
        frames
            .push_header(
                FrameHeader {
                    frame_id: 15,
                    ..header()
                },
                99,
                now + Duration::from_millis(1),
            )
            .unwrap();
        assert!(frames
            .push_header(
                FrameHeader {
                    frame_id: 16,
                    total_chunks: 0,
                    ..header()
                },
                99,
                now + Duration::from_millis(1),
            )
            .is_err());
        assert_eq!(frames.frames.len(), 16);
        assert!(frames.frames.contains_key(&0));
        assert_eq!(frames.frames[&15].timestamp_ms, 99);
    }

    #[test]
    fn frame_retention_complete_orphans_preserve_pending_frames() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        for frame_id in 0..16 {
            frames
                .push_header(
                    FrameHeader {
                        frame_id,
                        ..header()
                    },
                    42,
                    now,
                )
                .unwrap();
        }
        for index in 0..2 {
            frames
                .push_chunk(
                    FrameChunk {
                        frame_id: 100,
                        ..chunk(index)
                    },
                    now,
                )
                .unwrap();
        }
        let completed = frames
            .push_header(
                FrameHeader {
                    frame_id: 100,
                    ..header()
                },
                42,
                now,
            )
            .unwrap()
            .unwrap();
        assert_eq!(completed.data, [0, 1]);
        assert_eq!(frames.frames.len(), 16);
        assert!(frames.frames.contains_key(&0));
    }

    #[test]
    fn receiver_assembly_keeps_first_orphan_and_one_shot_completion_start() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        frames.push_chunk(chunk(1), now).unwrap();
        frames
            .push_chunk(chunk(1), now + Duration::from_millis(10))
            .unwrap();
        frames
            .push_header(header(), 42, now + Duration::from_millis(20))
            .unwrap();
        assert_eq!(frames.take_completed_started_at(), None);
        let frame = frames
            .push_chunk(chunk(0), now + Duration::from_millis(30))
            .unwrap()
            .unwrap();
        assert_eq!(frame.data, [0, 1]);
        assert_eq!(frames.take_completed_started_at(), Some(now));
        assert_eq!(frames.take_completed_started_at(), None);
    }

    #[test]
    fn receiver_header_completion_and_failed_completion_do_not_leak_timing() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        frames.push_chunk(chunk(0), now).unwrap();
        frames
            .push_chunk(chunk(1), now + Duration::from_millis(10))
            .unwrap();
        assert!(frames
            .push_header(header(), 42, now + Duration::from_millis(20))
            .unwrap()
            .is_some());
        assert_eq!(frames.take_completed_started_at(), Some(now));
        frames.push_header(header(), 42, now).unwrap();
        frames.push_chunk(chunk(0), now).unwrap();
        let mut invalid = chunk(1);
        invalid.data.clear();
        assert_eq!(
            frames.push_chunk(invalid, now),
            Err(MediaAssemblyError::SizeMismatch)
        );
        assert_eq!(frames.take_completed_started_at(), None);
        frames.clear();
        assert_eq!(frames.take_completed_started_at(), None);
    }

    #[test]
    fn retransmitted_header_preserves_chunks_and_completes() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        frames.push_header(header(), 42, now).unwrap();
        frames
            .push_chunk(chunk(0), now + Duration::from_millis(1))
            .unwrap();
        // A repeated header mid-frame must not erase chunk 0.
        frames
            .push_header(header(), 42, now + Duration::from_millis(2))
            .unwrap();
        assert!(frames.frames[&1].chunks.contains_key(&0));
        let completed = frames
            .push_chunk(chunk(1), now + Duration::from_millis(3))
            .unwrap()
            .unwrap();
        assert_eq!(completed.data, [0, 1]);
    }

    #[test]
    fn conflicting_header_for_existing_frame_is_rejected() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        frames.push_header(header(), 42, now).unwrap();
        frames.push_chunk(chunk(0), now).unwrap();
        // A conflicting header for the same frame id cannot silently replace
        // the in-progress assembly.
        assert!(frames
            .push_header(
                FrameHeader {
                    total_chunks: 3,
                    total_size: 3,
                    ..header()
                },
                7,
                now + Duration::from_millis(1),
            )
            .unwrap()
            .is_none());
        assert_eq!(frames.frames[&1].header.total_chunks, 2);
        assert!(frames.frames[&1].chunks.contains_key(&0));
        // The frame still completes under its original header.
        let completed = frames
            .push_chunk(chunk(1), now + Duration::from_millis(2))
            .unwrap()
            .unwrap();
        assert_eq!(completed.header.total_chunks, 2);
        assert_eq!(completed.data, [0, 1]);
    }

    #[test]
    fn promotion_validates_orphan_indices_and_payload_budget() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        // Out-of-range orphan indices and an over-budget payload cannot join
        // the assembly once the header declares its real shape.
        for index in [2_u16, 3] {
            frames
                .push_chunk(
                    FrameChunk {
                        frame_id: 100,
                        chunk_index: index,
                        data: vec![index as u8; 2],
                    },
                    now,
                )
                .unwrap();
        }
        frames
            .push_header(
                FrameHeader {
                    frame_id: 100,
                    ..header()
                },
                42,
                now,
            )
            .unwrap();
        assert!(frames.frames[&100].chunks.is_empty());
        // The frame only completes with the genuine index set.
        for index in 0..2 {
            let completed = frames
                .push_chunk(
                    FrameChunk {
                        frame_id: 100,
                        chunk_index: index,
                        data: vec![index as u8],
                    },
                    now + Duration::from_millis(1),
                )
                .unwrap();
            if index == 1 {
                assert_eq!(completed.unwrap().data, [0, 1]);
            } else {
                assert!(completed.is_none());
            }
        }
    }

    #[test]
    fn chunks_past_the_declared_payload_size_are_not_retained() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        frames.push_header(header(), 42, now).unwrap();
        frames.push_chunk(chunk(0), now).unwrap();
        // header() declares two 1-byte chunks; oversized duplicates and new
        // chunks must not inflate the assembly past the declared size.
        let oversized = FrameChunk {
            frame_id: 1,
            chunk_index: 0,
            data: vec![9; 4],
        };
        let oversized_new = FrameChunk {
            frame_id: 1,
            chunk_index: 1,
            data: vec![9; 4],
        };
        frames.push_chunk(oversized, now).unwrap();
        frames.push_chunk(oversized_new, now).unwrap();
        assert_eq!(frames.frames[&1].payload_bytes, 1);
        assert_eq!(frames.frames[&1].chunks[&0], vec![0]);
        assert!(!frames.frames[&1].chunks.contains_key(&1));
        // The frame still completes with genuine chunks.
        let completed = frames.push_chunk(chunk(1), now).unwrap().unwrap();
        assert_eq!(completed.data, [0, 1]);
    }

    #[test]
    fn orphan_chunks_above_the_chunk_cap_are_not_stored() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        frames
            .push_chunk(
                FrameChunk {
                    frame_id: 7,
                    chunk_index: MAX_CHUNKS_PER_FRAME,
                    data: vec![1],
                },
                now,
            )
            .unwrap();
        frames
            .push_header(
                FrameHeader {
                    frame_id: 7,
                    total_chunks: 1,
                    total_size: 1,
                    ..header()
                },
                42,
                now,
            )
            .unwrap();
        // The rejected orphan index was never retained, so chunk 0 completes.
        let completed = frames
            .push_chunk(
                FrameChunk {
                    frame_id: 7,
                    chunk_index: 0,
                    data: vec![0],
                },
                now,
            )
            .unwrap()
            .unwrap();
        assert_eq!(completed.data, [0]);
    }

    #[test]
    fn loss_tracking_survives_u32_wrap() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        for frame_id in [u32::MAX - 1, u32::MAX, 0] {
            frames
                .push_header(
                    FrameHeader {
                        frame_id,
                        ..header()
                    },
                    42,
                    now,
                )
                .unwrap();
        }
        // Receiving 2 while 1 is missing must count the gap after the wrap.
        frames
            .push_header(
                FrameHeader {
                    frame_id: 2,
                    ..header()
                },
                42,
                now,
            )
            .unwrap();
        assert_eq!(frames.loss_ratio(now), 1.0 / 5.0);
    }

    #[test]
    fn keyframe_pruning_uses_serial_order_across_wrap() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        frames
            .push_header(
                FrameHeader {
                    frame_id: 0,
                    is_key_frame: false,
                    ..header()
                },
                42,
                now,
            )
            .unwrap();
        // A reordered pre-wrap keyframe must not delete the post-wrap assembly.
        frames
            .push_header(
                FrameHeader {
                    frame_id: u32::MAX - 1,
                    is_key_frame: true,
                    ..header()
                },
                42,
                now + Duration::from_millis(1),
            )
            .unwrap();
        assert!(frames.frames.contains_key(&0));
        // An older frame on the same side of the wrap is still pruned.
        frames
            .push_header(
                FrameHeader {
                    frame_id: 3,
                    is_key_frame: false,
                    ..header()
                },
                42,
                now + Duration::from_millis(2),
            )
            .unwrap();
        frames
            .push_header(
                FrameHeader {
                    frame_id: 4,
                    is_key_frame: true,
                    ..header()
                },
                42,
                now + Duration::from_millis(3),
            )
            .unwrap();
        assert!(!frames.frames.contains_key(&3));
        assert!(frames.frames.contains_key(&4));
    }

    #[test]
    fn receiver_timing_does_not_change_legacy_header_gap_loss() {
        let now = Instant::now();
        let mut frames = FrameAssembler::default();
        for frame_id in [1, 3, 2, 3] {
            frames
                .push_header(
                    FrameHeader {
                        frame_id,
                        ..header()
                    },
                    42,
                    now,
                )
                .unwrap();
        }
        // Legacy frame-header inference counts the gap immediately and duplicates
        // in its denominator. Packet telemetry must not silently retune ABR.
        assert_eq!(frames.loss_ratio(now), 1.0 / 5.0);
        assert_eq!(frames.loss_ratio(now + Duration::from_secs(5)), 1.0 / 5.0);
        assert_eq!(
            frames.loss_ratio(now + Duration::from_secs(5) + Duration::from_nanos(1)),
            0.0
        );
    }
}
