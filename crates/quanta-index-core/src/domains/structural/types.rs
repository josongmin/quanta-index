//! Internal structural-match carriers.
//!
//! These DTOs belong to the core/searchd-side authority path. The public
//! query-response wire types live in `quanta-index-contract` and are filled
//! only at the search-plane response boundary.

use quanta_index_contract::LqFileScope;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum StructuralExecutableFilter {
    RepoRegexNoRev { pattern: String },
    FileRegex { pattern: String, scope: LqFileScope },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralMatchBinding {
    pub metavariable: String,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralMatchCandidate {
    pub candidate_id: String,
    pub pattern_start_byte: u32,
    pub pattern_end_byte: u32,
    pub bindings: Vec<StructuralMatchBinding>,
}
