//! SDK validation for the repository/path projection of Native text reads.

use super::*;
use quanta_index_contract::native_protocol::NativeTextPathRankingV1;
use quanta_index_contract::{LqFilter, LqSelect};

pub(super) fn query_mode(query: &TextQueryRequest) -> Result<(u32, bool), NativeUdsError> {
    let mut path = false;
    let limit = native_text_result_limit_checked(query, |parsed| {
        path = parsed.filters.iter().any(|filter| {
            matches!(
                filter,
                LqFilter::Select {
                    dim: LqSelect::Path
                }
            )
        });
        Ok(())
    })?;
    Ok((limit, path))
}

pub(super) fn validate_ranking(
    selection: ViewSelection,
    instance: IndexInstanceId,
    limit: u32,
    expected_count: u64,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    ranked: &NativeTextPathRankingV1,
    memory: Option<&MemoryScope>,
) -> Result<(), NativeUdsError> {
    let result = &ranked.ranking;
    let count = u64::try_from(result.hits.len())
        .map_err(|error| NativeUdsError::Protocol(error.to_string()))?;
    if !selection_matches(selection, result.view)
        || result.instance != instance
        || !result.exhaustive
        || ranked.repositories.len() != result.hits.len()
        || count != expected_count
        || count > u64::from(limit)
        || result.matched < count
    {
        return Err(NativeUdsError::Protocol(
            "text path identity or cardinality differs".into(),
        ));
    }
    let _scratch = temporary_rows::<usize>(memory, result.hits.len())?;
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(result.hits.len())
        .map_err(|error| NativeUdsError::Transport(error.to_string()))?;
    for (index, (hit, repository)) in result.hits.iter().zip(&ranked.repositories).enumerate() {
        if !valid_native_source_path(&hit.path)
            || constraints
                .repo_relative_path_exact
                .as_ref()
                .is_some_and(|expected| expected.as_str() != hit.path)
            || hit.score.get() < 0.0
            || repository.as_str().is_empty()
        {
            return Err(NativeUdsError::Protocol("text path row invalid".into()));
        }
        positions.push(index);
    }
    for index in 1..result.hits.len() {
        let prior = &result.hits[index - 1];
        let next = &result.hits[index];
        if quanta_index_contract::native_protocol::compare_native_text_path_rank_v1(
            (prior.score, &ranked.repositories[index - 1], &prior.path),
            (next.score, &ranked.repositories[index], &next.path),
        ) != std::cmp::Ordering::Less
        {
            return Err(NativeUdsError::Protocol(
                "text path rank order differs".into(),
            ));
        }
    }
    positions.sort_unstable_by(|left, right| {
        ranked.repositories[*left]
            .as_str()
            .cmp(ranked.repositories[*right].as_str())
            .then_with(|| result.hits[*left].path.cmp(&result.hits[*right].path))
    });
    if positions.windows(2).any(|pair| {
        ranked.repositories[pair[0]] == ranked.repositories[pair[1]]
            && result.hits[pair[0]].path == result.hits[pair[1]].path
    }) {
        return Err(NativeUdsError::Protocol("duplicate text path group".into()));
    }
    positions.sort_unstable_by_key(|index| result.hits[*index].owner);
    if positions
        .windows(2)
        .any(|pair| result.hits[pair[0]].owner == result.hits[pair[1]].owner)
    {
        return Err(NativeUdsError::Protocol("duplicate text path owner".into()));
    }
    Ok(())
}
