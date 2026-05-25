//! LXE-09 structural inbound types.
//!
//! In-process service-request shape passed by the search-plane to the
//! structural domain service. Uses the canonical AST block from
//! `quanta-index-lq-norm` (re-exported via `quanta-index-contract`) to avoid
//! forking a parallel pattern type.

use quanta_index_contract::{GenerationSelector, LqStructuralBlock, StructuralBinding};

/// Domain-level structural query.
///
/// `pattern` is the normalized `match { ... }` AST produced by PRE-NORM. The
/// service does not re-parse text; if the caller has only a textual pattern,
/// they must lower it through the lq-norm pipeline first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralQueryRequest {
    pub pattern: LqStructuralBlock,
    pub generation: GenerationSelector,
}

/// Domain-level structural query response.
///
/// Carries only the typed bindings; candidate composition (e.g. wrapping into
/// `StructuralCandidate` rows) is a search-plane concern.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct StructuralQueryResponse {
    pub bindings: Vec<StructuralBinding>,
}
