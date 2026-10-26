//! Immutable canonical Text result with allocation custody through emission.

use quanta_index_contract::{
    index::IndexInstanceId,
    native_protocol::{MAX_NATIVE_TEXT_TOP_K, NativeTextCoverage, NativeTextHit, NativeTextResult},
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, QueryCoverage, QueryOutcome,
};
use quanta_index_lexical::native::{NativeTextHitSource, TextReadResult};

use crate::native_result::{ResultValue, ScopedNativeResult, shared_value_bytes};

pub type ScopedNativeTextResult = ScopedNativeResult<NativeTextResult>;

pub(super) fn validate_read(
    outcome: &QueryOutcome<TextReadResult>,
    requested_top_k: u32,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<(), IndexError> {
    if !matches!(outcome.coverage(), QueryCoverage::Core) || !outcome.exhaustive() {
        return Err(IndexError::BackendViolation(
            "native text reader did not return exhaustive core coverage".into(),
        ));
    }
    let result = outcome.result();
    let count = result.hits.len();
    let hit_count = u64::try_from(count)
        .map_err(|_error| IndexError::BackendViolation("native text hit count overflow".into()))?;
    if !(1..=MAX_NATIVE_TEXT_TOP_K).contains(&requested_top_k)
        || hit_count != result.matched.min(u64::from(requested_top_k))
    {
        return Err(IndexError::BackendViolation(
            "native text reader returned inconsistent match cardinality".into(),
        ));
    }
    // Validate duplicates in a bounded admitted array rather than allocating
    // an uncharged BTreeSet after the execution permit has ended.
    let owner_bytes = count
        .checked_mul(std::mem::size_of::<quanta_index_core::index::OwnerKey>())
        .ok_or(IndexError::ResourceLimited)?;
    let _owner_memory = if owner_bytes == 0 {
        None
    } else {
        reserve(
            u64::try_from(owner_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
    };
    let mut owners = Vec::new();
    owners
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if owners.capacity() > count {
        return Err(IndexError::ResourceLimited);
    }
    for hit in &result.hits {
        if !super::native::valid_native_source_path(&hit.path) {
            return Err(IndexError::BackendViolation(
                "native text reader returned invalid path".into(),
            ));
        }
        owners.push(hit.owner);
    }
    owners.sort_unstable();
    if owners
        .iter()
        .zip(owners.iter().skip(1))
        .any(|(prior, next)| prior == next)
    {
        return Err(IndexError::BackendViolation(
            "native text reader returned repeated owner".into(),
        ));
    }
    for (prior, next) in result.hits.iter().zip(result.hits.iter().skip(1)) {
        let order = prior.score.get().total_cmp(&next.score.get());
        if order.is_lt() || order.is_eq() && prior.owner >= next.owner {
            return Err(IndexError::BackendViolation(
                "native text reader returned noncanonical rank order".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn convert(
    outcome: QueryOutcome<TextReadResult>,
    instance: IndexInstanceId,
    requested_top_k: u32,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<ScopedNativeTextResult, IndexError> {
    validate_read(&outcome, requested_top_k, &reserve)?;
    let view = outcome.view();
    let result = outcome.into_result();
    let count = result.hits.len();
    let value_bytes = shared_value_bytes::<NativeTextResult>()?;
    let capacity_bytes = count
        .checked_mul(
            std::mem::size_of::<NativeTextHit>()
                .checked_add(std::mem::size_of::<Option<MemoryLease>>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .and_then(|bytes| bytes.checked_add(value_bytes))
        .ok_or(IndexError::ResourceLimited)?;
    let capacity = reserve(
        u64::try_from(capacity_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let mut hits = Vec::new();
    hits.try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if hits.capacity() > count {
        return Err(IndexError::ResourceLimited);
    }
    let mut paths = Vec::new();
    paths
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if paths.capacity() > count {
        return Err(IndexError::ResourceLimited);
    }
    // The partial result owns data before leases from the first moved hit.
    // Unwind must not release a path grant while its String is still live.
    let mut converted = ResultValue {
        result: NativeTextResult {
            view,
            instance,
            coverage: NativeTextCoverage::CoreRequired,
            artifact: result.artifact,
            matched: result.matched,
            exhaustive: true,
            hits,
        },
        payload_memory: paths,
        _capacity_memory: capacity,
    };
    let parts = result.into_parts();
    let old_hit_memory = parts.3;
    let input_hits = parts.2;
    for hit in input_hits {
        let parts = hit.into_parts();
        // A refused origin still destroys its String before its path grant.
        let memory = parts.4;
        let path = parts.2;
        let owner = parts.0;
        let source = parts.1;
        let score = parts.3;
        let NativeTextHitSource::Durable(source) = source else {
            return Err(IndexError::BackendViolation(
                "durable text route returned an unsaved overlay hit".into(),
            ));
        };
        converted.result.hits.push(NativeTextHit {
            owner,
            source,
            path,
            score,
        });
        converted.payload_memory.push(memory);
    }
    // The consumed input iterator and its backing are gone before this refund.
    drop(old_hit_memory);
    Ok(ScopedNativeResult::new(converted))
}
