//! Private Legacy History checkpoint carriers. V1 pages retain their JSON
//! payloads. An oversized row uses a V2 binary directory of raw child objects.

use std::mem::size_of;

use sha2::{Digest as _, Sha256};

use crate::{AuxiliaryDomainV1, AuxiliaryRowFamilyV1, AuxiliaryRowKeyV1, AuxiliaryRowV1};

use super::{
    AuxiliaryGenerationKeyV1, Digest, HistorySnapshotRowV1, IndexError, MemoryLease,
    MemoryLifetime, ObjectRef, StoredObject, WorkBudget, WorkUsage,
    native_immutable_object_ref_with_work_v1,
};

const MAGIC: &[u8; 8] = b"QHROW\0\x02\n";
const PART_BYTES: usize = 1024 * 1024;
// The decoded ingest request ceiling also bounds the combined key/value of an
// individual producer row. One extra part accounts for splitting at the key.
const MAX_ROW_BYTES: usize = 128 * 1024 * 1024;
const REF_BYTES: usize = 32;
// V2 stores magic, little-endian ordinal, generation digest, family tag,
// little-endian key/value lengths, then key refs followed by value refs.
const HEADER_BYTES: usize = MAGIC.len() + 4 + 32 + 1 + 4 + 4;
// Array positions are persistent family tags.
const FAMILIES: [AuxiliaryRowFamilyV1; 6] = [
    AuxiliaryRowFamilyV1::Epoch,
    AuxiliaryRowFamilyV1::Commit,
    AuxiliaryRowFamilyV1::Ref,
    AuxiliaryRowFamilyV1::Tag,
    AuxiliaryRowFamilyV1::DiffHunk,
    AuxiliaryRowFamilyV1::StateMeta,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyHistoryCheckpointPageRefV2 {
    pub reference: ObjectRef,
    pub ordinal: u32,
    pub row_count: u64,
}

quanta_index_contract::native_serde_struct!([serde_identifier] LegacyHistoryCheckpointPageRefV2 {
    reference: ObjectRef,
    ordinal: u32,
    row_count: u64,
});

#[must_use]
pub fn is_large_row_v2(payload: &[u8]) -> bool {
    payload.starts_with(MAGIC)
}

fn generation_digest(generation: &AuxiliaryGenerationKeyV1) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"quanta-index:legacy-history-row-generation:v2\0");
    for id in [generation.repo_id.as_str(), generation.revision_id.as_str()] {
        digest.update((id.len() as u64).to_le_bytes());
        digest.update(id.as_bytes());
    }
    digest.update(generation.generation.get().to_le_bytes());
    digest.finalize().into()
}

fn part_count(key: usize, value: usize) -> Result<usize, IndexError> {
    if key == 0 && value == 0 {
        return Err(IndexError::DigestConflict);
    }
    if key
        .checked_add(value)
        .is_none_or(|total| total > MAX_ROW_BYTES)
    {
        return Err(IndexError::ResourceLimited);
    }
    Ok(key.div_ceil(PART_BYTES) + value.div_ceil(PART_BYTES))
}

fn reserve<T>(count: usize) -> Result<Vec<T>, IndexError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| IndexError::ResourceLimited)?;
    if values.capacity() > count {
        return Err(IndexError::ResourceLimited);
    }
    Ok(values)
}

fn object_ref(
    payload: &[u8],
    children: &[ObjectRef],
    budget: &mut WorkBudget,
) -> Result<ObjectRef, IndexError> {
    budget.charge(WorkUsage {
        bytes: (payload.len() + children.len() * REF_BYTES) as u64,
        ..WorkUsage::default()
    })?;
    native_immutable_object_ref_with_work_v1(payload, children, &mut || budget.checkpoint())?
        .ok_or(IndexError::DigestConflict)
}

fn serialized_object_bytes(payload: &[u8], children: &[ObjectRef]) -> u64 {
    (super::NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1 + payload.len() + children.len() * REF_BYTES)
        as u64
}

/// Emit each distinct raw part followed by the directory that binds the exact
/// logical order, generation, ordinal and lengths of the original SQL row.
pub fn encode_large_row_v2(
    generation: &AuxiliaryGenerationKeyV1,
    ordinal: u32,
    row: &HistorySnapshotRowV1,
    budget: &mut WorkBudget,
    emit: impl FnMut(ObjectRef, &[u8], &[ObjectRef], &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<ObjectRef, IndexError> {
    encode_row(
        generation,
        ordinal,
        &row.family,
        [&row.row_key, &row.value],
        budget,
        emit,
    )
}

/// Store the same raw History row format when its original Native JSON object
/// would exceed the immutable storage bound. A map entry carries one row.
pub fn encode_auxiliary_row_v2(
    row: &AuxiliaryRowV1,
    ordinal: u32,
    budget: &mut WorkBudget,
    emit: impl FnMut(ObjectRef, &[u8], &[ObjectRef], &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<ObjectRef, IndexError> {
    if row.key.domain != AuxiliaryDomainV1::History {
        return Err(IndexError::DigestConflict);
    }
    encode_row(
        &row.key.generation,
        ordinal,
        row.key.family.as_code_str(),
        [&row.key.row_key, &row.value],
        budget,
        emit,
    )
}

fn encode_row(
    generation: &AuxiliaryGenerationKeyV1,
    ordinal: u32,
    family: &str,
    [key, value]: [&[u8]; 2],
    budget: &mut WorkBudget,
    mut emit: impl FnMut(ObjectRef, &[u8], &[ObjectRef], &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<ObjectRef, IndexError> {
    if ordinal == 0 {
        return Err(IndexError::DigestConflict);
    }
    let family = FAMILIES
        .iter()
        .position(|candidate| candidate.as_code_str() == family)
        .ok_or(IndexError::DigestConflict)?;
    let count = part_count(key.len(), value.len())?;
    let directory_bytes = HEADER_BYTES + count * REF_BYTES;
    let _metadata = budget.reserve_memory(
        (directory_bytes + count * size_of::<ObjectRef>()) as u64,
        MemoryLifetime::Temporary,
    )?;
    let mut directory = reserve(directory_bytes)?;
    let mut children = reserve(count)?;
    directory.extend_from_slice(MAGIC);
    directory.extend_from_slice(&ordinal.to_le_bytes());
    directory.extend_from_slice(&generation_digest(generation));
    directory.push(family as u8);
    directory.extend_from_slice(&(key.len() as u32).to_le_bytes());
    directory.extend_from_slice(&(value.len() as u32).to_le_bytes());
    for part in key.chunks(PART_BYTES).chain(value.chunks(PART_BYTES)) {
        let reference = object_ref(part, &[], budget)?;
        if let Err(index) = children.binary_search(&reference) {
            emit(reference, part, &[], budget)?;
            children.insert(index, reference);
        }
        directory.extend_from_slice(&(reference.0).0);
    }
    let reference = object_ref(&directory, &children, budget)?;
    emit(reference, &directory, &children, budget)?;
    Ok(reference)
}

/// Recover the one physical row under the original caller's work budget. The
/// caller holds the returned loan while visiting the row and its owned bytes.
pub fn decode_large_row_v2(
    generation: &AuxiliaryGenerationKeyV1,
    ordinal: u32,
    reference: ObjectRef,
    stored: &StoredObject,
    budget: &mut WorkBudget,
    get: &mut impl FnMut(ObjectRef, &mut WorkBudget) -> Result<StoredObject, IndexError>,
) -> Result<(AuxiliaryRowV1, Option<MemoryLease>), IndexError> {
    let (magic, header) = stored
        .payload
        .split_first_chunk::<8>()
        .ok_or(IndexError::DigestConflict)?;
    let (stored_ordinal, header) = header
        .split_first_chunk::<4>()
        .ok_or(IndexError::DigestConflict)?;
    let (stored_generation, header) = header
        .split_first_chunk::<32>()
        .ok_or(IndexError::DigestConflict)?;
    let (family, header) = header.split_first().ok_or(IndexError::DigestConflict)?;
    let (key_width, header) = header
        .split_first_chunk::<4>()
        .ok_or(IndexError::DigestConflict)?;
    let (value_width, refs) = header
        .split_first_chunk::<4>()
        .ok_or(IndexError::DigestConflict)?;
    if magic != MAGIC
        || ordinal == 0
        || u32::from_le_bytes(*stored_ordinal) != ordinal
        || *stored_generation != generation_digest(generation)
        || stored.registration.reference != reference
        || stored.registration.bytes
            != serialized_object_bytes(&stored.payload, &stored.registration.children)
    {
        return Err(IndexError::DigestConflict);
    }
    let family = *FAMILIES
        .get(usize::from(*family))
        .ok_or(IndexError::DigestConflict)?;
    let key_bytes =
        usize::try_from(u32::from_le_bytes(*key_width)).map_err(|_| IndexError::ResourceLimited)?;
    let value_bytes = usize::try_from(u32::from_le_bytes(*value_width))
        .map_err(|_| IndexError::ResourceLimited)?;
    let count = part_count(key_bytes, value_bytes)?;
    let children = stored.registration.children.as_slice();
    if refs.len() != count * REF_BYTES
        || children.is_empty()
        || children.len() > count
        || object_ref(&stored.payload, children, budget)? != reference
    {
        return Err(IndexError::DigestConflict);
    }
    let read_ref = |bytes: &[u8]| -> Result<ObjectRef, IndexError> {
        Ok(ObjectRef(Digest(
            bytes.try_into().map_err(|_| IndexError::DigestConflict)?,
        )))
    };
    {
        let _edges = budget.reserve_memory(
            (count * size_of::<ObjectRef>()) as u64,
            MemoryLifetime::Temporary,
        )?;
        let mut expected = reserve(count)?;
        for bytes in refs.chunks_exact(REF_BYTES) {
            expected.push(read_ref(bytes)?);
        }
        expected.sort_unstable();
        expected.dedup();
        if expected != children {
            return Err(IndexError::DigestConflict);
        }
    }
    let row_bytes = key_bytes
        + value_bytes
        + generation.repo_id.as_str().len()
        + generation.revision_id.as_str().len();
    let memory = budget.reserve_memory(row_bytes as u64, MemoryLifetime::Temporary)?;
    let mut read_parts =
        |length: usize, refs: &[u8], budget: &mut WorkBudget| -> Result<Vec<u8>, IndexError> {
            let mut out = reserve(length)?;
            let mut previous: Option<StoredObject> = None;
            for bytes in refs.chunks_exact(REF_BYTES) {
                let reference = read_ref(bytes)?;
                if previous
                    .as_ref()
                    .is_none_or(|part| part.registration.reference != reference)
                {
                    // Keep only one immutable fragment funded at a time. An
                    // adjacent repetition can reuse the authenticated bytes.
                    drop(previous.take());
                    let part = get(reference, budget)?;
                    if part.registration.reference != reference
                        || !part.registration.children.is_empty()
                        || part.payload.len() > PART_BYTES
                        || part.registration.bytes != serialized_object_bytes(&part.payload, &[])
                        || object_ref(&part.payload, &[], budget)? != reference
                    {
                        return Err(IndexError::DigestConflict);
                    }
                    previous = Some(part);
                }
                let part = previous.as_ref().ok_or(IndexError::DigestConflict)?;
                let width = (length - out.len()).min(PART_BYTES);
                if part.payload.len() != width {
                    return Err(IndexError::DigestConflict);
                }
                budget.charge(WorkUsage {
                    bytes: width as u64,
                    ..WorkUsage::default()
                })?;
                out.extend_from_slice(&part.payload);
            }
            Ok(out)
        };
    let (key_refs, value_refs) = refs.split_at(key_bytes.div_ceil(PART_BYTES) * REF_BYTES);
    let row_key = read_parts(key_bytes, key_refs, budget)?;
    let value = read_parts(value_bytes, value_refs, budget)?;
    Ok((
        AuxiliaryRowV1 {
            key: AuxiliaryRowKeyV1 {
                domain: AuxiliaryDomainV1::History,
                generation: generation.clone(),
                family,
                row_key,
            },
            value,
        },
        memory,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{
        MemoryLeaseOwner, MemoryScope, MemoryScopePort, RegisteredObject, WorkLimits,
    };
    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
    use std::{
        cell::Cell,
        collections::BTreeMap,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    #[derive(Clone)]
    struct Memory {
        limit: u64,
        live: Arc<AtomicU64>,
    }

    impl MemoryLeaseOwner for Memory {
        fn release(&self, bytes: u64) {
            assert!(self.live.fetch_sub(bytes, Ordering::Relaxed) >= bytes);
        }
    }

    impl MemoryScopePort for Memory {
        fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            let _previous = self
                .live
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                    live.checked_add(bytes).filter(|total| *total <= self.limit)
                })
                .map_err(|_| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, Arc::new(self.clone())))
        }
    }

    fn work(limit: u64) -> (WorkBudget, Arc<AtomicU64>) {
        let live = Arc::new(AtomicU64::new(0));
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1024,
                writes: 1024,
                bytes: 64 * 1024 * 1024,
                mutations: 1024,
            },
            Instant::now() + Duration::from_secs(30),
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Memory {
            limit,
            live: live.clone(),
        }))));
        (budget, live)
    }

    fn fixture()
    -> Result<(AuxiliaryGenerationKeyV1, HistorySnapshotRowV1), Box<dyn std::error::Error>> {
        Ok((
            AuxiliaryGenerationKeyV1 {
                repo_id: RepoId::new("checkpoint")?,
                revision_id: RevisionId::new("main")?,
                generation: ManifestGeneration::new(7),
            },
            HistorySnapshotRowV1 {
                family: "diff-hunk".into(),
                row_key: b"original key".to_vec(),
                value: vec![120; 5 * PART_BYTES + 3],
            },
        ))
    }

    #[test]
    fn large_history_row_preserves_ordered_repeated_parts_and_refunds()
    -> Result<(), Box<dyn std::error::Error>> {
        let (generation, row) = fixture()?;
        let (mut budget, live) = work(64 * 1024 * 1024);
        let mut objects = BTreeMap::new();
        let reference = encode_large_row_v2(
            &generation,
            2,
            &row,
            &mut budget,
            |reference, payload, children, _| {
                assert!(children.iter().all(|child| objects.contains_key(child)));
                let replaced = objects.insert(
                    reference,
                    StoredObject::new(
                        RegisteredObject::unscoped(
                            reference,
                            serialized_object_bytes(payload, children),
                            children.to_vec(),
                        ),
                        payload.to_vec(),
                        None,
                    ),
                );
                assert!(replaced.is_none());
                Ok(())
            },
        )?;
        assert_eq!(
            objects.len(),
            4,
            "one key, one repeated part, one tail and one directory"
        );
        assert_eq!(live.load(Ordering::Relaxed), 0);
        let root = objects.get(&reference).ok_or(IndexError::DigestConflict)?;
        assert_eq!(root.registration.children.len(), 3);
        assert_eq!(root.payload.len(), HEADER_BYTES + 7 * REF_BYTES);
        let loaded = Cell::new(0);
        let mut get = |object, _: &mut WorkBudget| {
            loaded.set(loaded.get() + 1);
            objects
                .get(&object)
                .cloned()
                .ok_or(IndexError::DigestConflict)
        };
        let restored = decode_large_row_v2(&generation, 2, reference, root, &mut budget, &mut get)?;
        assert_eq!(restored.0.key.family, AuxiliaryRowFamilyV1::DiffHunk);
        assert_eq!(restored.0.key.row_key, row.row_key);
        assert_eq!(restored.0.value, row.value);
        assert_eq!(loaded.get(), 3, "the repeated part is only read once");
        assert!(live.load(Ordering::Relaxed) >= row.value.len() as u64);
        drop(restored);
        assert_eq!(live.load(Ordering::Relaxed), 0);
        let (mut limited, limited_live) = work(4 * 1024 * 1024);
        assert!(matches!(
            decode_large_row_v2(&generation, 2, reference, root, &mut limited, &mut get),
            Err(IndexError::ResourceLimited)
        ));
        assert_eq!(limited_live.load(Ordering::Relaxed), 0);
        assert!(matches!(
            decode_large_row_v2(&generation, 3, reference, root, &mut budget, &mut get),
            Err(IndexError::DigestConflict)
        ));
        let mut wrong_edges = root.registration.children.clone();
        wrong_edges.reverse();
        let wrong_reference = object_ref(&root.payload, &wrong_edges, &mut budget)?;
        let wrong_root = StoredObject::new(
            RegisteredObject::unscoped(
                wrong_reference,
                serialized_object_bytes(&root.payload, &wrong_edges),
                wrong_edges,
            ),
            root.payload.clone(),
            None,
        );
        assert!(matches!(
            decode_large_row_v2(
                &generation,
                2,
                wrong_reference,
                &wrong_root,
                &mut budget,
                &mut get,
            ),
            Err(IndexError::DigestConflict)
        ));
        let mut foreign_part = |requested, work: &mut WorkBudget| {
            let part = get(requested, work)?;
            Ok(StoredObject::new(
                part.registration.clone(),
                vec![0; part.payload.len()],
                None,
            ))
        };
        assert!(matches!(
            decode_large_row_v2(
                &generation,
                2,
                reference,
                root,
                &mut budget,
                &mut foreign_part
            ),
            Err(IndexError::DigestConflict)
        ));
        let tail = object_ref(&[120; 3], &[], &mut budget)?;
        let mut missing_tail = |requested, work: &mut WorkBudget| {
            if requested == tail {
                return Err(IndexError::SnapshotUnavailable);
            }
            get(requested, work)
        };
        assert!(matches!(
            decode_large_row_v2(
                &generation,
                2,
                reference,
                root,
                &mut budget,
                &mut missing_tail
            ),
            Err(IndexError::SnapshotUnavailable)
        ));
        assert_eq!(live.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
