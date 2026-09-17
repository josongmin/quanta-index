//! Pinned structural candidate universe for bounded negation.

use quanta_index_contract::{ChunkRecord, GenerationPin, LqFileScope};
use quanta_index_core::domains::structural::StructuralExecutableFilter;
use quanta_index_core::{CoreError, StructuralMatchCandidate};
use quanta_index_lq_regex::RegexExecutor;

use crate::query_dispatcher::errors::structural_invalid_request;
use crate::query_dispatcher::routes::structural::buckets::{
    StructuralCandidateBuckets, normalize_structural_match_bucket,
};
use crate::readiness::StructuralAuthorityState;

fn compile_structural_filter_regex(
    filter_name: &str,
    pattern: &str,
) -> Result<RegexExecutor, CoreError> {
    RegexExecutor::compile(pattern).map_err(|err| {
        structural_invalid_request(format!(
            "{filter_name} filter pattern failed to compile as regex: {err}"
        ))
    })
}

fn repo_matches_structural_filters(
    pin: &GenerationPin,
    filters: &[StructuralExecutableFilter],
) -> Result<bool, CoreError> {
    for filter in filters {
        if let StructuralExecutableFilter::RepoRegexNoRev { pattern } = filter {
            let executor = compile_structural_filter_regex("repo", pattern)?;
            if !executor.verify(pin.repo_id.as_str().as_bytes()) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn chunk_matches_structural_filters(
    chunk: &ChunkRecord,
    filters: &[StructuralExecutableFilter],
) -> Result<bool, CoreError> {
    for filter in filters {
        match filter {
            StructuralExecutableFilter::RepoRegexNoRev { .. } => {}
            StructuralExecutableFilter::FileRegex { pattern, scope } => {
                let executor = compile_structural_filter_regex("file", pattern)?;
                let path = chunk.repo_relative_path.as_str();
                let path_match = executor.verify(path.as_bytes());
                let matched = match scope {
                    LqFileScope::PathOnly => path_match,
                    LqFileScope::NameOnly => path
                        .rsplit('/')
                        .next()
                        .is_some_and(|name| executor.verify(name.as_bytes())),
                    LqFileScope::NameAndPath => {
                        path_match
                            || path
                                .rsplit('/')
                                .next()
                                .is_some_and(|name| executor.verify(name.as_bytes()))
                    }
                };
                if !matched {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

pub(super) fn build_pinned_structural_universe(
    pin: &GenerationPin,
    structural_state: &StructuralAuthorityState,
    requested_lang: Option<&str>,
    filters: &[StructuralExecutableFilter],
) -> Result<StructuralCandidateBuckets, CoreError> {
    if !repo_matches_structural_filters(pin, filters)? {
        return Ok(StructuralCandidateBuckets::new());
    }
    let mut buckets = StructuralCandidateBuckets::new();
    for (chunk_id, chunk) in structural_state.chunks() {
        if let Some(lang) = requested_lang
            && chunk.language.as_str() != lang
        {
            continue;
        }
        if !chunk_matches_structural_filters(chunk, filters)? {
            continue;
        }
        let candidate_id = chunk_id.as_str().to_string();
        buckets
            .entry(candidate_id.clone())
            .or_default()
            .push(StructuralMatchCandidate {
                candidate_id,
                pattern_start_byte: chunk.start_byte,
                pattern_end_byte: chunk.end_byte,
                bindings: Vec::new(),
            });
    }
    for bucket in buckets.values_mut() {
        normalize_structural_match_bucket(bucket);
    }
    Ok(buckets)
}
