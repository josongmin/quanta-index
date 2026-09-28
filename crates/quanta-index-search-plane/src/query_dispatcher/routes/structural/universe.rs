//! Pinned structural candidate universe for bounded negation.

use quanta_index_contract::{ChunkRecord, GenerationPin, LqFileScope, SearchPlaneErrorCodeV2};
use quanta_index_core::domains::structural::StructuralExecutableFilter;
use quanta_index_core::{CoreError, StructuralMatchCandidate};
use quanta_index_lq_regex::{RegexErrorCode, RegexExecutor};

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
        let detail = format!("{filter_name} filter pattern failed to compile as regex: {err}");
        match err.code {
            RegexErrorCode::ParseFail | RegexErrorCode::ForbiddenSyntax => {
                structural_invalid_request(detail)
            }
            RegexErrorCode::PlanLimitExceeded => CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
                message: format!("structural: {detail}"),
            },
            RegexErrorCode::RegexPrefilterUnusable => CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexRegexRegexPrefilterUnusable,
                message: format!("structural: {detail}"),
            },
            RegexErrorCode::QueryTimeout => CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexRegexQueryTimeout,
                message: format!("structural: {detail}"),
            },
            RegexErrorCode::Interrupted => CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexRegexInterrupted,
                message: format!("structural: {detail}"),
            },
            RegexErrorCode::ExecutionInternal => CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexRegexExecutionInternal,
                message: format!("structural: {detail}"),
            },
        }
    })
}

struct CompiledFileFilter {
    scope: LqFileScope,
    executor: RegexExecutor,
}

fn compile_file_filters(
    filters: &[StructuralExecutableFilter],
) -> Result<Vec<CompiledFileFilter>, CoreError> {
    filters
        .iter()
        .filter_map(|filter| match filter {
            StructuralExecutableFilter::FileRegex { pattern, scope } => Some((pattern, scope)),
            StructuralExecutableFilter::RepoRegexNoRev { .. } => None,
        })
        .map(|(pattern, scope)| {
            Ok(CompiledFileFilter {
                scope: *scope,
                executor: compile_structural_filter_regex("file", pattern)?,
            })
        })
        .collect()
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

fn chunk_matches_structural_filters(chunk: &ChunkRecord, filters: &[CompiledFileFilter]) -> bool {
    for filter in filters {
        let path = chunk.repo_relative_path.as_str();
        let path_match = filter.executor.verify(path.as_bytes());
        let matched = match filter.scope {
            LqFileScope::PathOnly => path_match,
            LqFileScope::NameOnly => path
                .rsplit('/')
                .next()
                .is_some_and(|name| filter.executor.verify(name.as_bytes())),
            LqFileScope::NameAndPath => {
                path_match
                    || path
                        .rsplit('/')
                        .next()
                        .is_some_and(|name| filter.executor.verify(name.as_bytes()))
            }
        };
        if !matched {
            return false;
        }
    }
    true
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
    // Compile each file filter once for this universe; every eligible chunk
    // verifies against the same executor and its byte-match semantics.
    let file_filters = compile_file_filters(filters)?;
    let mut buckets = StructuralCandidateBuckets::new();
    for (chunk_id, chunk) in structural_state.chunks() {
        if let Some(lang) = requested_lang
            && chunk.language.as_str() != lang
        {
            continue;
        }
        if !chunk_matches_structural_filters(chunk, &file_filters) {
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

#[cfg(test)]
mod tests {
    use quanta_index_contract::SearchPlaneErrorCodeV2;
    use quanta_index_core::CoreError;

    use super::compile_structural_filter_regex;

    #[test]
    fn structural_filter_regex_preserves_resource_vs_syntax_errors() {
        let huge = r"[\x{80}-\x{10FFFF}]{20000}";
        for name in ["repo", "file"] {
            let resource = compile_structural_filter_regex(name, huge);
            assert!(matches!(
                resource,
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
                    ..
                })
            ));
            let syntax = compile_structural_filter_regex(name, "[");
            assert!(matches!(
                syntax,
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::StrInvalidRequest,
                    ..
                })
            ));
        }
    }
}
