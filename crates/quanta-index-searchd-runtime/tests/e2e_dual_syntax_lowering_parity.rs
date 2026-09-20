//! E2E-02 — Dual-syntax lowering parity.
//!
//! Two syntaxes (Sourcegraph + Native) must lower to identical lexical IR
//! and produce identical query results when executed against the same
//! generation. There is no external Sourcegraph reference: this matrix
//! exercises self-parity between the two surface syntaxes that both feed
//! into this daemon's single active lowering + execution path per route.
//!
//! SG structural mixed-domain parity covers the subset frozen by
//! `quanta-index-search-plane::lowering::structural_leaf_verdict`: `Keyword`
//! and (ADV-02) `RawString` and `Predicate` leaves are preserved as lexical
//! siblings of a structural body; `Phrase`/`Regex` bodies become structural
//! blocks. A preserved `Predicate` is gated by the lexical executor exactly as
//! on native (executable predicates run; non-executable ones typed-fail at
//! execution). Only `StructuralBlock` remains typed-fail on the route.
//!
//! Each row pairs a Sourcegraph-syntax query and an equivalent native LQ
//! query. The runner issues both against the same sealed corpus and
//! asserts:
//!
//! 1. Both syntaxes return the **same** observed shape (same corpus ids in
//!    the same order, OR the same typed error code).
//! 2. The shape matches the row's `ExpectedOutcome` against the *current*
//!    behavior:
//!    - `Candidates` rows assert the exact corpus-id set.
//!    - `ExpectedFailing` rows stay GREEN while their
//!      `current_observation` continues to hold; when the owner ticket
//!      lands and behavior diverges, the row flips RED and must be
//!      promoted. The current live matrix happens to have no such rows,
//!      but the closed-loop variant stays in place for regressions.
//!
//! Any divergence between the two syntaxes for a given row is a parity
//! violation regardless of the wiring state of the underlying feature.

#![forbid(unsafe_code)]

use anyhow::{Result as AnyResult, ensure};
use quanta_index_contract::{
    FileContributorEntry, FileContributorIdentityEntry, FileContributorIngestBatch,
    FileOwnershipEntry, FileOwnershipIngestBatch, RepoCommitRecencyEntry,
    RepoCommitRecencyIngestBatch, RepoId, RepoMetaEntry, RepoMetaIngestBatch, RepoRelativePath,
    RepoTopicEntry, RepoTopicIngestBatch, TextQuerySyntax,
};
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::e2e_harness;
use crate::e2e_harness::{
    E2eHistoryFixtureSpec, E2eHistoryResult, E2eQueryResult, E2eRuntime, E2eRuntimeCatalogSpec,
    E2eRuntimeChangedSpec, E2eRuntimeEdgeSpec, E2eRuntimeFacetSpec, E2eRuntimeSnapshotSpec,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueryRoute {
    Text,
    Structural,
    HistoryCommits,
    HistoryDiffPaths,
    RuntimeMetadata,
}

/// Per-row expected outcome.
///
/// Variants are added only as live rows exist for them (CLAUDE.md "dead
/// port surface is forbidden"). When a parity row graduates to fully-green
/// candidates or a row currently fails with a typed error, add the
/// corresponding variant in the same PR.
#[derive(Clone, Debug)]
enum ExpectedOutcome {
    Candidates {
        ids: &'static [&'static str],
    },
    HistoryCommits {
        shas: &'static [&'static str],
    },
    HistoryDiffPaths {
        paths: &'static [&'static str],
    },
    TypedError {
        code: &'static str,
    },
    /// Closed-loop regression variant. If a future row is intentionally
    /// behind implementation, both syntaxes must exhibit
    /// `current_observation` until the owner ticket lands.
    #[expect(
        dead_code,
        reason = "all current dual-syntax rows are green or typed-error; keep closed-loop variant for future regressions"
    )]
    ExpectedFailing {
        owner_ticket: &'static str,
        reason: &'static str,
        current_observation: CurrentObservation,
    },
}

#[expect(
    dead_code,
    reason = "all current dual-syntax rows are green or typed-error; retained for future expected-failing rows"
)]
#[derive(Clone, Debug)]
enum CurrentObservation {
    Empty,
}

struct ParityScenario {
    route: QueryRoute,
    id: &'static str,
    sg_query: &'static str,
    lq_query: &'static str,
    top_k: u32,
    expected: ExpectedOutcome,
}

struct CorpusRow {
    id: &'static str,
    path: &'static str,
    content: &'static str,
    symbol_name: Option<&'static str>,
    structural_identifier: Option<&'static str>,
}

const CORPUS: &[CorpusRow] = &[
    CorpusRow {
        id: "alpha_rust",
        path: "src/lib.rs",
        content: "fn parity_needle_alpha() {}",
        symbol_name: None,
        structural_identifier: Some("parity_needle_alpha"),
    },
    CorpusRow {
        id: "beta_py",
        path: "scripts/helper.py",
        content: "def parity_needle_alpha(): pass",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "gamma_md",
        path: "docs/intro.md",
        content: "parity documentation lives here 1 123",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "delta_other_path",
        path: "src/other.rs",
        content: "fn parity_needle_alpha() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "delta_phrase",
        path: "src/phrase.rs",
        content: "lemon yellow banana",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "epsilon_regex",
        path: "src/version.rs",
        content: "const VERSION: &str = \"v9.8.7\";",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "zeta_raw",
        path: "src/raw.rs",
        content: "let parity_foo_bar_baz = 1;",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "theta_symbol",
        path: "src/sym.rs",
        content: "pub fn ParityTypeSymbol(arg: i32) -> i32 { arg }",
        symbol_name: Some("ParityTypeSymbol"),
        structural_identifier: None,
    },
];

const SCENARIOS: &[ParityScenario] = &[
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_filter_parity",
        sg_query: "repo:repo-other parity_needle_alpha",
        lq_query: "repo:repo-other parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_filter_parity",
        sg_query: "file:src/lib.rs parity_needle_alpha",
        lq_query: "file:src/lib.rs parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "lang_filter_parity",
        sg_query: "lang:rust parity_needle_alpha",
        lq_query: "lang:rust parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "case_filter_parity",
        sg_query: "case:yes PARITY_NEEDLE_ALPHA",
        lq_query: "case:yes PARITY_NEEDLE_ALPHA",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "count_parity",
        sg_query: "count:1 parity_needle_alpha",
        lq_query: "count:1 parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "fork_typed_unavailable_parity",
        sg_query: "fork:no parity_needle_alpha",
        lq_query: "fork:no parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::TypedError {
            code: "LEX_FILTER_FORK_UNAVAILABLE",
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "visibility_typed_unavailable_parity",
        sg_query: "visibility:public parity_needle_alpha",
        lq_query: "visibility:public parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::TypedError {
            code: "LEX_FILTER_VISIBILITY_UNAVAILABLE",
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "type_file_parity",
        sg_query: "type:file parity_needle_alpha",
        lq_query: "type:file parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "type_symbol_parity",
        sg_query: "type:symbol ParityTypeSymbol",
        lq_query: "type:symbol ParityTypeSymbol",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["theta_symbol"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "symbol_has_name_predicate_parity",
        sg_query: "symbol:has.name(ParityTypeSymbol)",
        lq_query: "symbol.has.name(ParityTypeSymbol)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["theta_symbol"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "select_file_parity",
        sg_query: "select:file parity_needle_alpha",
        lq_query: "select:file parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "select_content_parity",
        sg_query: "select:content parity_needle_alpha",
        lq_query: "select:content parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "select_symbol_parity",
        sg_query: "select:symbol ParityTypeSymbol",
        lq_query: "select:symbol ParityTypeSymbol",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["theta_symbol"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_raw_substring_parity",
        // The shared executable raw-substring surface today is
        // `file:contains('...')` on the SG side and a single-quoted raw
        // string leaf on the native side.
        sg_query: "file:contains('oo_ba')",
        lq_query: "'oo_ba'",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &["zeta_raw"] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_phrase_parity",
        sg_query: "file:contains(\"lemon yellow banana\")",
        lq_query: "file.contains(\"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_content_regex_parity",
        sg_query: "file:has.content(/v\\d+\\.\\d+\\.\\d+/)",
        lq_query: "file:has.content(/v\\d+\\.\\d+\\.\\d+/)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["epsilon_regex"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_content_alias_parity",
        sg_query: "file:contains.content(\"lemon yellow banana\")",
        lq_query: "file.contains(\"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_content_native_alias_parity",
        sg_query: "file:contains.content(\"lemon yellow banana\")",
        lq_query: "file.contains.content(\"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_number_parity",
        sg_query: "file:contains(123)",
        lq_query: "file.contains(123)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &["gamma_md"] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_content_number_parity",
        sg_query: "file:has.content(123)",
        lq_query: "file.has.content(123)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &["gamma_md"] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_content_number_miss_parity",
        sg_query: "file:has.content(404)",
        lq_query: "file.has.content(404)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_scoped_path_parity",
        sg_query: "file:contains(path:src/phrase.rs, \"lemon yellow banana\")",
        lq_query: "file.contains(path:src/phrase.rs, \"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_content_scoped_path_parity",
        sg_query: "file:has.content(path:src/phrase.rs, \"lemon yellow banana\")",
        lq_query: "file.has.content(path:src/phrase.rs, \"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_content_scoped_path_miss_parity",
        sg_query: "file:has.content(path:src/lib.rs, \"lemon yellow banana\")",
        lq_query: "file.has.content(path:src/lib.rs, \"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_scoped_file_parity",
        sg_query: "file:contains(file:phrase.rs, \"lemon yellow banana\")",
        lq_query: "file.contains(file:phrase.rs, \"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_content_scoped_file_parity",
        sg_query: "file:has.content(file:phrase.rs, \"lemon yellow banana\")",
        lq_query: "file.has.content(file:phrase.rs, \"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_scoped_file_miss_parity",
        sg_query: "file:contains(file:missing.rs, \"lemon yellow banana\")",
        lq_query: "file.contains(file:missing.rs, \"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_content_scoped_file_miss_parity",
        sg_query: "file:has.content(file:missing.rs, \"lemon yellow banana\")",
        lq_query: "file.has.content(file:missing.rs, \"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_content_scoped_lang_regex_parity",
        sg_query: "file:has.content(lang:rust, /v\\d+\\.\\d+\\.\\d+/)",
        lq_query: "file.has.content(lang:rust, /v\\d+\\.\\d+\\.\\d+/)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["epsilon_regex"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_content_scoped_lang_regex_miss_parity",
        sg_query: "file:has.content(lang:markdown, /v\\d+\\.\\d+\\.\\d+/)",
        lq_query: "file.has.content(lang:markdown, /v\\d+\\.\\d+\\.\\d+/)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_scoped_and_parity",
        sg_query: "file:contains(path:src/phrase.rs, \"lemon yellow banana\") AND banana",
        lq_query: "file.contains(path:src/phrase.rs, \"lemon yellow banana\") AND banana",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_scoped_and_miss_parity",
        sg_query: "file:contains(path:src/lib.rs, \"lemon yellow banana\") AND banana",
        lq_query: "file.contains(path:src/lib.rs, \"lemon yellow banana\") AND banana",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_scoped_or_single_hit_parity",
        sg_query: "file:contains(path:src/phrase.rs, \"lemon yellow banana\") OR missing_corpus_token",
        lq_query: "file.contains(path:src/phrase.rs, \"lemon yellow banana\") OR missing_corpus_token",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_scoped_not_empty_parity",
        sg_query: "banana NOT file:contains(path:src/phrase.rs, \"lemon yellow banana\")",
        lq_query: "banana NOT file.contains(path:src/phrase.rs, \"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_scoped_name_hit_parity",
        sg_query: "file:contains(name:phrase.rs, \"lemon yellow banana\")",
        lq_query: "file.contains(name:phrase.rs, \"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_content_scoped_or_single_hit_parity",
        sg_query: "file:has.content(path:src/phrase.rs, \"lemon yellow banana\") OR missing_corpus_token",
        lq_query: "file.has.content(path:src/phrase.rs, \"lemon yellow banana\") OR missing_corpus_token",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_content_scoped_not_empty_parity",
        sg_query: "banana NOT file:has.content(path:src/phrase.rs, \"lemon yellow banana\")",
        lq_query: "banana NOT file.has.content(path:src/phrase.rs, \"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_content_scoped_name_hit_parity",
        sg_query: "file:has.content(name:phrase.rs, \"lemon yellow banana\")",
        lq_query: "file.has.content(name:phrase.rs, \"lemon yellow banana\")",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_multiple_scalars_typed_fail_parity",
        sg_query: "file:contains(\"lemon\", \"banana\")",
        lq_query: "file.contains(\"lemon\", \"banana\")",
        top_k: 10,
        expected: ExpectedOutcome::TypedError {
            code: "LEX_PREDICATE_UNIMPLEMENTED",
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_content_scoped_typed_fail_parity",
        sg_query: "repo:has.content(path:src, 7)",
        lq_query: "repo.has.content(path:src, 7)",
        top_k: 10,
        expected: ExpectedOutcome::TypedError {
            code: "LEX_PREDICATE_UNIMPLEMENTED",
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "patterntype_regexp_parity",
        sg_query: "patterntype:regexp v\\d+\\.\\d+\\.\\d+",
        lq_query: "/v\\d+\\.\\d+\\.\\d+/",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["epsilon_regex"],
        },
    },
    // QI-BB-011: `parity_needle_alpha` is one term of the shared normalizer,
    // not the three-term phrase the old analyzer scored with three summed
    // IDFs, so under BM25 the rarer `documentation` (one document) now ranks
    // first; the identifier's documents follow, shortest first, then by path.
    ParityScenario {
        route: QueryRoute::Text,
        id: "boolean_or_parity",
        sg_query: "parity_needle_alpha OR documentation",
        lq_query: "parity_needle_alpha OR documentation",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["gamma_md", "alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "negation_parity",
        sg_query: "parity_needle_alpha NOT helper",
        lq_query: "parity_needle_alpha NOT helper",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_predicate_parity",
        // Dual-syntax parity only compares live equivalent surfaces.
        // The SG-only `repohasfile:` alias remains covered by bridge
        // parser/translator tests; the active equivalent query here is the
        // shared `repo:has.file(...)` predicate surface.
        sg_query: "repo:has.file(path:src/lib.rs) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/lib.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_predicate_under_or_parity",
        sg_query: "repo:has.file(path:src/lib.rs) OR documentation",
        lq_query: "repo:has.file(path:src/lib.rs) OR documentation",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &[
                "gamma_md",
                "beta_py",
                "alpha_rust",
                "delta_other_path",
                "delta_phrase",
                "zeta_raw",
                "theta_symbol",
                "epsilon_regex",
            ],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_predicate_under_not_parity",
        sg_query: "parity_needle_alpha NOT repo:has.file(path:src/missing.rs)",
        lq_query: "parity_needle_alpha NOT repo:has.file(path:src/missing.rs)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_name_predicate_parity",
        sg_query: "repo:has.file(name:lib.rs) parity_needle_alpha",
        lq_query: "repo:has.file(name:lib.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_scalar_path_predicate_parity",
        sg_query: "repo:has.file(src/lib.rs) parity_needle_alpha",
        lq_query: "repo:has.file(src/lib.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_path_alias_parity",
        sg_query: "repo:has.path(src/lib.rs) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/lib.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_path_native_alias_parity",
        sg_query: "repo:has.path(src/lib.rs) parity_needle_alpha",
        lq_query: "repo.has.path(src/lib.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    // SGT-01: `repo:contains.path(...)` is Sourcegraph's alias of
    // `repo:has.path(...)`, canonicalizing onto `repo:has.file(path:...)`.
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_contains_path_alias_parity",
        sg_query: "repo:contains.path(src/lib.rs) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/lib.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_contains_path_native_alias_parity",
        sg_query: "repo:contains.path(src/lib.rs) parity_needle_alpha",
        lq_query: "repo.contains.path(src/lib.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    // SGT-01: `repo:contains.file(...)` is Sourcegraph's alias of
    // `repo:has.file(...)`; it forwards the full matcher surface unchanged.
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_contains_file_alias_parity",
        sg_query: "repo:contains.file(path:src/lib.rs) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/lib.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_contains_file_native_alias_parity",
        sg_query: "repo:contains.file(path:src/lib.rs) parity_needle_alpha",
        lq_query: "repo.contains.file(path:src/lib.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    // SGT-01 (gate must be load-bearing): a MISSING path closes the gate to the
    // empty set. A no-op alias that silently dropped the predicate would instead
    // return the bare `parity_needle_alpha` hits, so these rows prove the alias
    // actually gates rather than passing trivially on an always-open path.
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_contains_path_alias_missing_path_gates_empty_parity",
        sg_query: "repo:contains.path(src/does-not-exist.rs) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/does-not-exist.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_contains_file_alias_missing_path_gates_empty_parity",
        sg_query: "repo:contains.file(path:src/does-not-exist.rs) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/does-not-exist.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    // ADV-01 widened arg-shape family: `repo.has.file(lang:<x>)`. The corpus
    // repo contains `beta_py` (scripts/helper.py, python), so the `lang:python`
    // gate opens identically on both syntaxes and returns the parity set.
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_lang_predicate_parity",
        sg_query: "repo:has.file(lang:python) parity_needle_alpha",
        lq_query: "repo:has.file(lang:python) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    // SGX-01: `repo:has.file(path:... content:...)` correlates per-document.
    // src/lib.rs (alpha_rust) content is `fn parity_needle_alpha() {}`, so the
    // file at that path DOES contain the token → gate opens.
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_path_content_predicate_parity",
        sg_query: "repo:has.file(path:src/lib.rs, content:parity_needle_alpha) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/lib.rs, content:parity_needle_alpha) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    // Load-bearing: the file at src/lib.rs does NOT contain `absent_zzz_token`, so
    // the content clause closes the gate to empty. A no-op content (path-only
    // overmatch) would instead return the three needle hits.
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_path_content_miss_parity",
        sg_query: "repo:has.file(path:src/lib.rs, content:absent_zzz_token) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/lib.rs, content:absent_zzz_token) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_path_name_predicate_parity",
        sg_query: "repo:has.file(path:src/lib.rs, name:lib.rs) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/lib.rs, name:lib.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_path_name_miss_parity",
        sg_query: "repo:has.file(path:src/lib.rs, name:helper.py) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/lib.rs, name:helper.py) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_path_lang_predicate_parity",
        sg_query: "repo:has.file(path:scripts/helper.py, lang:python) parity_needle_alpha",
        lq_query: "repo:has.file(path:scripts/helper.py, lang:python) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_path_lang_miss_parity",
        sg_query: "repo:has.file(path:src/lib.rs, lang:python) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/lib.rs, lang:python) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_name_lang_predicate_parity",
        sg_query: "repo:has.file(name:helper.py, lang:python) parity_needle_alpha",
        lq_query: "repo:has.file(name:helper.py, lang:python) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_name_lang_miss_parity",
        sg_query: "repo:has.file(name:helper.py, lang:rust) parity_needle_alpha",
        lq_query: "repo:has.file(name:helper.py, lang:rust) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_path_name_lang_predicate_parity",
        sg_query: "repo:has.file(path:src/lib.rs, name:lib.rs, lang:rust) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/lib.rs, name:lib.rs, lang:rust) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_path_name_lang_miss_parity",
        sg_query: "repo:has.file(path:src/lib.rs, name:lib.rs, lang:python) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/lib.rs, name:lib.rs, lang:python) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_content_predicate_parity",
        sg_query: "repo:has.content(parity_needle_alpha) parity_needle_alpha",
        lq_query: "repo:has.content(parity_needle_alpha) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_content_phrase_predicate_parity",
        sg_query: "repo:has.content(\"lemon yellow banana\") parity_needle_alpha",
        lq_query: "repo:has.content(\"lemon yellow banana\") parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_content_raw_predicate_parity",
        sg_query: "repo:has.content('parity_foo_bar_baz') parity_needle_alpha",
        lq_query: "repo:has.content('parity_foo_bar_baz') parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_contains_content_native_alias_parity",
        sg_query: "repo:contains.content(parity_needle_alpha) parity_needle_alpha",
        lq_query: "repo.contains.content(parity_needle_alpha) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_contains_content_alias_or_parity",
        sg_query: "repo:contains.content(missing_corpus_token) OR parity_needle_alpha",
        lq_query: "repo.contains.content(missing_corpus_token) OR parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_contains_content_alias_not_parity",
        sg_query: "parity_needle_alpha NOT repo:contains.content(parity_needle_alpha)",
        lq_query: "parity_needle_alpha NOT repo.contains.content(parity_needle_alpha)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_content_number_parity",
        sg_query: "repo:has.content(123) parity_needle_alpha",
        lq_query: "repo.has.content(123) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path", "beta_py"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "select_repo_projection_parity",
        sg_query: "select:repo parity_needle_alpha",
        lq_query: "select:repo parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_happy_path_parity",
        sg_query: r#"repo:repo-e2e path:src/lib.rs lang:rust patterntype:structural "function_item { { identifier :[name] } }""#,
        lq_query: "repo:repo-e2e file:src/lib.rs lang:rust match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_boolean_or_parity",
        sg_query: r#"patterntype:structural "function_item { { identifier :[name] } }" OR patterntype:structural "trait_item""#,
        lq_query: "match { function_item { { identifier :[name] } } } OR match { trait_item }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_boolean_not_parity",
        sg_query: r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT "trait_item""#,
        lq_query: "match { function_item { { identifier :[name] } } } AND NOT match { trait_item }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_typed_expr_hole_parity",
        sg_query: r#"patterntype:structural "function_item { { :[name.expr] } }""#,
        lq_query: "match { function_item { { :[name.expr] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_regex_body_parity",
        sg_query: r"patterntype:structural /^parity_needle_alpha$/",
        lq_query: "match { :[name] where :[name] == /^parity_needle_alpha$/ }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_select_rejection_parity",
        sg_query: r#"select:repo patterntype:structural "function_item""#,
        lq_query: "select:repo match { function_item }",
        top_k: 10,
        expected: ExpectedOutcome::TypedError {
            code: "STR_INVALID_REQUEST",
        },
    },
    // SG structural route: keyword leaf + structural body only (see lowering.rs).
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_lexical_and_parity",
        sg_query: r#"repo:repo-e2e path:src/lib.rs lang:rust patterntype:structural parity_needle_alpha AND "function_item { { identifier :[name] } }""#,
        lq_query: "repo:repo-e2e file:src/lib.rs lang:rust parity_needle_alpha AND match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    // ADV-02 widening: Predicate sibling preserved on the SG structural route.
    // `repo:has.file(path:src/lib.rs)` lowers to a preserved Predicate leaf that
    // gates the repo open, AND'd with a structural body — mirroring native
    // `repo:has.file(...) AND match { ... }`. The lexical executor runs the
    // predicate identically on both syntaxes; intersection is `alpha_rust`.
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_sibling_and_parity",
        sg_query: r#"patterntype:structural repo:has.file(path:src/lib.rs) AND "function_item { { identifier :[name] } }""#,
        lq_query: "repo:has.file(path:src/lib.rs) AND match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_scalar_path_sibling_and_parity",
        sg_query: r#"patterntype:structural repo:has.file(src/lib.rs) AND "function_item { { identifier :[name] } }""#,
        lq_query: "repo:has.file(src/lib.rs) AND match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_path_or_parity",
        sg_query: r#"patterntype:structural repo:has.file(path:src/lib.rs) OR "trait_item""#,
        lq_query: "repo:has.file(path:src/lib.rs) OR match { trait_item }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &[
                "alpha_rust",
                "delta_phrase",
                "epsilon_regex",
                "zeta_raw",
                "theta_symbol",
                "beta_py",
                "gamma_md",
                "delta_other_path",
            ],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_path_alias_and_parity",
        sg_query: r#"patterntype:structural repo:has.path(src/lib.rs) AND "function_item { { identifier :[name] } }""#,
        lq_query: "repo.has.path(src/lib.rs) AND match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_path_alias_or_parity",
        sg_query: r#"patterntype:structural repo:has.path(src/lib.rs) OR "trait_item""#,
        lq_query: "repo.has.path(src/lib.rs) OR match { trait_item }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &[
                "alpha_rust",
                "delta_phrase",
                "epsilon_regex",
                "zeta_raw",
                "theta_symbol",
                "beta_py",
                "gamma_md",
                "delta_other_path",
            ],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_path_alias_and_not_parity",
        sg_query: r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT repo:has.path(src/lib.rs)"#,
        lq_query: "match { function_item { { identifier :[name] } } } AND NOT repo.has.path(src/lib.rs)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_has_content_and_parity",
        sg_query: r#"patterntype:structural repo:has.content(parity_needle_alpha) AND "function_item { { identifier :[name] } }""#,
        lq_query: "repo:has.content(parity_needle_alpha) AND match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_has_content_or_parity",
        sg_query: r#"patterntype:structural repo:has.content(parity_needle_alpha) OR "trait_item""#,
        lq_query: "repo:has.content(parity_needle_alpha) OR match { trait_item }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &[
                "alpha_rust",
                "delta_phrase",
                "epsilon_regex",
                "zeta_raw",
                "theta_symbol",
                "beta_py",
                "gamma_md",
                "delta_other_path",
            ],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_has_content_and_not_parity",
        sg_query: r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT repo:has.content(parity_needle_alpha)"#,
        lq_query: "match { function_item { { identifier :[name] } } } AND NOT repo:has.content(parity_needle_alpha)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_contains_content_and_parity",
        sg_query: r#"patterntype:structural repo:contains.content(parity_needle_alpha) AND "function_item { { identifier :[name] } }""#,
        lq_query: "repo.contains.content(parity_needle_alpha) AND match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_contains_content_or_parity",
        sg_query: r#"patterntype:structural repo:contains.content(parity_needle_alpha) OR "trait_item""#,
        lq_query: "repo.contains.content(parity_needle_alpha) OR match { trait_item }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &[
                "alpha_rust",
                "delta_phrase",
                "epsilon_regex",
                "zeta_raw",
                "theta_symbol",
                "beta_py",
                "gamma_md",
                "delta_other_path",
            ],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_contains_content_and_not_parity",
        sg_query: r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT repo:contains.content(parity_needle_alpha)"#,
        lq_query: "match { function_item { { identifier :[name] } } } AND NOT repo.contains.content(parity_needle_alpha)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_file_contains_path_and_parity",
        sg_query: r#"patterntype:structural file:contains(path:src/lib.rs, parity_needle_alpha) AND "function_item { { identifier :[name] } }""#,
        lq_query: "file.contains(path:src/lib.rs, parity_needle_alpha) AND match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_file_contains_path_or_parity",
        sg_query: r#"patterntype:structural file:contains(path:src/lib.rs, parity_needle_alpha) OR "function_item { { identifier :[name] } }""#,
        lq_query: "file.contains(path:src/lib.rs, parity_needle_alpha) OR match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_file_contains_path_and_not_parity",
        sg_query: r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT file:contains(path:src/lib.rs, parity_needle_alpha)"#,
        lq_query: "match { function_item { { identifier :[name] } } } AND NOT file.contains(path:src/lib.rs, parity_needle_alpha)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_file_has_content_path_and_parity",
        sg_query: r#"patterntype:structural file:has.content(path:src/lib.rs, parity_needle_alpha) AND "function_item { { identifier :[name] } }""#,
        lq_query: "file.has.content(path:src/lib.rs, parity_needle_alpha) AND match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_file_has_content_path_or_parity",
        sg_query: r#"patterntype:structural file:has.content(path:src/lib.rs, parity_needle_alpha) OR "function_item { { identifier :[name] } }""#,
        lq_query: "file.has.content(path:src/lib.rs, parity_needle_alpha) OR match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_file_has_content_path_and_not_parity",
        sg_query: r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT file:has.content(path:src/lib.rs, parity_needle_alpha)"#,
        lq_query: "match { function_item { { identifier :[name] } } } AND NOT file.has.content(path:src/lib.rs, parity_needle_alpha)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    // Repo-scoped filters under mixed OR are bridge fail-closed; see lowering scoped-filter test.
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_lexical_or_parity",
        sg_query: r#"patterntype:structural parity_needle_alpha OR "function_item { { identifier :[name] } }""#,
        lq_query: "parity_needle_alpha OR match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "beta_py", "delta_other_path"],
        },
    },
    // ADV-02 widening: RawString sibling preserved on the SG structural route.
    // `file:contains('foo_bar')` lowers to an executable RawString leaf that
    // OR's with a structural body, mirroring native `'foo_bar' OR match { ... }`.
    // `'foo_bar'` raw-matches `zeta_raw`; the structural body matches `alpha_rust`.
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_raw_string_or_parity",
        sg_query: r#"patterntype:structural file:contains('foo_bar') OR "function_item { { identifier :[name] } }""#,
        lq_query: "'foo_bar' OR match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "zeta_raw"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_raw_string_and_not_parity",
        sg_query: r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT file:contains('parity_needle_alpha')"#,
        lq_query: "match { function_item { { identifier :[name] } } } AND NOT 'parity_needle_alpha'",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_predicate_and_not_parity",
        sg_query: r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT repo:has.file(src/lib.rs)"#,
        lq_query: "match { function_item { { identifier :[name] } } } AND NOT repo:has.file(src/lib.rs)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_mixed_lexical_and_not_parity",
        sg_query: r#"repo:repo-e2e patterntype:structural parity_needle_alpha AND NOT "function_item { { identifier :[name] } }""#,
        lq_query: "repo:repo-e2e parity_needle_alpha AND NOT match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["beta_py", "delta_other_path"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_pure_negative_root_parity",
        sg_query: r#"repo:repo-e2e path:src/lib.rs lang:rust patterntype:structural NOT "trait_item""#,
        lq_query: "repo:repo-e2e file:src/lib.rs lang:rust NOT match { trait_item }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
];

const AUTHORITY_CORPUS: &[CorpusRow] = &[
    CorpusRow {
        id: "repo_commit_after_alpha",
        path: "src/repo-commit-after-a.rs",
        content: "fn parity_needle_alpha() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "repo_commit_after_beta",
        path: "src/repo-commit-after-b.py",
        content: "def parity_needle_alpha(): pass",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_changed",
        path: "src/changed.rs",
        content: "fn parity_changed_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_changed_other",
        path: "src/changed-other.rs",
        content: "fn parity_changed_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_stale",
        path: "src/stale.rs",
        content: "fn parity_window_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_snapshot",
        path: "src/snap.rs",
        content: "fn parity_snapshot_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_owner",
        path: "src/owner.rs",
        content: "fn parity_owner_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_fresh",
        path: "src/fresh.rs",
        content: "fn parity_window_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_snapshot_other",
        path: "src/snap-other.rs",
        content: "fn parity_snapshot_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_owner_other",
        path: "src/owner-other.rs",
        content: "fn parity_owner_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_service",
        path: "src/service.rs",
        content: "fn parity_service_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_service_other",
        path: "src/service-other.rs",
        content: "fn parity_service_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_layer",
        path: "src/layer.rs",
        content: "fn parity_layer_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_layer_other",
        path: "src/layer-other.rs",
        content: "fn parity_layer_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_surface",
        path: "src/surface.rs",
        content: "fn parity_surface_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "catalog_surface_other",
        path: "src/surface-other.rs",
        content: "fn parity_surface_needle() {}",
        symbol_name: None,
        structural_identifier: None,
    },
];

const PARITY_HISTORY_COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const PARITY_HISTORY_COMMIT_SHA_LATER: &str = "89abcdef0123456789abcdef0123456789abcdef";
const PARITY_SOURCE_REPO_ID: &str = "corp-parity";

const AUTHORITY_SCENARIOS: &[ParityScenario] = &[
    ParityScenario {
        route: QueryRoute::HistoryCommits,
        id: "history_before_filter_parity",
        sg_query: "type:commit before:1970-01-01T00:00:00.020Z parity_needle_alpha",
        lq_query: "type:commit before:1970-01-01T00:00:00.020Z parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::HistoryCommits {
            shas: &[PARITY_HISTORY_COMMIT_SHA],
        },
    },
    ParityScenario {
        route: QueryRoute::HistoryCommits,
        id: "history_since_filter_parity",
        sg_query: "type:commit since:1970-01-01T00:00:00.022Z parity_needle_alpha",
        lq_query: "type:commit since:1970-01-01T00:00:00.022Z parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::HistoryCommits {
            shas: &[PARITY_HISTORY_COMMIT_SHA_LATER],
        },
    },
    ParityScenario {
        route: QueryRoute::HistoryCommits,
        id: "history_after_filter_parity",
        sg_query: "type:commit after:1970-01-01T00:00:00.015Z parity_needle_alpha",
        lq_query: "type:commit after:1970-01-01T00:00:00.015Z parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::HistoryCommits {
            shas: &[PARITY_HISTORY_COMMIT_SHA_LATER],
        },
    },
    ParityScenario {
        route: QueryRoute::HistoryCommits,
        id: "history_until_filter_parity",
        sg_query: "type:commit until:1970-01-01T00:00:00.013Z parity_needle_alpha",
        lq_query: "type:commit until:1970-01-01T00:00:00.013Z parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::HistoryCommits {
            shas: &[PARITY_HISTORY_COMMIT_SHA],
        },
    },
    ParityScenario {
        route: QueryRoute::HistoryDiffPaths,
        id: "history_diff_removed_filter_parity",
        sg_query: "type:diff diff.removed:history",
        lq_query: "type:diff diff.removed:history",
        top_k: 10,
        expected: ExpectedOutcome::HistoryDiffPaths {
            paths: &["src/lib.rs"],
        },
    },
    ParityScenario {
        route: QueryRoute::HistoryDiffPaths,
        id: "history_diff_touched_filter_parity",
        sg_query: "type:diff diff.touched:history",
        lq_query: "type:diff diff.touched:history",
        top_k: 10,
        expected: ExpectedOutcome::HistoryDiffPaths {
            paths: &["src/lib.rs"],
        },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_changed_filter_parity",
        sg_query: "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs parity_changed_needle",
        lq_query: "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs parity_changed_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["catalog_changed"],
        },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_stale_filter_parity",
        sg_query: "stale:before=1970-01-01T00:00:00.030Z file:src/stale.rs parity_window_needle",
        lq_query: "stale:before=1970-01-01T00:00:00.030Z file:src/stale.rs parity_window_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["catalog_stale"],
        },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_stale_filter_miss_parity",
        sg_query: "stale:before=1970-01-01T00:00:00.010Z file:src/stale.rs parity_window_needle",
        lq_query: "stale:before=1970-01-01T00:00:00.010Z file:src/stale.rs parity_window_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_snapshot_filter_parity",
        sg_query: "snapshot:active parity_snapshot_needle",
        lq_query: "snapshot:active parity_snapshot_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["catalog_snapshot"],
        },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_snapshot_filter_miss_parity",
        sg_query: "snapshot:active file:src/snap-other.rs parity_snapshot_needle",
        lq_query: "snapshot:active file:src/snap-other.rs parity_snapshot_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_meta_owner_filter_parity",
        sg_query: "meta.owner:team-a parity_owner_needle",
        lq_query: "meta.owner:team-a parity_owner_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["catalog_owner"],
        },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_meta_owner_filter_miss_parity",
        sg_query: "meta.owner:team-a file:src/owner-other.rs parity_owner_needle",
        lq_query: "meta.owner:team-a file:src/owner-other.rs parity_owner_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_meta_service_filter_parity",
        sg_query: "meta.service:search parity_service_needle",
        lq_query: "meta.service:search parity_service_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["catalog_service"],
        },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_meta_service_filter_miss_parity",
        sg_query: "meta.service:search file:src/service-other.rs parity_service_needle",
        lq_query: "meta.service:search file:src/service-other.rs parity_service_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_meta_layer_filter_parity",
        sg_query: "meta.layer:index parity_layer_needle",
        lq_query: "meta.layer:index parity_layer_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["catalog_layer"],
        },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_meta_layer_filter_miss_parity",
        sg_query: "meta.layer:index file:src/layer-other.rs parity_layer_needle",
        lq_query: "meta.layer:index file:src/layer-other.rs parity_layer_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_meta_surface_filter_parity",
        sg_query: "meta.surface:lexical parity_surface_needle",
        lq_query: "meta.surface:lexical parity_surface_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["catalog_surface"],
        },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_meta_surface_filter_miss_parity",
        sg_query: "meta.surface:lexical file:src/surface-other.rs parity_surface_needle",
        lq_query: "meta.surface:lexical file:src/surface-other.rs parity_surface_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_affected_filter_parity",
        sg_query: "affected:rebuild=lexical parity_changed_needle",
        lq_query: "affected:rebuild=lexical parity_changed_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["catalog_changed"],
        },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_affected_filter_miss_parity",
        sg_query: "affected:rebuild=semantic parity_changed_needle",
        lq_query: "affected:rebuild=semantic parity_changed_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_invalidated_by_filter_parity",
        sg_query: "invalidated_by:rebuild=lexical parity_changed_needle",
        lq_query: "invalidated_by:rebuild=lexical parity_changed_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["catalog_changed"],
        },
    },
    ParityScenario {
        route: QueryRoute::RuntimeMetadata,
        id: "runtime_invalidated_by_filter_miss_parity",
        sg_query: "invalidated_by:rebuild=semantic parity_changed_needle",
        lq_query: "invalidated_by:rebuild=semantic parity_changed_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_commit_after_predicate_parity",
        sg_query: "repo:has.commit.after(2024-01-01) parity_needle_alpha",
        lq_query: "repo.has.commit.after(2024-01-01) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha", "repo_commit_after_beta"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_contains_commit_after_alias_human_parity",
        sg_query: "repo:contains.commit.after(yesterday) parity_needle_alpha",
        lq_query: "repo.contains.commit.after(yesterday) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha", "repo_commit_after_beta"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_meta_predicate_parity",
        sg_query: "repo:has.meta(license:apache-2.0) parity_needle_alpha",
        lq_query: "repo.has.meta(license:apache-2.0) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha", "repo_commit_after_beta"],
        },
    },
    // SGX-03: key-only and `tag:` are key-EXISTENCE shapes (key present with any
    // value). Both parity repos carry `license=apache-2.0`, so existence of
    // `license` gates both.
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_meta_key_only_existence_parity",
        sg_query: "repo:has.meta(license) parity_needle_alpha",
        lq_query: "repo.has.meta(license) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha", "repo_commit_after_beta"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_meta_tag_existence_parity",
        sg_query: "repo:has.meta(license:) parity_needle_alpha",
        lq_query: "repo.has.meta(license:) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha", "repo_commit_after_beta"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_meta_regex_key_value_parity",
        sg_query: "repo:has.meta(/license/:/apache.*/) parity_needle_alpha",
        lq_query: "repo.has.meta(/license/:/apache.*/) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha", "repo_commit_after_beta"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_meta_regex_key_only_parity",
        sg_query: "repo:has.meta(/license/) parity_needle_alpha",
        lq_query: "repo.has.meta(/license/) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha", "repo_commit_after_beta"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_meta_exact_key_regex_value_parity",
        sg_query: "repo:has.meta(license:/apache.*/) parity_needle_alpha",
        lq_query: "repo.has.meta(license:/apache.*/) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha", "repo_commit_after_beta"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_meta_regex_key_exact_value_parity",
        sg_query: "repo:has.meta(/licens./:apache-2.0) parity_needle_alpha",
        lq_query: "repo.has.meta(/licens./:apache-2.0) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha", "repo_commit_after_beta"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_topic_predicate_parity",
        sg_query: "repo:has.topic(security) parity_needle_alpha",
        lq_query: "repo.has.topic(security) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha", "repo_commit_after_beta"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_owner_predicate_parity",
        sg_query: "file:has.owner(@alice) parity_needle_alpha",
        lq_query: "file.has.owner(@alice) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_contributor_predicate_parity",
        sg_query: "file:has.contributor(alice) parity_needle_alpha",
        lq_query: "file.has.contributor(alice) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_has_contributor_regex_predicate_parity",
        sg_query: r"file:has.contributor(/alice@example\.com/) parity_needle_alpha",
        lq_query: r"file.has.contributor(/alice@example\.com/) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["repo_commit_after_alpha"],
        },
    },
];

fn ingest_corpus(rt: &mut E2eRuntime) -> AnyResult<()> {
    for row in CORPUS {
        rt.ingest_text("repo-e2e", row.path, row.content)?;
        if let Some(identifier) = row.structural_identifier {
            rt.ingest_structural_function_tree(row.path, row.content, identifier)?;
        }
        if let Some(symbol_name) = row.symbol_name {
            rt.ingest_symbol("repo-e2e", row.path, row.id, symbol_name)?;
        }
    }
    Ok(())
}

fn ingest_authority_fixtures(rt: &mut E2eRuntime) -> AnyResult<()> {
    ingest_corpus(rt)?;
    rt.ingest_history_fixture_spec(&E2eHistoryFixtureSpec {
        commit_sha: PARITY_HISTORY_COMMIT_SHA,
        file_path: "src/lib.rs",
        author: "alice",
        committer: "alice",
        message: "fix: parity_needle_alpha history proof",
        author_time_ms: 11,
        committer_time_ms: 12,
        applied_at_ms: 13,
        ref_name: "refs/heads/main",
        tag_name: "v1.0.0",
        added_text: "history added line",
        removed_text: "history removed line",
        touched_text: "history touched line",
    })?;
    rt.ingest_text("repo-e2e", "src/main.rs", "fn parity_main_needle() {}")?;
    rt.ingest_history_fixture_spec(&E2eHistoryFixtureSpec {
        commit_sha: PARITY_HISTORY_COMMIT_SHA_LATER,
        file_path: "src/main.rs",
        author: "bob",
        committer: "bob",
        message: "chore: parity_needle_alpha secondary history proof",
        author_time_ms: 21,
        committer_time_ms: 22,
        applied_at_ms: 23,
        ref_name: "refs/heads/release",
        tag_name: "v2.0.0",
        added_text: "secondary added line",
        removed_text: "secondary removed line",
        touched_text: "secondary touched line",
    })?;
    rt.publish_repo_commit_recency_batch(RepoCommitRecencyIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "repo-commit-recency:parity".to_string(),
        entries: vec![RepoCommitRecencyEntry {
            source_repo_id: RepoId::new(PARITY_SOURCE_REPO_ID),
            latest_committer_time_ms: now_epoch_ms()?,
        }],
    })?;
    rt.publish_repo_meta_batch(RepoMetaIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "repo-meta:parity".to_string(),
        entries: vec![RepoMetaEntry {
            source_repo_id: RepoId::new(PARITY_SOURCE_REPO_ID),
            key: "license".to_string(),
            value: "apache-2.0".to_string(),
        }],
    })?;
    rt.publish_repo_topic_batch(RepoTopicIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "repo-topic:parity".to_string(),
        entries: vec![
            RepoTopicEntry {
                source_repo_id: RepoId::new(PARITY_SOURCE_REPO_ID),
                topic: "security".to_string(),
            },
            RepoTopicEntry {
                source_repo_id: RepoId::new(PARITY_SOURCE_REPO_ID),
                topic: "platform".to_string(),
            },
        ],
    })?;
    rt.publish_file_ownership_batch(FileOwnershipIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "file-ownership:parity".to_string(),
        entries: vec![
            FileOwnershipEntry {
                source_repo_id: RepoId::new(PARITY_SOURCE_REPO_ID),
                repo_relative_path: RepoRelativePath::new("src/repo-commit-after-a.rs"),
                owners: vec!["@alice".to_string(), "@acme/platform".to_string()],
            },
            FileOwnershipEntry {
                source_repo_id: RepoId::new(PARITY_SOURCE_REPO_ID),
                repo_relative_path: RepoRelativePath::new("src/repo-commit-after-b.py"),
                owners: vec!["@bob".to_string()],
            },
        ],
    })?;
    rt.publish_file_contributor_batch(FileContributorIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        batch_digest: "file-contributor:parity".to_string(),
        entries: vec![
            FileContributorEntry {
                source_repo_id: RepoId::new(PARITY_SOURCE_REPO_ID),
                repo_relative_path: RepoRelativePath::new("src/repo-commit-after-a.rs"),
                contributors: vec![
                    FileContributorIdentityEntry {
                        canonical: "alice".to_string(),
                        name: Some("Alice Example".to_string()),
                        email: Some("alice@example.com".to_string()),
                    },
                    FileContributorIdentityEntry {
                        canonical: "carol".to_string(),
                        name: Some("Carol Example".to_string()),
                        email: Some("carol@example.com".to_string()),
                    },
                ],
            },
            FileContributorEntry {
                source_repo_id: RepoId::new(PARITY_SOURCE_REPO_ID),
                repo_relative_path: RepoRelativePath::new("src/repo-commit-after-b.py"),
                contributors: vec![FileContributorIdentityEntry {
                    canonical: "bob".to_string(),
                    name: Some("Bob Builder".to_string()),
                    email: Some("bob@example.com".to_string()),
                }],
            },
        ],
    })?;
    for row in AUTHORITY_CORPUS {
        let _candidate_ids = rt.ingest_text_chunks(
            "repo-e2e",
            row.path,
            &[e2e_harness::E2eTextChunkSpec {
                content: row.content,
                start_line: 1,
                end_line: 2,
                source_repo_id: Some(PARITY_SOURCE_REPO_ID),
            }],
        )?;
    }
    rt.ingest_runtime_catalog(&E2eRuntimeCatalogSpec {
        producer_head_applied_at_ms: 100,
        generation_materialized_at_ms: 20,
        changed: vec![E2eRuntimeChangedSpec {
            path: "src/changed.rs".to_string(),
            applied_at_ms: 25,
        }],
        facets: vec![
            E2eRuntimeFacetSpec {
                path: "src/owner.rs".to_string(),
                owner: Some("team-a".to_string()),
                service: Some("shared-service".to_string()),
                layer: Some("shared-layer".to_string()),
                surface: Some("shared-surface".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/owner-other.rs".to_string(),
                owner: Some("team-b".to_string()),
                service: Some("shared-service".to_string()),
                layer: Some("shared-layer".to_string()),
                surface: Some("shared-surface".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/service.rs".to_string(),
                owner: Some("shared-owner".to_string()),
                service: Some("search".to_string()),
                layer: Some("shared-layer".to_string()),
                surface: Some("shared-surface".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/service-other.rs".to_string(),
                owner: Some("shared-owner".to_string()),
                service: Some("billing".to_string()),
                layer: Some("shared-layer".to_string()),
                surface: Some("shared-surface".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/layer.rs".to_string(),
                owner: Some("shared-owner".to_string()),
                service: Some("shared-service".to_string()),
                layer: Some("index".to_string()),
                surface: Some("shared-surface".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/layer-other.rs".to_string(),
                owner: Some("shared-owner".to_string()),
                service: Some("shared-service".to_string()),
                layer: Some("app".to_string()),
                surface: Some("shared-surface".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/surface.rs".to_string(),
                owner: Some("shared-owner".to_string()),
                service: Some("shared-service".to_string()),
                layer: Some("shared-layer".to_string()),
                surface: Some("lexical".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/surface-other.rs".to_string(),
                owner: Some("shared-owner".to_string()),
                service: Some("shared-service".to_string()),
                layer: Some("shared-layer".to_string()),
                surface: Some("semantic".to_string()),
            },
        ],
        snapshots: vec![E2eRuntimeSnapshotSpec {
            name: "active".to_string(),
            paths: vec!["src/changed.rs".to_string(), "src/snap.rs".to_string()],
        }],
        affected: vec![E2eRuntimeEdgeSpec {
            key: "rebuild=lexical".to_string(),
            paths: vec!["src/changed.rs".to_string()],
        }],
        invalidated_by: vec![E2eRuntimeEdgeSpec {
            key: "rebuild=lexical".to_string(),
            paths: vec!["src/changed.rs".to_string()],
        }],
    })?;
    Ok(())
}

fn corpus_tables() -> [&'static [CorpusRow]; 2] {
    [CORPUS, AUTHORITY_CORPUS]
}

fn now_epoch_ms() -> AnyResult<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow::anyhow!("system clock before unix epoch: {err}"))?
        .as_millis();
    u64::try_from(millis)
        .map_err(|err| anyhow::anyhow!("epoch milliseconds exceed u64 range: {err}"))
}

fn corpus_id_for_candidate_id(candidate_id: &str) -> Option<&'static str> {
    for table in corpus_tables() {
        if let Some(row) = table.iter().find(|row| row.id == candidate_id) {
            return Some(row.id);
        }
    }
    let rest = candidate_id.strip_prefix("e2e-")?;
    let (_, path) = rest.split_once('-')?;
    for table in corpus_tables() {
        if let Some(row) = table.iter().find(|row| row.path == path) {
            return Some(row.id);
        }
    }
    None
}

fn observed_corpus_ids(result: &E2eQueryResult) -> Result<Vec<&'static str>, Vec<String>> {
    let mut observed = Vec::with_capacity(result.candidate_ids.len());
    let mut unmapped = Vec::new();
    for candidate_id in &result.candidate_ids {
        match corpus_id_for_candidate_id(candidate_id) {
            Some(id) => observed.push(id),
            None => unmapped.push(candidate_id.clone()),
        }
    }
    if unmapped.is_empty() {
        Ok(observed)
    } else {
        Err(unmapped)
    }
}

fn raw_candidate_ids(result: &E2eQueryResult) -> Vec<String> {
    result.candidate_ids.clone()
}

fn sorted_file_owner_projection_rows(result: &E2eQueryResult) -> Vec<(String, Vec<String>)> {
    let mut rows = result
        .file_owner_rows
        .iter()
        .map(|row| {
            let mut owners = row.owners.clone();
            owners.sort();
            (row.repo_relative_path.as_str().to_string(), owners)
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

#[derive(Debug)]
struct Observation {
    corpus_ids: Vec<&'static str>,
    unmapped_candidate_ids: Vec<String>,
    typed_error_code: Option<String>,
}

fn observe(result: &E2eQueryResult) -> Observation {
    match observed_corpus_ids(result) {
        Ok(corpus_ids) => Observation {
            corpus_ids,
            unmapped_candidate_ids: Vec::new(),
            typed_error_code: result.typed_error.as_ref().map(|e| e.code.clone()),
        },
        Err(unmapped_candidate_ids) => Observation {
            corpus_ids: Vec::new(),
            unmapped_candidate_ids,
            typed_error_code: result.typed_error.as_ref().map(|e| e.code.clone()),
        },
    }
}

struct RowReport {
    id: &'static str,
    failure: Option<String>,
}

#[derive(Debug)]
struct HistoryObservation {
    commit_ids: Vec<String>,
    diff_paths: Vec<String>,
    typed_error_code: Option<String>,
}

fn observe_history(result: &E2eHistoryResult) -> HistoryObservation {
    HistoryObservation {
        commit_ids: result.commit_ids.clone(),
        diff_paths: result.diff_paths.clone(),
        typed_error_code: result.typed_error.as_ref().map(|e| e.code.clone()),
    }
}

fn execute_text_or_structural(
    rt: &mut E2eRuntime,
    route: QueryRoute,
    syntax: TextQuerySyntax,
    query: &str,
    top_k: u32,
) -> E2eQueryResult {
    match route {
        QueryRoute::Text => rt.query_text(syntax, query, top_k),
        QueryRoute::Structural => rt.query_structural(syntax, query, top_k),
        QueryRoute::HistoryCommits | QueryRoute::HistoryDiffPaths | QueryRoute::RuntimeMetadata => {
            E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                hybrid_candidates: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(crate::e2e_harness::E2eTypedError {
                    code: "HARNESS_ROUTE_MISMATCH".to_string(),
                    message: format!("text/structural execute called for {route:?}"),
                }),
            }
        }
    }
}

fn execute_history(
    rt: &mut E2eRuntime,
    syntax: TextQuerySyntax,
    query: &str,
    top_k: u32,
) -> E2eHistoryResult {
    rt.query_history(syntax, query, top_k)
}

fn assess_text(
    scenario: &ParityScenario,
    sg_result: &E2eQueryResult,
    lq_result: &E2eQueryResult,
) -> RowReport {
    let sg = observe(sg_result);
    let lq = observe(lq_result);

    if !sg.unmapped_candidate_ids.is_empty() || !lq.unmapped_candidate_ids.is_empty() {
        return RowReport {
            id: scenario.id,
            failure: Some(format!(
                "[candidate-id mapping failure] sg unmapped={:?} raw={:?} vs lq unmapped={:?} raw={:?}",
                sg.unmapped_candidate_ids,
                raw_candidate_ids(sg_result),
                lq.unmapped_candidate_ids,
                raw_candidate_ids(lq_result),
            )),
        };
    }

    if sg.corpus_ids != lq.corpus_ids || sg.typed_error_code != lq.typed_error_code {
        return RowReport {
            id: scenario.id,
            failure: Some(format!(
                "[parity violation] sg observed={{ids: {:?}, err: {:?}, raw: {:?}}} vs lq observed={{ids: {:?}, err: {:?}, raw: {:?}}}",
                sg.corpus_ids,
                sg.typed_error_code,
                raw_candidate_ids(sg_result),
                lq.corpus_ids,
                lq.typed_error_code,
                raw_candidate_ids(lq_result),
            )),
        };
    }

    assess_expected(scenario, &sg.corpus_ids, sg.typed_error_code.as_deref())
}

fn assess_history(
    scenario: &ParityScenario,
    sg_result: &E2eHistoryResult,
    lq_result: &E2eHistoryResult,
) -> RowReport {
    let sg = observe_history(sg_result);
    let lq = observe_history(lq_result);

    if sg.commit_ids != lq.commit_ids
        || sg.diff_paths != lq.diff_paths
        || sg.typed_error_code != lq.typed_error_code
    {
        return RowReport {
            id: scenario.id,
            failure: Some(format!(
                "[parity violation] sg history={{commits: {:?}, diffs: {:?}, err: {:?}}} vs lq history={{commits: {:?}, diffs: {:?}, err: {:?}}}",
                sg.commit_ids,
                sg.diff_paths,
                sg.typed_error_code,
                lq.commit_ids,
                lq.diff_paths,
                lq.typed_error_code,
            )),
        };
    }

    match &scenario.expected {
        ExpectedOutcome::HistoryCommits { shas } => {
            if let Some(code) = sg.typed_error_code.as_deref() {
                RowReport {
                    id: scenario.id,
                    failure: Some(format!(
                        "expected HistoryCommits shas={shas:?}, got typed error code={code}"
                    )),
                }
            } else if sg.commit_ids == *shas {
                RowReport {
                    id: scenario.id,
                    failure: None,
                }
            } else {
                RowReport {
                    id: scenario.id,
                    failure: Some(format!(
                        "expected HistoryCommits shas={shas:?}, got commits={:?}",
                        sg.commit_ids
                    )),
                }
            }
        }
        ExpectedOutcome::HistoryDiffPaths { paths } => {
            if let Some(code) = sg.typed_error_code.as_deref() {
                RowReport {
                    id: scenario.id,
                    failure: Some(format!(
                        "expected HistoryDiffPaths paths={paths:?}, got typed error code={code}"
                    )),
                }
            } else if sg.diff_paths == *paths {
                RowReport {
                    id: scenario.id,
                    failure: None,
                }
            } else {
                RowReport {
                    id: scenario.id,
                    failure: Some(format!(
                        "expected HistoryDiffPaths paths={paths:?}, got paths={:?}",
                        sg.diff_paths
                    )),
                }
            }
        }
        other @ (ExpectedOutcome::Candidates { .. }
        | ExpectedOutcome::TypedError { .. }
        | ExpectedOutcome::ExpectedFailing { .. }) => RowReport {
            id: scenario.id,
            failure: Some(format!(
                "history route row has unexpected expected outcome: {other:?}"
            )),
        },
    }
}

fn assess_expected(
    scenario: &ParityScenario,
    corpus_ids: &[&str],
    typed_error_code: Option<&str>,
) -> RowReport {
    match &scenario.expected {
        ExpectedOutcome::Candidates { ids } => typed_error_code.map_or_else(
            || {
                if corpus_ids == *ids {
                    RowReport {
                        id: scenario.id,
                        failure: None,
                    }
                } else {
                    RowReport {
                        id: scenario.id,
                        failure: Some(format!(
                            "expected Candidates ids={ids:?}, got ids={corpus_ids:?}"
                        )),
                    }
                }
            },
            |observed| RowReport {
                id: scenario.id,
                failure: Some(format!(
                    "expected Candidates ids={ids:?}, got typed error code={observed}"
                )),
            },
        ),
        ExpectedOutcome::TypedError { code } => match typed_error_code {
            Some(observed) if observed == *code => RowReport {
                id: scenario.id,
                failure: None,
            },
            Some(observed) => RowReport {
                id: scenario.id,
                failure: Some(format!(
                    "expected typed error code={code}, got code={observed}"
                )),
            },
            None => RowReport {
                id: scenario.id,
                failure: Some(format!(
                    "expected typed error code={code}, got ids={corpus_ids:?}"
                )),
            },
        },
        ExpectedOutcome::HistoryCommits { .. } | ExpectedOutcome::HistoryDiffPaths { .. } => {
            RowReport {
                id: scenario.id,
                failure: Some("text/structural route cannot expect history outcome".to_string()),
            }
        }
        ExpectedOutcome::ExpectedFailing {
            owner_ticket,
            reason,
            current_observation,
        } => match current_observation {
            CurrentObservation::Empty => {
                // Closed-loop: row stays GREEN while *both* syntaxes still
                // exhibit the predicted empty observation. When the owner
                // ticket lands and the engine produces real candidates,
                // the assertion flips red and the row owner must promote it.
                if corpus_ids.is_empty() && typed_error_code.is_none() {
                    RowReport {
                        id: scenario.id,
                        failure: None,
                    }
                } else {
                    RowReport {
                        id: scenario.id,
                        failure: Some(format!(
                            "[ExpectedFailing owner={owner_ticket}] predicted empty observation but observed ids={corpus_ids:?} typed_error={typed_error_code:?}; {owner_ticket} may have landed — promote this row. reason={reason}",
                        )),
                    }
                }
            }
        },
    }
}

fn assess_scenario(rt: &mut E2eRuntime, scenario: &ParityScenario) -> RowReport {
    match scenario.route {
        QueryRoute::Text | QueryRoute::Structural => {
            let sg_result = execute_text_or_structural(
                rt,
                scenario.route,
                TextQuerySyntax::Sourcegraph,
                scenario.sg_query,
                scenario.top_k,
            );
            let lq_result = execute_text_or_structural(
                rt,
                scenario.route,
                TextQuerySyntax::Native,
                scenario.lq_query,
                scenario.top_k,
            );
            assess_text(scenario, &sg_result, &lq_result)
        }
        QueryRoute::HistoryCommits | QueryRoute::HistoryDiffPaths => {
            let sg_result = execute_history(
                rt,
                TextQuerySyntax::Sourcegraph,
                scenario.sg_query,
                scenario.top_k,
            );
            let lq_result = execute_history(
                rt,
                TextQuerySyntax::Native,
                scenario.lq_query,
                scenario.top_k,
            );
            assess_history(scenario, &sg_result, &lq_result)
        }
        QueryRoute::RuntimeMetadata => {
            let sg_result = rt.query_runtime_metadata(
                TextQuerySyntax::Sourcegraph,
                scenario.sg_query,
                scenario.top_k,
            );
            let lq_result = rt.query_runtime_metadata(
                TextQuerySyntax::Native,
                scenario.lq_query,
                scenario.top_k,
            );
            assess_text(scenario, &sg_result, &lq_result)
        }
    }
}

fn run_parity_matrix(
    scenarios: &[ParityScenario],
    ingest: impl FnOnce(&mut E2eRuntime) -> AnyResult<()>,
) -> AnyResult<Vec<RowReport>> {
    let mut rt = E2eRuntime::boot()?;
    ingest(&mut rt)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let mut rt = rt.reopen();

    let mut failures = Vec::new();
    for scenario in scenarios {
        let report = assess_scenario(&mut rt, scenario);
        if report.failure.is_some() {
            failures.push(report);
        }
    }
    Ok(failures)
}

#[test]
fn dual_syntax_lowering_parity_matrix() -> AnyResult<()> {
    let mut failures = run_parity_matrix(SCENARIOS, ingest_corpus)?;
    failures.extend(run_parity_matrix(
        AUTHORITY_SCENARIOS,
        ingest_authority_fixtures,
    )?);

    if failures.is_empty() {
        return Ok(());
    }

    let total_rows = SCENARIOS.len() + AUTHORITY_SCENARIOS.len();
    let mut buf = String::new();
    writeln!(
        buf,
        "E2E-02 dual_syntax_lowering_parity_matrix: {} of {} rows failed",
        failures.len(),
        total_rows
    )?;
    for failure in &failures {
        if let Some(message) = &failure.failure {
            writeln!(buf, "  - [{}] {}", failure.id, message)?;
        }
    }
    Err(anyhow::anyhow!("{buf}"))
}

#[test]
fn select_file_owners_dual_syntax_projection_parity() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_authority_fixtures(&mut rt)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let mut rt = rt.reopen();

    let sg = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "select:file.owners parity_needle_alpha",
        10,
    );
    let lq = rt.query_text(
        TextQuerySyntax::Native,
        "select:file.owners parity_needle_alpha",
        10,
    );
    ensure!(
        sg.typed_error.is_none(),
        "sg select:file.owners typed-error: {:?}",
        sg.typed_error,
    );
    ensure!(
        lq.typed_error.is_none(),
        "lq select:file.owners typed-error: {:?}",
        lq.typed_error,
    );
    ensure!(
        raw_candidate_ids(&sg) == raw_candidate_ids(&lq),
        "select:file.owners candidate parity drift: sg={:?} lq={:?}",
        raw_candidate_ids(&sg),
        raw_candidate_ids(&lq),
    );
    ensure!(
        sorted_file_owner_projection_rows(&sg) == sorted_file_owner_projection_rows(&lq),
        "select:file.owners projection parity drift: sg={:?} lq={:?}",
        sorted_file_owner_projection_rows(&sg),
        sorted_file_owner_projection_rows(&lq),
    );
    ensure!(
        sorted_file_owner_projection_rows(&sg)
            == [
                ("scripts/helper.py".to_string(), Vec::new()),
                ("src/lib.rs".to_string(), Vec::new()),
                ("src/other.rs".to_string(), Vec::new()),
                (
                    "src/repo-commit-after-a.rs".to_string(),
                    vec!["@acme/platform".to_string(), "@alice".to_string()],
                ),
                (
                    "src/repo-commit-after-b.py".to_string(),
                    vec!["@bob".to_string()],
                ),
            ],
        "select:file.owners projection rows drifted: {:?}",
        sorted_file_owner_projection_rows(&sg),
    );
    Ok(())
}

#[test]
fn native_structural_mixed_scoped_file_predicates_execute_on_current_route() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_corpus(&mut rt)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let mut rt = rt.reopen();

    let lexical_file_contains = rt.query_text(
        TextQuerySyntax::Native,
        "file.contains(path:src/lib.rs, parity_needle_alpha)",
        10,
    );
    ensure!(
        lexical_file_contains.typed_error.is_none(),
        "native lexical file.contains typed-error: {:?}",
        lexical_file_contains.typed_error,
    );
    let lexical_contains_obs = observe(&lexical_file_contains);
    ensure!(
        lexical_contains_obs.corpus_ids == ["alpha_rust"],
        "native lexical file.contains expected alpha_rust, got raw={:?} mapped={:?}",
        raw_candidate_ids(&lexical_file_contains),
        lexical_contains_obs.corpus_ids,
    );

    let structural_first_file_contains = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { identifier :[name] } } } AND file.contains(path:src/lib.rs, parity_needle_alpha)",
        10,
    );
    ensure!(
        structural_first_file_contains.typed_error.is_none(),
        "native structural-first mixed file.contains typed-error: {:?}",
        structural_first_file_contains.typed_error,
    );
    let structural_first_contains_obs = observe(&structural_first_file_contains);
    ensure!(
        structural_first_contains_obs.corpus_ids == ["alpha_rust"],
        "native structural-first mixed file.contains expected alpha_rust, got raw={:?} mapped={:?}",
        raw_candidate_ids(&structural_first_file_contains),
        structural_first_contains_obs.corpus_ids,
    );

    let file_contains = rt.query_structural(
        TextQuerySyntax::Native,
        "file.contains(path:src/lib.rs, parity_needle_alpha) AND match { function_item { { identifier :[name] } } }",
        10,
    );
    ensure!(
        file_contains.typed_error.is_none(),
        "native structural mixed file.contains typed-error: {:?}",
        file_contains.typed_error,
    );
    let contains_obs = observe(&file_contains);
    ensure!(
        contains_obs.corpus_ids == ["alpha_rust"],
        "native structural mixed file.contains expected alpha_rust, got raw={:?} mapped={:?}",
        raw_candidate_ids(&file_contains),
        contains_obs.corpus_ids,
    );

    let lexical_file_has_content = rt.query_text(
        TextQuerySyntax::Native,
        "file.has.content(path:src/lib.rs, parity_needle_alpha)",
        10,
    );
    ensure!(
        lexical_file_has_content.typed_error.is_none(),
        "native lexical file.has.content typed-error: {:?}",
        lexical_file_has_content.typed_error,
    );
    let lexical_has_content_obs = observe(&lexical_file_has_content);
    ensure!(
        lexical_has_content_obs.corpus_ids == ["alpha_rust"],
        "native lexical file.has.content expected alpha_rust, got raw={:?} mapped={:?}",
        raw_candidate_ids(&lexical_file_has_content),
        lexical_has_content_obs.corpus_ids,
    );

    let structural_first_file_has_content = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { identifier :[name] } } } AND file.has.content(path:src/lib.rs, parity_needle_alpha)",
        10,
    );
    ensure!(
        structural_first_file_has_content.typed_error.is_none(),
        "native structural-first mixed file.has.content typed-error: {:?}",
        structural_first_file_has_content.typed_error,
    );
    let structural_first_has_content_obs = observe(&structural_first_file_has_content);
    ensure!(
        structural_first_has_content_obs.corpus_ids == ["alpha_rust"],
        "native structural-first mixed file.has.content expected alpha_rust, got raw={:?} mapped={:?}",
        raw_candidate_ids(&structural_first_file_has_content),
        structural_first_has_content_obs.corpus_ids,
    );

    let file_has_content = rt.query_structural(
        TextQuerySyntax::Native,
        "file.has.content(path:src/lib.rs, parity_needle_alpha) AND match { function_item { { identifier :[name] } } }",
        10,
    );
    ensure!(
        file_has_content.typed_error.is_none(),
        "native structural mixed file.has.content typed-error: {:?}",
        file_has_content.typed_error,
    );
    let has_content_obs = observe(&file_has_content);
    ensure!(
        has_content_obs.corpus_ids == ["alpha_rust"],
        "native structural mixed file.has.content expected alpha_rust, got raw={:?} mapped={:?}",
        raw_candidate_ids(&file_has_content),
        has_content_obs.corpus_ids,
    );
    Ok(())
}

#[test]
fn native_structural_symbol_has_name_projects_into_structural_chunk_scope() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    let content = "fn ParityTypeSymbol() {}";
    rt.ingest_text("repo-e2e", "src/sym-struct.rs", content)?;
    rt.ingest_structural_function_tree("src/sym-struct.rs", content, "ParityTypeSymbol")?;
    rt.ingest_symbol(
        "repo-e2e",
        "src/sym-struct.rs",
        "sym-struct-needle",
        "ParityTypeSymbol",
    )?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let mut rt = rt.reopen();

    let admitted = rt.query_structural(
        TextQuerySyntax::Native,
        "symbol.has.name(ParityTypeSymbol) AND match { function_item { { identifier :[name] } } }",
        10,
    );
    ensure!(
        admitted.typed_error.is_none(),
        "native structural symbol.has.name AND typed-error: {:?}",
        admitted.typed_error,
    );
    ensure!(
        admitted.candidate_ids.len() == 1
            && admitted
                .candidate_ids
                .first()
                .is_some_and(|id| id.ends_with("-src/sym-struct.rs")),
        "native structural symbol.has.name AND must project to one structural chunk, got {:?}",
        admitted.candidate_ids,
    );

    let or = rt.query_structural(
        TextQuerySyntax::Native,
        "symbol.has.name(ParityTypeSymbol) OR match { trait_item }",
        10,
    );
    ensure!(
        or.typed_error.is_none(),
        "native structural symbol.has.name OR typed-error: {:?}",
        or.typed_error,
    );
    ensure!(
        or.candidate_ids.len() == 1 && or.candidate_ids == admitted.candidate_ids,
        "native structural symbol.has.name OR must preserve the projected structural chunk, got {:?}",
        or.candidate_ids,
    );

    let and_not = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { identifier :[name] } } } AND NOT symbol.has.name(ParityTypeSymbol)",
        10,
    );
    ensure!(
        and_not.typed_error.is_none(),
        "native structural symbol.has.name AND NOT typed-error: {:?}",
        and_not.typed_error,
    );
    ensure!(
        and_not.candidate_ids.is_empty(),
        "native structural symbol.has.name AND NOT must subtract the projected chunk, got {:?}",
        and_not.candidate_ids,
    );

    let sg_and = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural symbol:has.name(ParityTypeSymbol) AND "function_item { { identifier :[name] } }""#,
        10,
    );
    ensure!(
        sg_and.typed_error.is_none(),
        "sourcegraph structural symbol.has.name AND typed-error: {:?}",
        sg_and.typed_error,
    );
    ensure!(
        sg_and.candidate_ids == admitted.candidate_ids,
        "sourcegraph structural symbol.has.name AND must match native projection, got {:?}",
        sg_and.candidate_ids,
    );

    let sg_or = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural symbol:has.name(ParityTypeSymbol) OR "trait_item""#,
        10,
    );
    ensure!(
        sg_or.typed_error.is_none(),
        "sourcegraph structural symbol.has.name OR typed-error: {:?}",
        sg_or.typed_error,
    );
    ensure!(
        sg_or.candidate_ids == or.candidate_ids,
        "sourcegraph structural symbol.has.name OR must preserve the projected chunk, got {:?}",
        sg_or.candidate_ids,
    );

    let sg_and_not = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural "identifier :[name]" AND NOT symbol:has.name(ParityTypeSymbol)"#,
        10,
    );
    ensure!(
        sg_and_not.typed_error.is_none(),
        "sourcegraph structural symbol.has.name AND NOT typed-error: {:?}",
        sg_and_not.typed_error,
    );
    ensure!(
        sg_and_not.candidate_ids == and_not.candidate_ids,
        "sourcegraph structural symbol.has.name AND NOT must match native subtraction, got {:?}",
        sg_and_not.candidate_ids,
    );
    Ok(())
}
