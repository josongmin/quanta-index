//! WAL publisher. Generic over [`OpCodec`].

use std::fs::{File, OpenOptions, create_dir_all};
use std::path::PathBuf;
use std::sync::Mutex;

use fs2::FileExt;
use quanta_index_contract::{ChannelSeq, ManifestGeneration, RepoId, RevisionId};

use crate::api::error::ChannelError;
use crate::api::publisher::BundleChannelPublisher;

use super::codec::OpCodec;
use super::segment::{SegmentLayout, SegmentReader, SegmentWriter, open_segment_for_write};

/// Honest durability default: every published frame is fsync'd before
/// `publish()` returns.
///
/// This guarantees the contract that the returned `ChannelSeq` is durable —
/// power-loss after `publish()` returns can never lose the entry.
/// Throughput-sensitive deployments can wrap the publisher in a batch buffer
/// at a higher layer where the relaxed semantics are visible to the producer.
const BATCH_FSYNC_EVERY_N: u64 = 1;

pub struct WalPublisher<C: OpCodec> {
    inner: Mutex<PublisherInner>,
    _codec: std::marker::PhantomData<fn() -> C>,
}

struct PublisherInner {
    layout: SegmentLayout,
    _lock: File,
    writer: SegmentWriter,
    next_seq: u64,
    since_last_fsync: u64,
}

impl<C: OpCodec> WalPublisher<C> {
    pub fn open_with_codec(track_root: PathBuf, _codec: C) -> Result<Self, ChannelError> {
        create_dir_all(&track_root)?;
        let layout = SegmentLayout::new(track_root);

        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(layout.lock_path())?;
        lock_file
            .try_lock_exclusive()
            .map_err(|err| ChannelError::State(format!("publisher lock contention: {err}")))?;

        let (active_seg_id, next_seq) = recover_state(&layout)?;
        let writer = SegmentWriter::open_or_create(&layout, active_seg_id)?;

        Ok(Self {
            inner: Mutex::new(PublisherInner {
                layout,
                _lock: lock_file,
                writer,
                next_seq,
                since_last_fsync: 0,
            }),
            _codec: std::marker::PhantomData,
        })
    }
}

impl<C: OpCodec> BundleChannelPublisher for WalPublisher<C> {
    type Op = C::Op;

    #[expect(
        clippy::significant_drop_tightening,
        reason = "publish() must operate under the mutex for the entire frame-append + seq-bump critical section; tightening the guard would allow torn frame interleaving"
    )]
    fn publish(&self, op: Self::Op) -> Result<ChannelSeq, ChannelError> {
        let is_seal = C::is_seal(&op);
        let mut body: Vec<u8> = Vec::with_capacity(256);
        let tag = C::encode_op(&op, &mut body)?;
        // prepend tag byte
        let mut full_body: Vec<u8> = Vec::with_capacity(body.len().saturating_add(1));
        full_body.push(tag);
        full_body.extend_from_slice(&body);

        let mut guard = self
            .inner
            .lock()
            .map_err(|err| ChannelError::State(format!("publisher mutex poisoned: {err}")))?;

        rotate_if_needed(&mut guard)?;

        let seq = ChannelSeq::new(guard.next_seq);
        let fsync_now = is_seal;
        let _bytes = guard.writer.append_frame(seq, &full_body, fsync_now)?;
        guard.next_seq = guard.next_seq.saturating_add(1);
        if fsync_now {
            guard.since_last_fsync = 0;
        } else {
            guard.since_last_fsync = guard.since_last_fsync.saturating_add(1);
            if guard.since_last_fsync >= BATCH_FSYNC_EVERY_N {
                guard.writer.fsync()?;
                guard.since_last_fsync = 0;
            }
        }
        Ok(seq)
    }

    fn seal(
        &self,
        repo: RepoId,
        revision: RevisionId,
        generation: ManifestGeneration,
    ) -> Result<ChannelSeq, ChannelError> {
        let op = C::make_seal(repo, revision, generation);
        self.publish(op)
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "flush() needs the mutex held across fsync + counter reset to keep the durability point consistent with subsequent publishers"
    )]
    fn flush(&self) -> Result<(), ChannelError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|err| ChannelError::State(format!("publisher mutex poisoned: {err}")))?;
        guard.writer.fsync()?;
        guard.since_last_fsync = 0;
        Ok(())
    }
}

fn rotate_if_needed(guard: &mut PublisherInner) -> Result<(), ChannelError> {
    if !guard.writer.should_rotate() {
        return Ok(());
    }
    // close current by fsync then open next segment
    guard.writer.fsync()?;
    let new_seg_id = guard.writer.seg_id().saturating_add(1);
    let new_file = open_segment_for_write(&guard.layout, new_seg_id)?;
    guard.writer.reset_after_rotate(new_file, new_seg_id);
    Ok(())
}

/// Recover the active segment id and next sequence number from on-disk state.
///
/// If the tail segment ends with a partial / torn frame, the file is truncated
/// back to the end of the last good frame. Without this heal step, a fresh
/// subscriber would later interpret the garbage tail as a frame header and
/// raise `Corrupted` — turning a recoverable partial write into a permanently
/// poisoned channel.
fn recover_state(layout: &SegmentLayout) -> Result<(u64, u64), ChannelError> {
    let segments = layout.list_segments()?;
    if segments.is_empty() {
        return Ok((0, 1));
    }
    let max_seg_id = segments.iter().map(|(id, _)| *id).max().ok_or_else(|| {
        ChannelError::State("segment listing empty after non-empty check".to_string())
    })?;
    let mut next_seq: u64 = 1;
    for (seg_id, _) in &segments {
        let mut reader = SegmentReader::open(layout, *seg_id)?;
        let mut last_good_end: u64 = reader.position();
        let mut needs_heal = false;
        loop {
            match reader.read_next_frame() {
                Ok(Some((seq, _body))) => {
                    next_seq = seq.get().saturating_add(1).max(next_seq);
                    last_good_end = reader.position();
                }
                Ok(None) => break,
                Err(ChannelError::Corrupted { .. }) => {
                    // Partial / corrupt tail in the active write segment is acceptable —
                    // we resume after the last good seq. Earlier segments must be intact.
                    if *seg_id == max_seg_id {
                        needs_heal = true;
                        break;
                    }
                    return Err(ChannelError::State(format!(
                        "corruption in non-tail segment {seg_id}"
                    )));
                }
                Err(err) => return Err(err),
            }
        }
        if needs_heal {
            heal_segment_tail(layout, *seg_id, last_good_end)?;
        }
    }
    Ok((max_seg_id, next_seq))
}

/// Truncate the tail segment to the end of the last good frame. Subsequent
/// appends will land cleanly at `last_good_end`, and any subscriber that opens
/// after recovery sees only well-formed frames.
fn heal_segment_tail(
    layout: &SegmentLayout,
    seg_id: u64,
    last_good_end: u64,
) -> Result<(), ChannelError> {
    let path = layout.segment_path(seg_id);
    let file = OpenOptions::new().write(true).open(&path)?;
    file.set_len(last_good_end)?;
    file.sync_all()?;
    Ok(())
}
