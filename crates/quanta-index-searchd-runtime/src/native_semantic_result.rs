//! Canonical exact-vector output retains IDs and capacity through emission.

use quanta_index_contract::{
    index::IndexInstanceId,
    native_protocol::{
        MAX_NATIVE_SEMANTIC_OWNER_SCAN, MAX_NATIVE_SEMANTIC_TOP_K,
        MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES, NativeSemanticHit, NativeSemanticResult,
    },
};
use quanta_index_core::index::{
    CoverageStatus, FiniteScore, IndexError, MemoryLease, MemoryLifetime, QueryCoverage,
    QueryOutcome,
};
use quanta_index_semantic::native::SearchResult;

use crate::native_result::{ResultValue, ScopedNativeResult, shared_value_bytes};

mod overlay;
pub(super) use overlay::convert as convert_overlay;
pub type ScopedOverlaySemanticResult =
    ScopedNativeResult<quanta_index_contract::native_protocol::NativeOverlaySemanticResult>;

pub type ScopedNativeSemanticResult = ScopedNativeResult<NativeSemanticResult>;

pub(super) fn convert(
    outcome: QueryOutcome<SearchResult>,
    instance: IndexInstanceId,
    top_k: u32,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<ScopedNativeSemanticResult, IndexError> {
    let QueryCoverage::Async(coverage) = outcome.coverage() else {
        return Err(IndexError::BackendViolation(
            "native semantic reader did not return async coverage".into(),
        ));
    };
    let coverage = **coverage;
    let view = outcome.view();
    let exhaustive = outcome.exhaustive();
    let result = outcome.into_result();
    let complete = matches!(coverage.status, CoverageStatus::Complete);
    if !(1..=MAX_NATIVE_SEMANTIC_TOP_K).contains(&top_k)
        || coverage.instance != instance
        || (coverage.owners.0.is_some() && coverage.searchable.is_none())
        || (complete && coverage.covered != Some(coverage.desired))
        || (exhaustive && !complete)
        || (coverage.owners.0.is_none()
            && (result.scanned_owners != 0 || result.skipped_stale != 0 || !result.hits.is_empty()))
        || result.exhaustive != exhaustive
        || result
            .traversal
            .certifies_exhaustive(&coverage, result.skipped_stale)
            != exhaustive
        || result.skipped_stale > result.scanned_owners
        || (exhaustive && result.skipped_stale != 0)
        || result.scanned_owners > MAX_NATIVE_SEMANTIC_OWNER_SCAN
        || u64::try_from(result.hits.len()).map_err(|_error| IndexError::ResourceLimited)?
            > u64::from(top_k)
    {
        return Err(IndexError::BackendViolation(
            "native semantic reader returned inconsistent scan".into(),
        ));
    }
    validate_hits(&result, coverage.desired.policy, &reserve)?;
    validate_order(&result)?;
    if result.hits.iter().any(|hit| hit.overlay.is_some()) {
        return Err(IndexError::BackendViolation(
            "durable semantic route returned an unsaved hit".into(),
        ));
    }
    let count = result.hits.len();
    let value_bytes = shared_value_bytes::<NativeSemanticResult>()?;
    let capacity_bytes = count
        .checked_mul(
            std::mem::size_of::<NativeSemanticHit>()
                .checked_add(std::mem::size_of::<Option<MemoryLease>>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .and_then(|bytes| bytes.checked_add(value_bytes))
        .ok_or(IndexError::ResourceLimited)?;
    let capacity_memory = reserve(
        u64::try_from(capacity_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let mut hits = Vec::new();
    hits.try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut payload_memory = Vec::new();
    payload_memory
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let traversal = result.traversal;
    let scanned_owners = result.scanned_owners;
    let skipped_stale = result.skipped_stale;
    let mut converted = ResultValue {
        result: NativeSemanticResult {
            view,
            instance,
            coverage,
            traversal,
            exhaustive,
            scanned_owners,
            skipped_stale,
            hits,
        },
        payload_memory,
        _capacity_memory: capacity_memory,
    };
    let (input_hits, old_capacity) = result.into_parts();
    for hit in input_hits {
        let (owner, vector_id, cosine, chunk, memory) = hit.into_parts();
        converted.result.hits.push(NativeSemanticHit {
            owner,
            vector_id,
            score: checked_score(cosine)?,
            chunk,
        });
        converted.payload_memory.push(memory);
    }
    drop(old_capacity);
    Ok(ScopedNativeResult::new(converted))
}

fn validate_hits(
    result: &SearchResult,
    recipe: quanta_index_core::index::Digest,
    reserve: &impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<(), IndexError> {
    let count = result.hits.len();
    let validation_bytes = count
        .checked_mul(std::mem::size_of::<&quanta_index_semantic::native::SearchHit>())
        .ok_or(IndexError::ResourceLimited)?;
    let _validation_memory = if validation_bytes == 0 {
        None
    } else {
        reserve(
            u64::try_from(validation_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
    };
    let mut identities = Vec::new();
    identities
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    identities.extend(result.hits.iter());
    identities.sort_unstable_by(|left, right| {
        left.owner
            .cmp(&right.owner)
            .then_with(|| left.vector_id.cmp(&right.vector_id))
    });
    let mut prior = None;
    let mut fresh_hit_owners = 0_u32;
    for hit in identities {
        if let Some(chunk) = hit.chunk {
            chunk
                .validate_binding(&hit.vector_id, recipe)
                .map_err(|error| {
                    IndexError::BackendViolation(format!("native semantic chunk: {error}"))
                })?;
        }
        if hit.vector_id.is_empty()
            || hit.vector_id.len() > MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES
            || !(-1.0..=1.0).contains(&hit.cosine)
        {
            return Err(IndexError::BackendViolation(
                "native semantic hit has invalid identity or score".into(),
            ));
        }
        if prior.is_some_and(|(owner, id)| owner == hit.owner && id == hit.vector_id.as_slice()) {
            return Err(IndexError::BackendViolation(
                "native semantic reader repeated a vector identity".into(),
            ));
        }
        if prior.is_none_or(|(owner, _)| owner != hit.owner) {
            fresh_hit_owners = fresh_hit_owners
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        prior = Some((hit.owner, hit.vector_id.as_slice()));
    }
    if fresh_hit_owners
        > result
            .scanned_owners
            .checked_sub(result.skipped_stale)
            .ok_or(IndexError::ResourceLimited)?
    {
        return Err(IndexError::BackendViolation(
            "native semantic reader returned hits beyond fresh scanned owners".into(),
        ));
    }
    Ok(())
}

fn validate_order(result: &SearchResult) -> Result<(), IndexError> {
    for (prior, next) in result.hits.iter().zip(result.hits.iter().skip(1)) {
        let order = checked_score(prior.cosine)?
            .get()
            .total_cmp(&checked_score(next.cosine)?.get());
        if order.is_lt()
            || (order.is_eq()
                && (prior.owner > next.owner
                    || (prior.owner == next.owner && prior.vector_id >= next.vector_id)))
        {
            return Err(IndexError::BackendViolation(
                "native semantic reader returned noncanonical rank order".into(),
            ));
        }
    }
    Ok(())
}

fn checked_score(cosine: f64) -> Result<FiniteScore, IndexError> {
    FiniteScore::new(cosine).map_err(|error| IndexError::BackendViolation(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{
        index::{Digest, ObjectRef, OwnerInputBinding, OwnerKey, ProducerId, StableOwnerId},
        native_protocol::{NativeSemanticChunk, NativeSemanticChunkSpan},
    };
    use quanta_index_semantic::native::SearchHit;

    fn chunk() -> NativeSemanticChunk {
        NativeSemanticChunk {
            id: Digest([1; 32]),
            payload: ObjectRef(Digest([2; 32])),
            input: OwnerInputBinding {
                source: Some(ObjectRef(Digest([3; 32]))),
                projection: None,
                producer_context: Digest([0; 32]),
                derivation_policy: Digest([4; 32]),
            },
            input_digest: Digest([5; 32]),
            span: NativeSemanticChunkSpan::SourcePayload {
                start_byte: 0,
                end_byte: 4,
            },
        }
    }

    fn hit(chunk: NativeSemanticChunk) -> SearchHit {
        let mut hit = SearchHit::from_parts(
            OwnerKey {
                producer: ProducerId([1; 16]),
                stable_id: StableOwnerId([2; 16]),
            },
            chunk.id.0.to_vec(),
            0.5,
            None,
        );
        hit.chunk = Some(chunk);
        hit
    }

    #[test]
    fn selected_chunk_metadata_survives_owned_hit_transfer() -> Result<(), IndexError> {
        let chunk = chunk();
        let hit = hit(chunk);
        let (_, id, _, selected, memory) = hit.into_parts();
        if id != chunk.id.0 || selected != Some(chunk) || memory.is_some() {
            return Err(IndexError::BackendViolation(
                "chunk transfer changed selected metadata".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn conversion_gate_rejects_unbound_chunk_before_emission() -> Result<(), IndexError> {
        let original = chunk();
        let valid = SearchResult::from_parts(
            vec![hit(original)],
            1,
            0,
            quanta_index_contract::native_protocol::NativeSemanticTraversal::ExactExhausted,
            true,
            None,
        );
        validate_hits(&valid, Digest([4; 32]), &|_bytes, _lifetime| Ok(None))?;
        for forged in [
            NativeSemanticChunk {
                input: OwnerInputBinding {
                    derivation_policy: Digest([9; 32]),
                    ..original.input
                },
                ..original
            },
            NativeSemanticChunk {
                span: NativeSemanticChunkSpan::ProjectionPayload {
                    start_byte: 0,
                    end_byte: 4,
                },
                ..original
            },
            NativeSemanticChunk {
                span: NativeSemanticChunkSpan::SourcePayload {
                    start_byte: 5,
                    end_byte: 4,
                },
                ..original
            },
        ] {
            let result = SearchResult::from_parts(
                vec![hit(forged)],
                1,
                0,
                quanta_index_contract::native_protocol::NativeSemanticTraversal::ExactExhausted,
                true,
                None,
            );
            if !matches!(
                validate_hits(&result, Digest([4; 32]), &|_bytes, _lifetime| Ok(None)),
                Err(IndexError::BackendViolation(_))
            ) {
                return Err(IndexError::BackendViolation(
                    "conversion accepted an unbound chunk".into(),
                ));
            }
        }
        Ok(())
    }
}
