//! Hellgate (worst-case) integration tests for the WAL-backed channel.
//!
//! These tests exercise catastrophic failure modes the channel must handle
//! correctly: CRC corruption, torn-tail writes, truncation, segment rotation,
//! cursor regression, and producer/subscriber lifecycle edges. The channel is
//! fail-closed — it must surface `ChannelError::Corrupted` (or `Io`) rather
//! than silently skipping bad entries.

#![forbid(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use quanta_index_channel::{
    BundleChannelPublisher, BundleChannelSubscriber, ChannelError, open_lexical_publisher,
    open_lexical_subscriber,
};
use quanta_index_contract::{
    ChannelSeq, ChunkId, DeleteChunk, LexicalChannelOp, LexicalFullBundle, ManifestGeneration,
    RepoId, RevisionId, UpsertChunk,
};

type BoxedErr = Box<dyn std::error::Error>;
type TestResult = Result<(), BoxedErr>;

fn boxed(msg: impl Into<String>) -> BoxedErr {
    BoxedErr::from(msg.into())
}

fn repo() -> RepoId {
    RepoId::new("repo-hell")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-hell")
}

fn gen_1() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn mk_upsert(name: &str, payload: &[u8]) -> LexicalChannelOp {
    LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation: gen_1(),
        chunk_id: ChunkId::new(name),
        payload: payload.to_vec(),
    })
}

fn lexical_segment_dir(state_root: &Path) -> PathBuf {
    state_root.join("channel").join("lexical")
}

fn lexical_segment_file(state_root: &Path) -> PathBuf {
    lexical_segment_dir(state_root).join("log.wal.00000000000000000000")
}

fn read_all(path: &Path) -> Result<Vec<u8>, BoxedErr> {
    let mut file = File::open(path)?;
    let mut buf: Vec<u8> = Vec::new();
    let _bytes = file.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Compute the byte offset (within the segment file) of frame `index` (0-based,
/// after the 16-byte segment header). Walks the on-disk frame headers so the
/// caller does not need to know payload lengths.
///
/// Frame layout: `[u32 body_len][u32 crc][u64 seq][codec_body...]`
fn frame_offset(segment: &[u8], index: usize) -> Result<usize, BoxedErr> {
    const HEADER: usize = 16;
    let mut pos: usize = HEADER;
    for i in 0..=index {
        if i == index {
            return Ok(pos);
        }
        let body_len_slice = segment
            .get(pos..pos.saturating_add(4))
            .ok_or_else(|| boxed("frame body_len out of bounds"))?;
        let body_len_arr: [u8; 4] = body_len_slice
            .try_into()
            .map_err(|_err| boxed("body_len slice convert"))?;
        let body_len = u32::from_le_bytes(body_len_arr);
        let body_len_usize = usize::try_from(body_len).map_err(|_err| boxed("body_len usize"))?;
        let total = 8usize
            .checked_add(body_len_usize)
            .ok_or_else(|| boxed("frame total overflow"))?;
        pos = pos
            .checked_add(total)
            .ok_or_else(|| boxed("pos overflow"))?;
    }
    Err(boxed("unreachable"))
}

#[test]
fn crc_mismatch_is_fail_closed() -> TestResult {
    let dir = tempfile::tempdir()?;
    {
        let publisher = open_lexical_publisher(dir.path())?;
        let _ = publisher.publish(mk_upsert("c-1", b"alpha"))?;
        let _ = publisher.publish(mk_upsert("c-2", b"bravo"))?;
        let _ = publisher.publish(mk_upsert("c-3", b"charlie"))?;
        publisher.flush()?;
    }

    let seg_path = lexical_segment_file(dir.path());
    let segment = read_all(&seg_path)?;
    // Find frame #1 (0-indexed: this is the second frame, seq=2). Then flip a
    // byte inside its body — specifically several bytes past the seq prefix so
    // the seq decoded from the corrupted body is still 2.
    let frame2_off = frame_offset(&segment, 1)?;
    // [u32 body_len][u32 crc][u64 seq][codec_body...]
    // Body starts at frame2_off + 8 (seq). Skip 8 bytes of seq + a few body
    // bytes to land mid-payload.
    let corrupt_off = frame2_off
        .checked_add(8 + 8 + 4)
        .ok_or_else(|| boxed("corrupt off overflow"))?;

    {
        let mut file = OpenOptions::new().read(true).write(true).open(&seg_path)?;
        let _ = file.seek(SeekFrom::Start(u64::try_from(corrupt_off)?))?;
        let mut byte = [0u8; 1];
        file.read_exact(&mut byte)?;
        let flipped_slot = byte.get_mut(0).ok_or_else(|| boxed("byte slice missing"))?;
        *flipped_slot ^= 0x5A;
        let _ = file.seek(SeekFrom::Start(u64::try_from(corrupt_off)?))?;
        file.write_all(&byte)?;
        file.sync_data()?;
    }

    let mut subscriber = open_lexical_subscriber(dir.path())?;
    let evt1 = match subscriber.next_event()? {
        Some(e) => e,
        None => return Err(boxed("expected evt1, got None")),
    };
    if evt1.seq.get() != 1 {
        return Err(boxed(format!(
            "evt1 seq expected 1, got {}",
            evt1.seq.get()
        )));
    }

    let result = subscriber.next_event();
    match result {
        Err(ChannelError::Corrupted { at_seq, reason: _ }) => {
            // Spec: at_seq must be the seq of the corrupted frame when
            // recoverable; ZERO is also acceptable. We flipped a byte beyond
            // the seq prefix so we expect seq=2.
            if at_seq.get() != 2 && at_seq != ChannelSeq::ZERO {
                return Err(boxed(format!(
                    "expected at_seq=2 or ZERO, got {}",
                    at_seq.get()
                )));
            }
        }
        Err(other) => {
            return Err(boxed(format!("expected Corrupted, got error {other:?}")));
        }
        Ok(opt) => {
            return Err(boxed(format!(
                "expected Corrupted error, got Ok({:?})",
                opt.is_some()
            )));
        }
    }

    Ok(())
}

#[test]
fn truncated_frame_at_segment_tail() -> TestResult {
    let dir = tempfile::tempdir()?;
    {
        let publisher = open_lexical_publisher(dir.path())?;
        for i in 0..4u64 {
            let _ = publisher.publish(mk_upsert(&format!("c-{i}"), b"payload-bytes"))?;
        }
        publisher.flush()?;
    }

    let seg_path = lexical_segment_file(dir.path());
    let original_len = std::fs::metadata(&seg_path)?.len();
    let new_len = original_len
        .checked_sub(3)
        .ok_or_else(|| boxed("segment file too small to truncate"))?;
    {
        let file = OpenOptions::new().write(true).open(&seg_path)?;
        file.set_len(new_len)?;
        file.sync_data()?;
    }

    let mut subscriber = open_lexical_subscriber(dir.path())?;
    for expected in 1u64..=3 {
        let evt = match subscriber.next_event()? {
            Some(e) => e,
            None => {
                return Err(boxed(format!("expected event at seq {expected}, got None")));
            }
        };
        if evt.seq.get() != expected {
            return Err(boxed(format!(
                "evt seq expected {expected}, got {}",
                evt.seq.get()
            )));
        }
    }

    match subscriber.next_event() {
        Err(ChannelError::Corrupted { .. }) | Err(ChannelError::Io(_)) => Ok(()),
        Err(other) => Err(boxed(format!("expected Corrupted or Io, got {other:?}"))),
        Ok(opt) => Err(boxed(format!(
            "expected error, got Ok({:?})",
            opt.is_some()
        ))),
    }
}

#[test]
fn publisher_restart_resumes_seq_after_partial_tail_write() -> TestResult {
    let dir = tempfile::tempdir()?;
    {
        let publisher = open_lexical_publisher(dir.path())?;
        let s1 = publisher.publish(mk_upsert("a", b"alpha"))?;
        let s2 = publisher.publish(mk_upsert("b", b"bravo"))?;
        if s1.get() != 1 || s2.get() != 2 {
            return Err(boxed(format!(
                "initial seqs expected 1,2 got {},{}",
                s1.get(),
                s2.get()
            )));
        }
        publisher.flush()?;
    }

    // Append 5 bytes of garbage simulating a partial torn write.
    let seg_path = lexical_segment_file(dir.path());
    {
        let mut file = OpenOptions::new().append(true).open(&seg_path)?;
        file.write_all(&[0xAB, 0xCD, 0xEF, 0x12, 0x34])?;
        file.sync_data()?;
    }

    // Re-open publisher: recover_state must skip the partial tail and resume.
    let seq_seal = {
        let publisher = open_lexical_publisher(dir.path())?;
        publisher.seal(repo(), revision(), gen_1())?
    };
    if seq_seal.get() != 3 {
        return Err(boxed(format!(
            "seal seq expected 3 after partial tail, got {}",
            seq_seal.get()
        )));
    }

    let mut subscriber = open_lexical_subscriber(dir.path())?;
    let mut seen: Vec<u64> = Vec::new();
    loop {
        match subscriber.next_event()? {
            Some(evt) => seen.push(evt.seq.get()),
            None => break,
        }
    }
    if seen != vec![1u64, 2, 3] {
        return Err(boxed(format!("expected seqs [1,2,3], got {seen:?}")));
    }
    Ok(())
}

#[test]
fn cursor_regression_is_rejected() -> TestResult {
    let dir = tempfile::tempdir()?;
    {
        let publisher = open_lexical_publisher(dir.path())?;
        for i in 0..5u64 {
            let _ = publisher.publish(mk_upsert(&format!("c-{i}"), b"x"))?;
        }
        publisher.flush()?;
    }

    let mut subscriber = open_lexical_subscriber(dir.path())?;
    let mut last_seq = ChannelSeq::ZERO;
    for _ in 0..3 {
        let evt = match subscriber.next_event()? {
            Some(e) => e,
            None => return Err(boxed("ran out of events before seq 3")),
        };
        last_seq = evt.seq;
    }
    if last_seq.get() != 3 {
        return Err(boxed(format!(
            "expected to land on seq 3, got {}",
            last_seq.get()
        )));
    }
    subscriber.ack(last_seq)?;
    if subscriber.cursor().get() != 3 {
        return Err(boxed(format!(
            "cursor expected 3 after ack, got {}",
            subscriber.cursor().get()
        )));
    }

    match subscriber.ack(ChannelSeq::new(1)) {
        Err(ChannelError::State(_)) => Ok(()),
        Err(other) => Err(boxed(format!(
            "expected State error on ack regression, got {other:?}"
        ))),
        Ok(()) => Err(boxed("expected error on ack(1) after ack(3), got Ok")),
    }
}

#[test]
fn segment_rotation_preserves_continuity() -> TestResult {
    // Default rotation threshold is 10,000 entries. Push enough small ops to
    // force at least one rotation.
    const TOTAL: u64 = 12_000;

    let dir = tempfile::tempdir()?;
    {
        let publisher = open_lexical_publisher(dir.path())?;
        for i in 0..TOTAL {
            let _ = publisher.publish(LexicalChannelOp::DeleteChunk(DeleteChunk {
                repo_id: repo(),
                revision_id: revision(),
                generation: gen_1(),
                chunk_id: ChunkId::new(format!("c-{i}")),
            }))?;
        }
        publisher.flush()?;
    }

    // Verify multiple segment files exist on disk.
    let seg_dir = lexical_segment_dir(dir.path());
    let mut seg_count: usize = 0;
    for entry in std::fs::read_dir(&seg_dir)? {
        let entry = entry?;
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if name.starts_with("log.wal.") {
            seg_count = seg_count.saturating_add(1);
        }
    }
    if seg_count < 2 {
        return Err(boxed(format!(
            "expected >= 2 segment files after rotation, got {seg_count}"
        )));
    }

    let mut subscriber = open_lexical_subscriber(dir.path())?;
    let mut expected: u64 = 1;
    loop {
        match subscriber.next_event()? {
            Some(evt) => {
                if evt.seq.get() != expected {
                    return Err(boxed(format!(
                        "gap detected: expected seq {expected}, got {}",
                        evt.seq.get()
                    )));
                }
                expected = expected.saturating_add(1);
            }
            None => break,
        }
    }
    let last_seen = expected.saturating_sub(1);
    if last_seen != TOTAL {
        return Err(boxed(format!("expected last seq {TOTAL}, got {last_seen}")));
    }
    Ok(())
}

#[test]
fn double_subscriber_independent_cursors() -> TestResult {
    let dir = tempfile::tempdir()?;
    {
        let publisher = open_lexical_publisher(dir.path())?;
        for i in 0..4u64 {
            let _ = publisher.publish(mk_upsert(&format!("c-{i}"), b"p"))?;
        }
        publisher.flush()?;
    }

    let mut sub_a = open_lexical_subscriber(dir.path())?;
    let mut sub_b = open_lexical_subscriber(dir.path())?;

    // Sub A: advance to seq 2 and ack.
    let a1 = sub_a
        .next_event()?
        .ok_or_else(|| boxed("sub_a evt1 missing"))?;
    let a2 = sub_a
        .next_event()?
        .ok_or_else(|| boxed("sub_a evt2 missing"))?;
    if a1.seq.get() != 1 || a2.seq.get() != 2 {
        return Err(boxed(format!(
            "sub_a seqs expected 1,2 got {},{}",
            a1.seq.get(),
            a2.seq.get()
        )));
    }
    sub_a.ack(a2.seq)?;

    // Sub B: independent cursor — must start at zero and see seq 1 first.
    if sub_b.cursor() != ChannelSeq::ZERO {
        return Err(boxed(format!(
            "sub_b cursor expected ZERO, got {}",
            sub_b.cursor().get()
        )));
    }
    let b1 = sub_b
        .next_event()?
        .ok_or_else(|| boxed("sub_b evt1 missing"))?;
    if b1.seq.get() != 1 {
        return Err(boxed(format!(
            "sub_b first evt expected seq 1, got {}",
            b1.seq.get()
        )));
    }
    sub_b.ack(b1.seq)?;
    if sub_b.cursor().get() != 1 {
        return Err(boxed(format!(
            "sub_b cursor expected 1 after ack, got {}",
            sub_b.cursor().get()
        )));
    }
    if sub_a.cursor().get() != 2 {
        return Err(boxed(format!(
            "sub_a cursor expected 2 (unchanged by B), got {}",
            sub_a.cursor().get()
        )));
    }
    Ok(())
}

#[test]
fn fresh_subscriber_on_empty_channel_returns_none() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut subscriber = open_lexical_subscriber(dir.path())?;
    if subscriber.cursor() != ChannelSeq::ZERO {
        return Err(boxed(format!(
            "fresh cursor expected ZERO, got {}",
            subscriber.cursor().get()
        )));
    }
    match subscriber.next_event()? {
        None => Ok(()),
        Some(evt) => Err(boxed(format!(
            "expected None on empty channel, got seq {}",
            evt.seq.get()
        ))),
    }
}

#[test]
fn producer_can_publish_after_subscriber_open() -> TestResult {
    let dir = tempfile::tempdir()?;
    // Subscriber opens first, sees no segments.
    let mut subscriber = open_lexical_subscriber(dir.path())?;
    match subscriber.next_event()? {
        None => {}
        Some(evt) => {
            return Err(boxed(format!(
                "pre-publish next_event expected None, got seq {}",
                evt.seq.get()
            )));
        }
    }

    // Now a publisher writes the first segment + frame.
    {
        let publisher = open_lexical_publisher(dir.path())?;
        let seq = publisher.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: gen_1(),
            payload: b"late-bundle".to_vec(),
        }))?;
        if seq.get() != 1 {
            return Err(boxed(format!("publish seq expected 1, got {}", seq.get())));
        }
        publisher.flush()?;
    }

    // Subscriber must re-scan the segments directory and surface the new event.
    let evt = match subscriber.next_event()? {
        Some(e) => e,
        None => {
            return Err(boxed("subscriber failed to pick up late-arriving segment"));
        }
    };
    if evt.seq.get() != 1 {
        return Err(boxed(format!(
            "late evt seq expected 1, got {}",
            evt.seq.get()
        )));
    }
    match evt.op {
        LexicalChannelOp::FullBundle(b) => {
            if b.payload != b"late-bundle" {
                return Err(boxed("late bundle payload mismatch"));
            }
        }
        other => {
            return Err(boxed(format!("expected FullBundle, got {other:?}")));
        }
    }
    Ok(())
}

#[test]
fn publisher_lock_releases_on_drop() -> TestResult {
    let dir = tempfile::tempdir()?;
    {
        let first = open_lexical_publisher(dir.path())?;
        let _ = first.publish(mk_upsert("c-1", b"x"))?;
        // first drops here, releasing the advisory lock.
    }

    // Second open must succeed and continue the sequence.
    let second = open_lexical_publisher(dir.path())?;
    let seq = second.publish(mk_upsert("c-2", b"y"))?;
    if seq.get() != 2 {
        return Err(boxed(format!(
            "second publisher seq expected 2, got {}",
            seq.get()
        )));
    }
    Ok(())
}
