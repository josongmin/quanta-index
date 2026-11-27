//! Immutable canonical Text result with allocation custody through emission.

use quanta_index_contract::{
    RepoId,
    index::{Digest, IndexInstanceId},
    native_protocol::{
        MAX_NATIVE_TEXT_TOP_K, NativeReadSessionId, NativeTextCoverage, NativeTextHit,
        NativeTextPathCursorV1, NativeTextPathPageV1, NativeTextPathRankingV1,
        NativeTextPathResultV1, NativeTextPreview, NativeTextPreviewResult, NativeTextResult,
        compare_native_text_path_rank_v1,
    },
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, QueryCoverage, QueryOutcome,
};
use quanta_index_lexical::native::{CodeSearchMatch, NativeTextHitSource, TextReadResult};

use crate::native_result::{ResultValue, ScopedNativeResult, shared_value_bytes};

pub type ScopedNativeTextResult = ScopedNativeResult<NativeTextResult>;
pub(super) type ScopedNativeTextPreviewResult = ScopedNativeResult<NativeTextPreviewResult>;
pub(super) type ScopedNativeTextPathResult = ScopedNativeResult<NativeTextPathResultV1>;
pub(super) type ScopedNativeTextPathPage = ScopedNativeResult<NativeTextPathPageV1>;

#[derive(Clone, Copy)]
enum ReadShape {
    Owners,
    Paths,
    PathPage,
}

impl ReadShape {
    fn groups_paths(self) -> bool {
        matches!(self, Self::Paths | Self::PathPage)
    }
}

pub(super) fn validate_read(
    outcome: &QueryOutcome<TextReadResult>,
    requested_top_k: u32,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<(), IndexError> {
    validate_read_shape(outcome, requested_top_k, ReadShape::Owners, &reserve)
}

fn validate_read_shape(
    outcome: &QueryOutcome<TextReadResult>,
    requested_top_k: u32,
    shape: ReadShape,
    reserve: &impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
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
    let eligible = if matches!(shape, ReadShape::PathPage) {
        result.remaining
    } else {
        result.matched
    };
    if !(1..=MAX_NATIVE_TEXT_TOP_K).contains(&requested_top_k)
        || eligible > result.matched
        || hit_count != eligible.min(u64::from(requested_top_k))
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
        if !quanta_index_contract::index::is_canonical_source_path(&hit.path) {
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
    if shape.groups_paths() {
        return validate_path_groups(result, reserve);
    }
    if result.hits.iter().any(|hit| hit.path_group_repository_v1().is_some()) {
        return Err(IndexError::BackendViolation(
            "owner-ranked text reader returned path groups".into(),
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

fn validate_path_groups(
    result: &TextReadResult,
    reserve: &impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<(), IndexError> {
    let count = result.hits.len();
    let bytes = count
        .checked_mul(std::mem::size_of::<(&RepoId, &str)>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or(IndexError::ResourceLimited)?;
    let _scratch = if bytes == 0 {
        None
    } else {
        reserve(bytes, MemoryLifetime::Temporary)?
    };
    let mut groups = Vec::new();
    groups.try_reserve_exact(count).map_err(|_| IndexError::ResourceLimited)?;
    if groups.capacity() > count {
        return Err(IndexError::ResourceLimited);
    }
    let mut prior = None;
    for hit in &result.hits {
        let repository = hit.path_group_repository_v1().ok_or_else(|| {
            IndexError::BackendViolation("native path group omitted its source repository".into())
        })?;
        let rank = (hit.score, repository, hit.path.as_str());
        if hit.score.get() < 0.0
            || prior.is_some_and(|prior| !compare_native_text_path_rank_v1(prior, rank).is_lt())
        {
            return Err(IndexError::BackendViolation(
                "native path groups returned noncanonical rank order".into(),
            ));
        }
        groups.push((repository, hit.path.as_str()));
        prior = Some(rank);
    }
    groups.sort_unstable_by(|left, right| {
        (left.0.as_str(), left.1).cmp(&(right.0.as_str(), right.1))
    });
    if groups.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(IndexError::BackendViolation(
            "native path groups returned a repeated repository path".into(),
        ));
    }
    Ok(())
}

pub(super) fn convert(
    outcome: QueryOutcome<TextReadResult>,
    instance: IndexInstanceId,
    requested_top_k: u32,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<ScopedNativeTextResult, IndexError> {
    convert_impl(
        outcome, instance, requested_top_k, false, ReadShape::Owners, &reserve,
        |ranking, _, _, _| Ok((ranking, None)),
    )
}

pub(super) fn convert_preview(
    outcome: QueryOutcome<TextReadResult>,
    instance: IndexInstanceId,
    requested_top_k: u32,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<ScopedNativeTextPreviewResult, IndexError> {
    convert_impl(outcome, instance, requested_top_k, true, ReadShape::Owners, &reserve, |ranking, _, previews, _| {
        let result = NativeTextPreviewResult {
            ranking,
            previews: previews.ok_or_else(|| {
                IndexError::BackendViolation("native preview reader omitted preview rows".into())
            })?,
        };
        result
            .validate()
            .map_err(|error| IndexError::BackendViolation(error.into()))?;
        Ok((result, None))
    })
}

pub(super) fn convert_paths(
    outcome: QueryOutcome<TextReadResult>,
    instance: IndexInstanceId,
    requested_top_k: u32,
    with_preview: bool,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<ScopedNativeTextPathResult, IndexError> {
    convert_impl(
        outcome, instance, requested_top_k, with_preview, ReadShape::Paths, &reserve,
        |ranking, repositories, previews, _| {
            let result = NativeTextPathResultV1 {
                ranked: NativeTextPathRankingV1 { ranking, repositories },
                previews,
            };
            result.validate_previews().map_err(|error| IndexError::BackendViolation(error.into()))?;
            Ok((result, None))
        },
    )
}

/// A projected path page before its `CodeSearch` matches are attached, with
/// the scan's representative match of each row, in row order.
pub(super) struct ProjectedPathPage {
    pub(super) value: ResultValue<NativeTextPathPageV1>,
    pub(super) witnesses: Vec<Option<CodeSearchMatch>>,
    // The witness list dies before its grant.
    pub(super) _witness_memory: Option<MemoryLease>,
}

pub(super) fn convert_path_page(
    outcome: QueryOutcome<TextReadResult>,
    session: NativeReadSessionId,
    instance: IndexInstanceId,
    requested_top_k: u32,
    query: Digest,
    after: Option<&NativeTextPathCursorV1>,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<ProjectedPathPage, IndexError> {
    let rows = outcome.result().hits.len();
    let witness_bytes = rows
        .checked_mul(std::mem::size_of::<Option<CodeSearchMatch>>())
        .ok_or(IndexError::ResourceLimited)
        .and_then(|bytes| u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited))?;
    let witness_memory = if witness_bytes == 0 {
        None
    } else {
        // The list outlives the execution reservation: the excerpt step
        // consumes it after the projection returns.
        reserve(witness_bytes, MemoryLifetime::Retained)?
    };
    let mut witnesses = Vec::new();
    witnesses
        .try_reserve_exact(rows)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let value = convert_value_impl(
        outcome,
        instance,
        requested_top_k,
        false,
        ReadShape::PathPage,
        &reserve,
        Some(&mut witnesses),
        |ranking, repositories, _, remaining| {
            if after.is_none() && remaining != ranking.matched {
                return Err(IndexError::BackendViolation(
                    "initial path page omitted groups".into(),
                ));
            }
            if let Some(after) = after {
                if after.artifact != ranking.artifact {
                    return Err(IndexError::InvalidInput("path page artifact differs".into()));
                }
                if remaining == ranking.matched {
                    return Err(IndexError::BackendViolation(
                        "path page remaining count includes its anchor".into(),
                    ));
                }
                if ranking.hits.iter().zip(&repositories).any(|(hit, repo)| {
                        (repo == &after.repository && hit.path == after.path)
                            || !compare_native_text_path_rank_v1(
                                (after.score, &after.repository, &after.path),
                                (hit.score, repo, &hit.path),
                            ).is_lt()
                    })
                {
                    return Err(IndexError::BackendViolation("path page does not follow its anchor".into()));
                }
            }
            let (cursor, memory) = if remaining > u64::from(requested_top_k) {
                let (last, repository) = ranking.hits.last().zip(repositories.last())
                    .ok_or_else(|| IndexError::BackendViolation("empty path page has a continuation".into()))?;
                let bytes = last.path.len().checked_add(repository.as_str().len())
                    .and_then(|bytes| u64::try_from(bytes).ok())
                    .ok_or(IndexError::ResourceLimited)?;
                let memory = reserve(bytes, MemoryLifetime::Retained)?;
                (Some(NativeTextPathCursorV1 {
                    view: ranking.view, instance, query, artifact: ranking.artifact,
                    score: last.score, repository: repository.clone(), path: last.path.clone(),
                }), memory)
            } else {
                (None, None)
            };
            Ok((
                NativeTextPathPageV1 {
                    session,
                    ranked: NativeTextPathRankingV1 {
                        ranking,
                        repositories,
                    },
                    remaining,
                    after: cursor,
                    code_search: Vec::new(),
                },
                memory,
            ))
        },
    )?;
    Ok(ProjectedPathPage {
        value,
        witnesses,
        _witness_memory: witness_memory,
    })
}

fn convert_impl<R>(
    outcome: QueryOutcome<TextReadResult>,
    instance: IndexInstanceId,
    requested_top_k: u32,
    with_preview: bool,
    shape: ReadShape,
    reserve: &impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
    make_result: impl FnOnce(
        NativeTextResult,
        Vec<RepoId>,
        Option<Vec<NativeTextPreview>>,
        u64,
    ) -> Result<(R, Option<MemoryLease>), IndexError>,
) -> Result<ScopedNativeResult<R>, IndexError> {
    convert_value_impl(
        outcome,
        instance,
        requested_top_k,
        with_preview,
        shape,
        reserve,
        None,
        make_result,
    )
    .map(ScopedNativeResult::new)
}

/// `witnesses`, when given, receives each row's representative `CodeSearch`
/// match in row order; its capacity is reserved by the caller.
fn convert_value_impl<R>(
    outcome: QueryOutcome<TextReadResult>,
    instance: IndexInstanceId,
    requested_top_k: u32,
    with_preview: bool,
    shape: ReadShape,
    reserve: &impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
    mut witnesses: Option<&mut Vec<Option<CodeSearchMatch>>>,
    make_result: impl FnOnce(
        NativeTextResult,
        Vec<RepoId>,
        Option<Vec<NativeTextPreview>>,
        u64,
    ) -> Result<(R, Option<MemoryLease>), IndexError>,
) -> Result<ResultValue<R>, IndexError> {
    validate_read_shape(&outcome, requested_top_k, shape, reserve)?;
    let view = outcome.view();
    let mut result = outcome.into_result();
    let preview = result.take_previews();
    if preview.is_some() != with_preview {
        return Err(IndexError::BackendViolation(
            "native preview reader returned unexpected preview state".into(),
        ));
    }
    let count = result.hits.len();
    let remaining = result.remaining;
    let repository_count = if shape.groups_paths() { count } else { 0 };
    let preview_lease_count = preview.as_ref().map_or(0, |(_, leases)| leases.len());
    let lease_count = count
        .checked_add(preview_lease_count)
        .and_then(|count| count.checked_add(repository_count))
        .and_then(|count| count.checked_add(usize::from(matches!(shape, ReadShape::PathPage))))
        .ok_or(IndexError::ResourceLimited)?;
    let value_bytes = shared_value_bytes::<R>()?;
    let capacity_bytes = count
        .checked_mul(std::mem::size_of::<NativeTextHit>())
        .and_then(|bytes| {
            lease_count
                .checked_mul(std::mem::size_of::<Option<MemoryLease>>())
                .and_then(|leases| bytes.checked_add(leases))
        })
        .and_then(|bytes| repository_count.checked_mul(std::mem::size_of::<RepoId>())
            .and_then(|repo_bytes| bytes.checked_add(repo_bytes)))
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
    let mut payload_memory = Vec::new();
    payload_memory
        .try_reserve_exact(lease_count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if payload_memory.capacity() > lease_count {
        return Err(IndexError::ResourceLimited);
    }
    let mut repositories = Vec::new();
    repositories.try_reserve_exact(repository_count).map_err(|_| IndexError::ResourceLimited)?;
    if repositories.capacity() > repository_count {
        return Err(IndexError::ResourceLimited);
    }
    // The ranking is declared after its lease vector, so partial conversion
    // drops path data before path grants on every early return.
    let mut ranking = NativeTextResult {
        view,
        instance,
        coverage: NativeTextCoverage::CoreRequired,
        artifact: result.artifact,
        matched: result.matched,
        exhaustive: true,
        hits,
    };
    let parts = result.into_parts();
    let old_hit_memory = parts.3;
    let input_hits = parts.2;
    for mut hit in input_hits {
        if shape.groups_paths() {
            let (repository, memory) = hit.take_path_group_repository_v1().ok_or_else(|| {
                IndexError::BackendViolation("native path group lost its repository".into())
            })?;
            repositories.push(repository);
            payload_memory.push(memory);
        }
        if let Some(witnesses) = witnesses.as_deref_mut() {
            if witnesses.len() == witnesses.capacity() {
                return Err(IndexError::BackendViolation(
                    "path page witness list exceeds its reserved rows".into(),
                ));
            }
            witnesses.push(hit.code_search);
        }
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
        ranking.hits.push(NativeTextHit {
            owner,
            source,
            path,
            score,
        });
        payload_memory.push(memory);
    }
    // The consumed input iterator and its backing are gone before this refund.
    drop(old_hit_memory);
    let (previews, preview_leases) = match preview {
        Some((previews, leases)) => (Some(previews), leases),
        None => (None, Vec::new()),
    };
    let (value, cursor_memory) = make_result(ranking, repositories, previews, remaining)?;
    payload_memory.extend(preview_leases.into_iter().map(Some));
    if cursor_memory.is_some() {
        payload_memory.push(cursor_memory);
    }
    Ok(ResultValue {
        result: value,
        payload_memory,
        _capacity_memory: capacity,
    })
}
