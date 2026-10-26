//! Source-bound unsaved semantic output; never creates immutable source refs.
use crate::{
    native_overlay::OverlayCorpusSnapshot,
    native_result::{ResultValue, ScopedNativeResult, shared_value_bytes},
    native_semantic_result::ScopedOverlaySemanticResult,
};
use quanta_index_contract::{
    index::IndexInstanceId,
    native_protocol::{
        MAX_NATIVE_SEMANTIC_OWNER_SCAN, MAX_NATIVE_SEMANTIC_TOP_K, NativeOverlaySemanticChunk,
        NativeOverlaySemanticHit, NativeOverlaySemanticHitOrigin, NativeOverlaySemanticResult,
    },
};
use quanta_index_core::index::{
    CoverageStatus, IndexError, MemoryLease, MemoryLifetime, QueryCoverage, QueryOutcome,
};
use quanta_index_semantic::native::SearchResult;
use sha2::{Digest as _, Sha256};

pub(crate) fn convert(
    outcome: QueryOutcome<SearchResult>,
    instance: IndexInstanceId,
    top_k: u32,
    snapshot: &OverlayCorpusSnapshot,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<ScopedOverlaySemanticResult, IndexError> {
    let QueryCoverage::Async(coverage) = outcome.coverage() else {
        return Err(IndexError::BackendViolation(
            "overlay Deep has no durable coverage binding".into(),
        ));
    };
    let coverage = **coverage;
    let view = outcome.view();
    let exhaustive = outcome.exhaustive();
    let result = outcome.into_result();
    if view.workspace != snapshot.session.workspace
        || coverage.instance != instance
        || (coverage.owners.0.is_some() && coverage.searchable.is_none())
        || !(1..=MAX_NATIVE_SEMANTIC_TOP_K).contains(&top_k)
        || result.exhaustive != exhaustive
        || result
            .traversal
            .certifies_exhaustive(&coverage, result.skipped_stale)
            != exhaustive
        || exhaustive && !matches!(coverage.status, CoverageStatus::Complete)
        || matches!(coverage.status, CoverageStatus::Complete)
            && coverage.covered != Some(coverage.desired)
        || result.scanned_owners > MAX_NATIVE_SEMANTIC_OWNER_SCAN
        || result.skipped_stale > result.scanned_owners
        || exhaustive && result.skipped_stale != 0
        || u64::try_from(result.hits.len()).map_err(|_error| IndexError::ResourceLimited)?
            > u64::from(top_k)
    {
        return Err(IndexError::BackendViolation(
            "overlay Deep coverage or counters differ".into(),
        ));
    }
    super::validate_hits(&result, coverage.desired.policy, &reserve)?;
    super::validate_order(&result)?;
    for hit in &result.hits {
        let row = snapshot
            .documents
            .iter()
            .find(|row| row.owner == hit.owner && !row.retired);
        match hit.overlay {
            Some(origin) => {
                if hit.chunk.is_some()
                    || row.is_none_or(|row| row.revision != origin.revision || row.text.is_none())
                {
                    return Err(IndexError::BackendViolation(
                        "overlay Deep source revision differs".into(),
                    ));
                }
                if let Some(chunk) = origin.chunk {
                    let text = row
                        .and_then(|row| row.text.as_deref())
                        .ok_or(IndexError::Conflict)?;
                    let start = usize::try_from(chunk.start)
                        .map_err(|_error| IndexError::ResourceLimited)?;
                    let end =
                        usize::try_from(chunk.end).map_err(|_error| IndexError::ResourceLimited)?;
                    let input = text.get(start..end).ok_or_else(|| {
                        IndexError::BackendViolation("overlay Deep chunk escaped input".into())
                    })?;
                    if hit.vector_id != chunk.id.0.as_slice()
                        || chunk.input_digest.0
                            != <[u8; 32]>::from(Sha256::digest(input.as_bytes()))
                        || start > end
                        || start == end && (start != 0 || !text.is_empty())
                    {
                        return Err(IndexError::BackendViolation(
                            "overlay Deep chunk membership differs".into(),
                        ));
                    }
                } else if hit.vector_id != b"whole-file" {
                    return Err(IndexError::BackendViolation(
                        "overlay Deep whole-file identity differs".into(),
                    ));
                }
            }
            None => {
                if row.is_some() {
                    return Err(IndexError::BackendViolation(
                        "masked durable semantic hit escaped".into(),
                    ));
                }
            }
        }
    }
    let count = result.hits.len();
    let leases = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    let value_bytes = shared_value_bytes::<NativeOverlaySemanticResult>()?;
    let bytes = count
        .checked_mul(std::mem::size_of::<NativeOverlaySemanticHit>())
        .and_then(|b| {
            leases
                .checked_mul(std::mem::size_of::<Option<MemoryLease>>())
                .and_then(|m| b.checked_add(m))
        })
        .and_then(|b| b.checked_add(value_bytes))
        .ok_or(IndexError::ResourceLimited)?;
    let memory = reserve(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let (corpus, corpus_memory) = crate::native_overlay_query::stamp(snapshot, &reserve)?;
    let traversal = result.traversal;
    let scanned_owners = result.scanned_owners;
    let skipped_stale = result.skipped_stale;
    let (input_hits, old_memory) = result.into_parts();
    let mut hits = Vec::new();
    hits.try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut payload_memory = Vec::new();
    payload_memory
        .try_reserve_exact(leases)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut output = ResultValue {
        result: NativeOverlaySemanticResult {
            view,
            instance,
            coverage,
            traversal,
            exhaustive,
            scanned_owners,
            skipped_stale,
            corpus,
            hits,
        },
        payload_memory,
        _capacity_memory: memory,
    };
    output.payload_memory.push(corpus_memory);
    for hit in input_hits {
        let overlay = hit.overlay.map(|origin| NativeOverlaySemanticHitOrigin {
            revision: origin.revision,
            chunk: origin.chunk.map(|chunk| NativeOverlaySemanticChunk {
                id: chunk.id,
                input_digest: chunk.input_digest,
                start: chunk.start,
                end: chunk.end,
            }),
        });
        let (owner, vector_id, cosine, chunk, memory) = hit.into_parts();
        output.result.hits.push(NativeOverlaySemanticHit {
            owner,
            vector_id,
            score: super::checked_score(cosine)?,
            chunk,
            overlay,
        });
        output.payload_memory.push(memory);
    }
    drop(old_memory);
    Ok(ScopedNativeResult::new(output))
}
