//! Authenticated file metadata on the canonical semantic hit, before its window.

use super::*;

/// Identity comes from the selected source-owner map, never from a path or a
/// projection owner's stable ID. The record retains its original path grant.
#[derive(Debug)]
pub struct SemanticSourceFile {
    pub owner: OwnerKey,
    pub source: ObjectRef,
    record: FileRecord,
}

#[derive(Debug)]
pub(super) enum FileRecord {
    Owned(SourceRecordOwner),
    Shared(Arc<SharedFileRecord>),
}

impl std::ops::Deref for FileRecord {
    type Target = SourceRecord;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(record) => record,
            Self::Shared(record) => &record.record,
        }
    }
}

impl FileRecord {
    fn into_shared(self) -> Result<Self, IndexError> {
        let Self::Owned(record) = self else {
            return Ok(self);
        };
        let backing = MemoryLease::shared_allocation_backing_layout(std::alloc::Layout::new::<
            SharedFileRecord,
        >())?;
        if record.covers_shared_backing(backing.size()) {
            Ok(Self::Shared(Arc::new(SharedFileRecord {
                record,
                _memory: None,
            })))
        } else {
            Ok(Self::Owned(record))
        }
    }
}

#[derive(Debug)]
pub(super) struct SharedFileRecord {
    record: SourceRecordOwner,
    _memory: Option<MemoryLease>,
}

impl SharedFileRecord {
    pub(super) fn retain(
        record: SourceRecordOwner,
        budget: &WorkBudget,
    ) -> Result<Arc<Self>, IndexError> {
        let backing =
            MemoryLease::shared_allocation_backing_layout(std::alloc::Layout::new::<Self>())?;
        let memory = budget.reserve_memory(
            u64::try_from(backing.size()).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?;
        Ok(Arc::new(Self {
            record,
            _memory: memory,
        }))
    }
}

impl PartialEq for SemanticSourceFile {
    fn eq(&self, other: &Self) -> bool {
        self.owner == other.owner && self.source == other.source && *self.record == *other.record
    }
}
impl Eq for SemanticSourceFile {}

impl SemanticSourceFile {
    #[must_use]
    pub fn path(&self) -> &str {
        &self.record.path
    }
}

/// Borrow an authenticated record from the live file window by its exact
/// selected source identity. The candidate must still validate its own input.
pub(super) fn retained_record(
    heap: &BinaryHeap<RankedHit>,
    owner: OwnerKey,
    source: ObjectRef,
) -> Option<FileRecord> {
    let file = heap
        .iter()
        .filter_map(|hit| hit.0.source_file())
        .find(|file| file.owner == owner && file.source == source)?;
    let FileRecord::Shared(record) = &file.record else {
        return None;
    };
    Some(FileRecord::Shared(Arc::clone(record)))
}

/// The vector identity remains projection provenance. The nested file owner
/// and vector ID keep their producing grants through the consumer conversion.
#[derive(Debug)]
pub struct SemanticSourceFileHitParts {
    pub owner: OwnerKey,
    pub vector_id: Vec<u8>,
    pub cosine: f64,
    pub chunk: Option<NativeSemanticChunk>,
    pub source_file: SemanticSourceFile,
    pub vector_id_memory: Option<MemoryLease>,
}

impl SearchHit {
    #[must_use]
    pub fn source_file(&self) -> Option<&SemanticSourceFile> {
        self.source_file.as_ref()
    }

    pub fn into_source_file_parts(self) -> Result<SemanticSourceFileHitParts, IndexError> {
        let mut value = self;
        if value.overlay.is_some() {
            return Err(IndexError::BackendViolation(
                "file semantic hit carries overlay".into(),
            ));
        }
        let Some(source_file) = value.source_file.take() else {
            return Err(IndexError::BackendViolation(
                "file semantic hit lacks source authority".into(),
            ));
        };
        Ok(SemanticSourceFileHitParts {
            owner: value.owner,
            vector_id: std::mem::take(&mut value.vector_id),
            cosine: value.cosine,
            chunk: value.chunk,
            source_file,
            vector_id_memory: value.vector_id_memory.take(),
        })
    }
}

pub(super) fn load(
    member: &OwnerMemberBinding,
    contribution: Option<ProjectionOwnerContribution>,
    authenticated_record: Option<FileRecord>,
    query: &SearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<Option<SemanticSourceFile>, IndexError> {
    let (owner, reference) = match contribution {
        Some(contribution) => match (contribution.source_owner, contribution.source) {
            (Some(owner), Some(reference)) if member.input.source == Some(reference) => {
                (owner, reference)
            }
            (None, None) if member.input.source.is_none() => return Ok(None),
            _ => {
                return Err(IndexError::CorruptObject(
                    "file projection source identity differs".into(),
                ));
            }
        },
        None => (
            member.owner,
            member.input.source.ok_or_else(|| {
                IndexError::CorruptObject("file semantic source is absent".into())
            })?,
        ),
    };
    if context
        .maps
        .lookup_object(query.source_owners, &owner_map_key(owner), context.budget)?
        != Some(reference)
    {
        return Err(IndexError::CorruptObject(
            "file semantic source is outside the selected view".into(),
        ));
    }
    let record = match authenticated_record {
        Some(record) => record,
        None => FileRecord::Owned(load_source_record_with_lifetime(
            reference,
            context.objects,
            context.budget,
            MemoryLifetime::Retained,
        )?),
    };
    if query
        .exact_path
        .is_some_and(|path| path != record.path.as_str())
    {
        return Ok(None);
    }
    Ok(Some(SemanticSourceFile {
        owner,
        source: reference,
        record: record.into_shared()?,
    }))
}

/// Choose max cosine per source file before applying the bounded file window.
/// The heap's ordering supplies source-owner and vector provenance tie breaks.
pub(super) fn admit(
    mut hit: RankedHit,
    top_k: usize,
    heap: &mut BinaryHeap<RankedHit>,
    context: &mut ReadContext<'_>,
) -> Result<(), IndexError> {
    if let Some(file) = hit.0.source_file.as_ref()
        && let Some(prior) = heap.iter().find(|prior| {
            prior
                .0
                .source_file
                .as_ref()
                .is_some_and(|other| other.owner == file.owner)
        })
    {
        if prior
            .0
            .source_file
            .as_ref()
            .is_some_and(|other| other.source != file.source)
        {
            return Err(IndexError::CorruptObject(
                "same-view semantic file has two source refs".into(),
            ));
        }
        if hit >= *prior {
            return Ok(());
        }
        let owner = file.owner;
        heap.retain(|entry| {
            !entry
                .0
                .source_file
                .as_ref()
                .is_some_and(|file| file.owner == owner)
        });
    }
    if heap.len() < top_k || heap.peek().is_some_and(|worst| hit < *worst) {
        let bytes =
            u64::try_from(hit.0.vector_id.capacity()).map_err(|_| IndexError::ResourceLimited)?;
        // Release the displaced hit before charging the replacement.
        if heap.len() == top_k {
            drop(heap.pop());
        }
        hit.0.vector_id_memory = context
            .budget
            .reserve_memory(bytes, MemoryLifetime::Retained)?;
        heap.push(hit);
    }
    Ok(())
}

/// Exact within one admitted member (or its explicit ANN vector candidate).
/// Select before transferring any ID/path backing into the bounded file heap.
pub(super) fn best_vector(
    vectors: &[EmbeddingVector],
    only: Option<&[u8]>,
    query: &SearchQuery<'_>,
    query_norm: f64,
    context: &mut ReadContext<'_>,
) -> Result<Option<(usize, f64)>, IndexError> {
    let mut best: Option<(usize, f64)> = None;
    for (index, vector) in vectors.iter().enumerate() {
        if only.is_some_and(|id| id != vector.id.as_slice()) {
            continue;
        }
        context.budget.charge(WorkUsage {
            mutations: 1,
            ..WorkUsage::default()
        })?;
        let score = cosine(query.vector, query_norm, &vector.values)?;
        if best.is_none_or(|(prior, value)| {
            score.total_cmp(&value).is_gt()
                || (score.total_cmp(&value).is_eq() && vector.id < vectors[prior].id)
        }) {
            best = Some((index, score));
        }
    }
    Ok(best)
}
