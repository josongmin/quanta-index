//! Converts authenticated reader outputs while the final execution grant lives.

use crate::native_result::{ResultValue, ScopedNativeResult, shared_value_bytes};
use quanta_index_contract::{
    QueryConstraintSetV1,
    index::IndexInstanceId,
    native_protocol::{
        MAX_NATIVE_SEMANTIC_OWNER_SCAN, MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES,
        NativeHybridFileResult, NativeReadSessionId, NativeSemanticFileHit, NativeTextHit,
        rank_native_hybrid_files,
    },
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, QueryCoverage, QueryOutcome, WorkBudget, WorkUsage,
};
use quanta_index_lexical::native::{NativeTextHitSource, TextReadResult};
use quanta_index_semantic::native::SearchResult;

pub(crate) type ScopedNativeHybridFileResult = ScopedNativeResult<NativeHybridFileResult>;

pub(crate) fn convert(
    text: QueryOutcome<TextReadResult>,
    semantic: QueryOutcome<SearchResult>,
    session: NativeReadSessionId,
    text_instance: IndexInstanceId,
    semantic_instance: IndexInstanceId,
    file_top_k: u32,
    dense_limit: u32,
    constraints: &QueryConstraintSetV1,
    work: &mut WorkBudget,
) -> Result<ScopedNativeHybridFileResult, IndexError> {
    let QueryCoverage::Async(coverage) = semantic.coverage() else {
        return Err(IndexError::BackendViolation(
            "Hybrid dense reader omitted async coverage".into(),
        ));
    };
    let coverage = **coverage;
    let raw = semantic.result();
    if text.view() != semantic.view()
        || coverage.instance != semantic_instance
        || raw.hits.len() as u64 > u64::from(dense_limit)
        || raw.scanned_owners > MAX_NATIVE_SEMANTIC_OWNER_SCAN
        || raw.skipped_stale > raw.scanned_owners
        || raw.hits.len() as u64 > u64::from(raw.scanned_owners.saturating_sub(raw.skipped_stale))
        || raw.exhaustive != semantic.exhaustive()
        || raw.exhaustive
            != raw
                .traversal
                .certifies_exhaustive(&coverage, raw.skipped_stale)
        || (coverage.owners.0.is_some() && coverage.searchable.is_none())
        || (coverage.owners.0.is_none() && (raw.scanned_owners != 0 || !raw.hits.is_empty()))
    {
        return Err(IndexError::BackendViolation(
            "Hybrid dense scan identity or coverage differs".into(),
        ));
    }
    let lexical_count = text.result().hits.len();
    let dense_count = raw.hits.len();
    let union_capacity = lexical_count
        .checked_add(dense_count)
        .ok_or(IndexError::ResourceLimited)?;
    let lease_count = dense_count
        .checked_mul(2)
        .and_then(|count| count.checked_add(lexical_count))
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = lexical_count
        .checked_mul(std::mem::size_of::<NativeTextHit>())
        .and_then(|bytes| {
            bytes
                .checked_add(dense_count.checked_mul(std::mem::size_of::<NativeSemanticFileHit>())?)
        })
        .and_then(|bytes| {
            bytes.checked_add(union_capacity.checked_mul(std::mem::size_of::<
                quanta_index_contract::native_protocol::NativeHybridFileHit,
            >())?)
        })
        .and_then(|bytes| {
            bytes.checked_add(lease_count.checked_mul(std::mem::size_of::<Option<MemoryLease>>())?)
        })
        .and_then(|bytes| bytes.checked_add(shared_value_bytes::<NativeHybridFileResult>().ok()?))
        .ok_or(IndexError::ResourceLimited)?;
    let memory = work.reserve_memory(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let mut lexical_pool = Vec::new();
    let mut dense_pool = Vec::new();
    let mut hits = Vec::new();
    let mut leases = Vec::new();
    lexical_pool
        .try_reserve_exact(lexical_count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    dense_pool
        .try_reserve_exact(dense_count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    hits.try_reserve_exact(union_capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    leases
        .try_reserve_exact(lease_count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut result = ResultValue {
        result: NativeHybridFileResult {
            session,
            view: text.view(),
            text_instance,
            semantic_instance,
            lexical_artifact: text.result().artifact,
            lexical_matched: text.result().matched,
            semantic_coverage: coverage,
            semantic_traversal: raw.traversal,
            semantic_exhaustive: raw.exhaustive,
            scanned_owners: raw.scanned_owners,
            skipped_stale: raw.skipped_stale,
            file_top_k,
            lexical_pool,
            dense_pool,
            hits,
        },
        payload_memory: leases,
        _capacity_memory: memory,
    };
    let (_, _, text_rows, text_memory) = text.into_result().into_parts();
    for row in text_rows {
        let parts = row.into_parts();
        // Preserve the canonical Text mapper's data-before-grant unwind order.
        let lease = parts.4;
        let path = parts.2;
        let (owner, origin, score) = (parts.0, parts.1, parts.3);
        let NativeTextHitSource::Durable(source) = origin else {
            return Err(IndexError::BackendViolation(
                "durable Hybrid returned lexical overlay".into(),
            ));
        };
        if !quanta_index_contract::index::is_canonical_source_path(&path)
            || constraints
                .repo_relative_path_exact
                .as_ref()
                .is_some_and(|expected| expected.as_str() != path)
        {
            return Err(IndexError::BackendViolation(
                "Hybrid lexical file predicate differs".into(),
            ));
        }
        result.result.lexical_pool.push(NativeTextHit {
            owner,
            source,
            path,
            score,
        });
        result.payload_memory.push(lease);
    }
    drop(text_memory);
    let (dense_rows, dense_memory) = semantic.into_result().into_parts();
    for row in dense_rows {
        let parts = row.into_source_file_parts()?;
        let source_path = parts.source_file.path();
        if !quanta_index_contract::index::is_canonical_source_path(source_path)
            || constraints
                .repo_relative_path_exact
                .as_ref()
                .is_some_and(|path| path.as_str() != source_path)
            || parts.vector_id.is_empty()
            || parts.vector_id.len() > MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES
            || !(-1.0..=1.0).contains(&parts.cosine)
        {
            return Err(IndexError::BackendViolation(
                "Hybrid source-file witness is invalid".into(),
            ));
        }
        if let Some(chunk) = parts.chunk {
            chunk
                .validate_file_binding(
                    &parts.vector_id,
                    coverage.desired.policy,
                    parts.source_file.source,
                )
                .map_err(|error| {
                    IndexError::BackendViolation(format!("Hybrid vector witness differs: {error}"))
                })?;
        }
        work.charge(WorkUsage {
            bytes: u64::try_from(source_path.len())
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let path_memory = work.reserve_memory(
            u64::try_from(source_path.len()).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?;
        let mut path = String::new();
        path.try_reserve_exact(source_path.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        path.push_str(source_path);
        result.result.dense_pool.push(NativeSemanticFileHit {
            owner: parts.source_file.owner,
            source: parts.source_file.source,
            path,
            projection_owner: parts.owner,
            vector_id: parts.vector_id,
            chunk: parts.chunk,
            score: super::checked_score(parts.cosine)?,
        });
        result.payload_memory.push(path_memory);
        result.payload_memory.push(parts.vector_id_memory);
    }
    drop(dense_memory);
    rank_native_hybrid_files(
        &result.result.lexical_pool,
        &result.result.dense_pool,
        file_top_k,
        &mut result.result.hits,
    )
    .map_err(IndexError::BackendViolation)?;
    work.checkpoint()?;
    Ok(ScopedNativeResult::new(result))
}
