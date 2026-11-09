//! Independent literal-wire checks for the execution-local occurrence reader.

use super::*;
use quanta_index_core::index::{
    Digest, ObjectBatchInput, RegisteredObject, RegisteredObjectBatch, WorkUsage,
    ports::ObjectWriteSink,
};
use std::sync::atomic::{AtomicUsize, Ordering};

const DISTINCT_TF: &[u8] = b"QITXOC01\x00\x02\x00\x02\x00\x00\x00\x00\x00\x06\x00alpha\x00\x00\x00\x05\x00beta\x00\x01\x00\x02\x00\x00\x00\x03\x00\x00\x00\x0b\x00\x01\x00\x00\x00\x07";
const LEAF: &[u8] = br#"{"positions":[3,11]}"#;

#[derive(Clone, Copy)]
enum RetainedReply {
    Same,
    Different,
    Refuse,
    Cancel,
}

struct ProbePort {
    entries: Vec<(ObjectRef, StoredObject)>,
    wrong: Option<StoredObject>,
    reply: RetainedReply,
    gets: AtomicUsize,
    retained: AtomicUsize,
}

impl ProbePort {
    fn new(entries: Vec<(ObjectRef, StoredObject)>, reply: RetainedReply) -> Self {
        Self {
            entries,
            wrong: None,
            reply,
            gets: AtomicUsize::new(0),
            retained: AtomicUsize::new(0),
        }
    }

    fn counts(&self) -> (usize, usize) {
        (
            self.gets.load(Ordering::Relaxed),
            self.retained.load(Ordering::Relaxed),
        )
    }
}

impl ImmutableObjectPort for ProbePort {
    fn identify(
        &self,
        _: &[u8],
        _: &[ObjectRef],
        _: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation("unexpected identify".into()))
    }

    fn put(
        &self,
        _: &[u8],
        _: &[ObjectRef],
        _: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation("unexpected put".into()))
    }

    fn install(
        &self,
        _: &[u8],
        _: &[ObjectRef],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation("unexpected install".into()))
    }

    fn install_batch(
        &self,
        _: &[ObjectBatchInput],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected batch install".into(),
        ))
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 64,
            ..WorkUsage::default()
        })?;
        let _previous = self.gets.fetch_add(1, Ordering::Relaxed);
        self.entries
            .iter()
            .find(|(key, _)| *key == reference)
            .map(|(_, object)| object.clone())
            .ok_or(IndexError::SnapshotUnavailable)
    }

    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 64,
            ..WorkUsage::default()
        })?;
        let _previous = self.retained.fetch_add(1, Ordering::Relaxed);
        match self.reply {
            RetainedReply::Same => Ok(object.clone()),
            RetainedReply::Different => self.wrong.clone().ok_or_else(|| {
                IndexError::BackendViolation("missing foreign retained fixture".into())
            }),
            RetainedReply::Refuse => Err(IndexError::ResourceLimited),
            RetainedReply::Cancel => {
                budget.request().cancel_handle().cancel();
                Ok(object.clone())
            }
        }
    }
}

fn reference(byte: u8) -> ObjectRef {
    ObjectRef(Digest([byte; 32]))
}

fn object(
    reference: ObjectRef,
    payload: &[u8],
    budget: &WorkBudget,
) -> Result<StoredObject, IndexError> {
    let memory = budget.reserve_memory(
        StoredObject::allocation_bytes(payload.len(), 0)?,
        MemoryLifetime::Temporary,
    )?;
    Ok(StoredObject::new(
        RegisteredObject::unscoped(
            reference,
            u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
            Vec::new(),
        ),
        payload.to_vec(),
        memory,
    ))
}

#[test]
fn one_block_ref_has_two_independent_term_counts_and_positions() -> Result<(), IndexError> {
    let (mut budget, tracker) = funded(100_000, None, None)?;
    let key = reference(1);
    let port = ProbePort::new(
        vec![(key, object(key, DISTINCT_TF, &budget)?)],
        RetainedReply::Same,
    );
    let mut reader = OccurrenceReader::new(2)?;
    require(!reader.contains(key), "empty reader claimed a block")?;

    let alpha = reader.count(key, b"\0alpha", &port, &mut budget)?;
    require(alpha.value == 2 && !alpha.leaf, "alpha tf differs")?;
    require(reader.contains(key), "parsed block was not cached")?;
    let beta = reader.count(key, b"\0beta", &port, &mut budget)?;
    require(beta.value == 1 && !beta.leaf, "beta reused alpha tf")?;
    require(
        reader.count(key, b"\0alpha", &port, &mut budget)?.value == 2,
        "repeated alpha tf differs",
    )?;
    require(
        matches!(
            reader.count(key, b"\0missing", &port, &mut budget),
            Err(IndexError::CorruptObject(_))
        ),
        "missing term returned a count",
    )?;
    let beta_positions = reader.load(key, b"\0beta", &port, &mut budget)?;
    require(
        beta_positions.positions.0.as_slice() == [7],
        "beta selected wrong vector",
    )?;
    let alpha_positions = reader.load(key, b"\0alpha", &port, &mut budget)?;
    require(
        alpha_positions.positions.0.as_slice() == [3, 11],
        "alpha selected wrong vector",
    )?;
    require(
        port.counts() == (1, 5),
        "block cache did not use retained reads",
    )?;
    drop(alpha_positions);
    drop(beta_positions);
    drop(reader);
    drop(port);
    cleaned(&tracker)
}

#[test]
fn packed_load_requires_a_checked_block() -> Result<(), IndexError> {
    let (mut budget, tracker) = funded(100_000, None, None)?;
    let block_ref = reference(21);
    let leaf_ref = reference(22);
    let port = ProbePort::new(
        vec![
            (block_ref, object(block_ref, DISTINCT_TF, &budget)?),
            (leaf_ref, object(leaf_ref, LEAF, &budget)?),
        ],
        RetainedReply::Same,
    );
    let mut reader = OccurrenceReader::new(1)?;
    let packed = reader.load_packed(block_ref, b"\0beta", &port, &mut budget)?;
    require(packed.positions.0.as_slice() == [7], "packed beta differs")?;
    require(
        matches!(
            reader.load_packed(leaf_ref, b"\0beta", &port, &mut budget),
            Err(IndexError::CorruptObject(_))
        ),
        "packed inventory accepted scalar leaf",
    )?;
    drop(packed);
    drop(reader);
    drop(port);
    cleaned(&tracker)
}

#[test]
fn leaf_reads_remain_uncached_and_typed_as_leaf() -> Result<(), IndexError> {
    let (mut budget, tracker) = funded(100_000, None, None)?;
    let key = reference(2);
    let port = ProbePort::new(
        vec![(key, object(key, LEAF, &budget)?)],
        RetainedReply::Same,
    );
    let mut reader = OccurrenceReader::new(1)?;
    for _ in 0..2 {
        let count = reader.count(key, b"\0alpha", &port, &mut budget)?;
        require(count.value == 2 && count.leaf, "leaf count or tag differs")?;
    }
    require(!reader.contains(key), "leaf entered block cache")?;
    require(port.counts() == (2, 0), "leaf used retained block path")?;
    drop(reader);
    drop(port);
    cleaned(&tracker)
}

#[test]
fn uncached_blocks_repeat_original_reads_without_retained_reuse() -> Result<(), IndexError> {
    let (mut budget, tracker) = funded(100_000, None, None)?;
    let key = reference(10);
    let port = ProbePort::new(
        vec![(key, object(key, DISTINCT_TF, &budget)?)],
        RetainedReply::Same,
    );
    let mut reader = OccurrenceReader::uncached()?;
    for _ in 0..2 {
        for (term, expected) in [(b"\0alpha".as_slice(), 2), (b"\0beta", 1)] {
            let count = reader.count(key, term, &port, &mut budget)?;
            require(
                count.value == expected && !count.leaf,
                "uncached term tf differs",
            )?;
            require(!reader.contains(key), "uncached reader exposed a block hit")?;
            require(
                reader.validated_count(key, term, &budget)?.is_none(),
                "uncached reader reused derived block DATA",
            )?;
        }
    }
    let alpha = reader.load(key, b"\0alpha", &port, &mut budget)?;
    require(
        alpha.positions.0.as_slice() == [3, 11],
        "uncached alpha positions differ",
    )?;
    let beta = reader.load(key, b"\0beta", &port, &mut budget)?;
    require(
        beta.positions.0.as_slice() == [7],
        "uncached beta positions differ",
    )?;
    require(
        port.counts() == (6, 0),
        "uncached block used a retained read",
    )?;
    drop(alpha);
    drop(beta);
    drop(reader);
    drop(port);
    cleaned(&tracker)
}

#[test]
fn ring_evicts_one_block_and_keeps_capacity_bound() -> Result<(), IndexError> {
    let (mut budget, tracker) = funded(200_000, None, None)?;
    let refs = [reference(3), reference(4), reference(5)];
    let [first, second, third] = refs;
    let mut entries = Vec::new();
    for key in refs {
        entries.push((key, object(key, DISTINCT_TF, &budget)?));
    }
    let port = ProbePort::new(entries, RetainedReply::Same);
    let mut reader = OccurrenceReader::new(2)?;
    for key in refs {
        require(
            reader.count(key, b"\0beta", &port, &mut budget)?.value == 1,
            "block tf differs",
        )?;
    }
    require(
        !reader.contains(first) && reader.contains(second) && reader.contains(third),
        "ring retained wrong block",
    )?;
    require(port.counts() == (3, 0), "ring performed unexpected read")?;
    require(
        reader.count(first, b"\0alpha", &port, &mut budget)?.value == 2,
        "evicted block could not reload",
    )?;
    require(port.counts() == (4, 0), "evicted block did not reload")?;
    drop(reader);
    drop(port);
    cleaned(&tracker)
}

#[test]
fn wrong_retained_reply_and_refusal_cannot_return_cached_data() -> Result<(), IndexError> {
    for reply in [
        RetainedReply::Different,
        RetainedReply::Refuse,
        RetainedReply::Cancel,
    ] {
        let (mut budget, tracker) = funded(100_000, None, None)?;
        let key = reference(6);
        let mut port = ProbePort::new(vec![(key, object(key, DISTINCT_TF, &budget)?)], reply);
        let mut changed = DISTINCT_TF.to_vec();
        // Same reference and shape, different authenticated DATA.
        *changed.get_mut(44).ok_or_else(|| {
            IndexError::BackendViolation("literal retained fixture has wrong length".into())
        })? = 8;
        port.wrong = Some(object(key, &changed, &budget)?);
        let mut reader = OccurrenceReader::new(1)?;
        require(
            reader.count(key, b"\0alpha", &port, &mut budget)?.value == 2,
            "initial cache fill failed",
        )?;
        let result = reader.count(key, b"\0beta", &port, &mut budget);
        match reply {
            RetainedReply::Different => require(
                matches!(result, Err(IndexError::CorruptObject(_))),
                "foreign retained DATA was accepted",
            )?,
            RetainedReply::Refuse => require(
                matches!(result, Err(IndexError::ResourceLimited)),
                "retained refusal was replaced",
            )?,
            RetainedReply::Cancel => require(
                matches!(result, Err(IndexError::Cancelled)),
                "retained cancellation was replaced",
            )?,
            RetainedReply::Same => {
                return Err(IndexError::BackendViolation(
                    "retained fault fixture selected the success branch".into(),
                ));
            }
        }
        require(port.counts() == (1, 1), "retained branch made a fresh get")?;
        drop(reader);
        drop(port);
        cleaned(&tracker)?;
    }
    Ok(())
}

#[test]
fn checked_layout_and_object_custody_outlive_position_result() -> Result<(), IndexError> {
    let (mut budget, tracker) = funded(100_000, None, None)?;
    let key = reference(7);
    let port = ProbePort::new(
        vec![(key, object(key, DISTINCT_TF, &budget)?)],
        RetainedReply::Same,
    );
    let baseline = tracker.snapshot()?.live;
    let mut reader = OccurrenceReader::new(1)?;
    let positions = reader.load(key, b"\0alpha", &port, &mut budget)?;
    require(
        tracker.snapshot()?.live > baseline,
        "reader did not fund cache and positions",
    )?;
    drop(reader);
    require(
        positions.positions.0.as_slice() == [3, 11],
        "position result depended on cache lifetime",
    )?;
    require(
        tracker.snapshot()?.live >= baseline,
        "object grant dropped before port custody",
    )?;
    drop(positions);
    drop(port);
    cleaned(&tracker)
}

#[test]
fn reader_refuses_invalid_window_and_foreign_initial_object() -> Result<(), IndexError> {
    require(
        matches!(OccurrenceReader::new(0), Err(IndexError::InvalidInput(_)))
            && matches!(OccurrenceReader::new(65), Err(IndexError::InvalidInput(_))),
        "reader accepted an unbounded cache window",
    )?;
    let (mut budget, tracker) = funded(100_000, None, None)?;
    let requested = reference(8);
    let foreign = reference(9);
    let port = ProbePort::new(
        vec![(requested, object(foreign, DISTINCT_TF, &budget)?)],
        RetainedReply::Same,
    );
    let mut reader = OccurrenceReader::new(1)?;
    require(
        matches!(
            reader.count(requested, b"\0alpha", &port, &mut budget),
            Err(IndexError::CorruptObject(_))
        ),
        "foreign get response entered the cache",
    )?;
    require(!reader.contains(requested), "foreign object entered cache")?;
    require(
        port.counts() == (1, 0),
        "foreign response used retained path",
    )?;
    drop(reader);
    drop(port);
    cleaned(&tracker)
}
