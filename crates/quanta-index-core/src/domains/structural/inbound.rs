//! LXE-09 structural inbound types.
//!
//! In-process service-request shape passed by the search-plane to the
//! structural domain service. Uses the canonical AST block from
//! `quanta-index-lq-norm` (re-exported via `quanta-index-contract`) to avoid
//! forking a parallel pattern type.

use quanta_index_contract::{GenerationSelector, LqOptions, LqStructuralBlock};

use super::types::{StructuralExecutableFilter, StructuralMatchCandidate};

/// Domain-level structural query.
///
/// `pattern` is the normalized `match { ... }` AST produced by PRE-NORM. The
/// service does not re-parse text; if the caller has only a textual pattern,
/// they must lower it through the lq-norm pipeline first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralQueryRequest {
    pub pattern: LqStructuralBlock,
    pub requested_lang: Option<String>,
    pub filters: Vec<StructuralExecutableFilter>,
    pub candidate_scope: Option<Vec<String>>,
    pub options: LqOptions,
    pub generation: GenerationSelector,
}

/// Domain-level structural query response.
///
/// Carries the typed candidate rows returned by the producer-backed
/// structural executor.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct StructuralQueryResponse {
    pub candidates: Vec<StructuralMatchCandidate>,
}
