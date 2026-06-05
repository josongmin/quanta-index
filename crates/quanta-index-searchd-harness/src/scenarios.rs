//! Bench-owned scenario authority.
//!
//! A static table mirroring the shipped DSL surface. Query strings here are
//! verbatim copies of the repo's real test scenario truth and must not be
//! invented or altered; the harness benchmarks exactly what ships.

use std::borrow::Cow;

use crate::artifact::{BenchSyntax, ResultShape, RouteFamily};

/// How a scenario's query string is produced.
///
/// Most are literals. A few adversarial scenarios send queries that exceed the
/// parser's hard caps and are too large to embed as a literal, so they carry a
/// generator instead — keeping the query source in the scenario row, not coupled
/// to its id by an external string match.
#[derive(Clone, Copy, Debug)]
pub enum QuerySpec {
    Literal(&'static str),
    Generated(fn() -> String),
}

impl QuerySpec {
    /// The concrete query string this scenario sends.
    #[must_use]
    pub fn resolve(&self) -> Cow<'static, str> {
        match *self {
            QuerySpec::Literal(text) => Cow::Borrowed(text),
            QuerySpec::Generated(generate) => Cow::Owned(generate()),
        }
    }
}

/// A keyword query past the 16 KiB `MAX_INPUT_BYTES` cap; the tokenizer must
/// reject it typed, not truncate or panic.
fn oversized_keyword_query() -> String {
    let mut query = String::with_capacity(18_000);
    while query.len() < 17_000 {
        query.push_str("needle ");
    }
    query
}

/// Parenthesis nesting past the depth-32 `MAX_AST_DEPTH` cap; the parser must
/// reject it typed before the recursion descends.
fn deep_nesting_query() -> String {
    let depth = 64_usize;
    let mut query = String::with_capacity(200);
    for _ in 0..depth {
        query.push('(');
    }
    query.push_str("needle");
    for _ in 0..depth {
        query.push(')');
    }
    query
}

/// Backing fixture a scenario runs against.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FixtureKind {
    LexicalCorpus,
    HistoryLedger,
    RuntimeCatalog,
    StructuralTree,
}

impl FixtureKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            FixtureKind::LexicalCorpus => "lexical_corpus",
            FixtureKind::HistoryLedger => "history_ledger",
            FixtureKind::RuntimeCatalog => "runtime_catalog",
            FixtureKind::StructuralTree => "structural_tree",
        }
    }
}

/// One static DSL benchmark scenario definition.
#[derive(Clone, Copy, Debug)]
pub struct DslBenchScenario {
    pub id: &'static str,
    pub route_family: RouteFamily,
    pub syntax: BenchSyntax,
    pub query: QuerySpec,
    pub fixture: FixtureKind,
    pub expected_shape: ResultShape,
}

/// The full bench scenario table, mirroring the shipped DSL surface.
pub const SCENARIOS: &[DslBenchScenario] = &[
    // --- LEXICAL (LexicalCorpus / Candidates) ---
    DslBenchScenario {
        id: "lexical.keyword.native",
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("parity_needle_alpha"),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "lexical.phrase.native",
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("\"sphinx of quartz\""),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "lexical.regex.native",
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("/v\\d+\\.\\d+\\.\\d+/"),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "lexical.file_contains.native",
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("file.contains('oo_ba')"),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "lexical.repo_has_file.sourcegraph",
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("repo:has.file(path:src/lib.rs) needle"),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    // --- HISTORY (HistoryLedger / Commits, diff.* -> DiffPaths) ---
    // query_history requires the `type:commit` / `type:diff` route discriminator.
    DslBenchScenario {
        id: "history.since_time.native",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("type:commit since.time:1970-01-01T00:00:00.011Z fix"),
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::Commits,
    },
    DslBenchScenario {
        id: "history.since_commit.native",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("type:commit since.commit:refs/heads/main alpha_content_needle"),
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::Commits,
    },
    DslBenchScenario {
        id: "history.after.sourcegraph",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal(
            "type:commit after:1970-01-01T00:00:00.011Z alpha_content_needle",
        ),
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::Commits,
    },
    DslBenchScenario {
        id: "history.until.sourcegraph",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal(
            "type:commit until:1970-01-01T00:00:00.013Z alpha_content_needle",
        ),
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::Commits,
    },
    DslBenchScenario {
        id: "history.diff_added.native",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("type:diff diff.added:history"),
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::DiffPaths,
    },
    DslBenchScenario {
        id: "history.diff_removed.native",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("type:diff diff.removed:history"),
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::DiffPaths,
    },
    DslBenchScenario {
        id: "history.diff_touched.native",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("type:diff diff.touched:history"),
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::DiffPaths,
    },
    // --- RUNTIME_CATALOG (RuntimeCatalog / Candidates) ---
    DslBenchScenario {
        id: "runtime.dirty_no.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("dirty:no quartz"),
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.changed.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("changed:since=1970-01-01T00:00:00.010Z"),
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.stale.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("stale:before=1970-01-01T00:00:00.030Z"),
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.snapshot.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("snapshot:active"),
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.meta_owner.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("meta.owner:team-a"),
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.meta_service.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("meta.service:search"),
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.meta_layer.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("meta.layer:index"),
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.meta_surface.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("meta.surface:lexical"),
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.affected.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("affected:rebuild=lexical"),
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.invalidated_by.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("invalidated_by:rebuild=lexical"),
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    // --- STRUCTURAL ---
    // Boolean OR / NOT are lexical-route queries (`query_text`) over the
    // multi-file corpus; the genuine tree pattern is a `query_structural` route.
    DslBenchScenario {
        id: "structural.mixed_or.native",
        route_family: RouteFamily::Structural,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("parity_needle_alpha OR documentation"),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "structural.mixed_and_not.native",
        route_family: RouteFamily::Structural,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("parity_needle_alpha NOT helper"),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "structural.tree_match.native",
        route_family: RouteFamily::Structural,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("match { function_item { { identifier :[name] } } }"),
        fixture: FixtureKind::StructuralTree,
        expected_shape: ResultShape::Candidates,
    },
    // --- NATIVE <-> SOURCEGRAPH PARITY ---
    // Same semantic surface, sourcegraph syntax (translated through the lq
    // bridge) vs the native scenarios above (direct parser). Pair these with
    // their `.native` twins by `route_family` to compare cross-syntax latency.
    DslBenchScenario {
        id: "lexical.keyword.sourcegraph",
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("parity_needle_alpha"),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "structural.mixed_or.sourcegraph",
        route_family: RouteFamily::Structural,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal("parity_needle_alpha OR documentation"),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "structural.tree_match.sourcegraph",
        route_family: RouteFamily::Structural,
        syntax: BenchSyntax::Sourcegraph,
        query: QuerySpec::Literal(
            "patterntype:structural \"function_item { { identifier :[name] } }\"",
        ),
        fixture: FixtureKind::StructuralTree,
        expected_shape: ResultShape::Candidates,
    },
    // --- ADVERSARIAL (malformed / cap-boundary -> typed error, fail-closed) ---
    // These exercise the *typed-error path latency*: fail-closed must be fast.
    // The oversized / deep-nesting queries exceed the 16 KiB / depth-32 parser
    // caps, so they carry a `QuerySpec::Generated` rather than a literal.
    DslBenchScenario {
        id: "adversarial.unterminated_phrase.native",
        route_family: RouteFamily::Adversarial,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("\"unterminated"),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::TypedError,
    },
    DslBenchScenario {
        id: "adversarial.bad_regex.native",
        route_family: RouteFamily::Adversarial,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("/[/"),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::TypedError,
    },
    DslBenchScenario {
        id: "adversarial.dangling_operator.native",
        route_family: RouteFamily::Adversarial,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Literal("parity_needle_alpha AND"),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::TypedError,
    },
    DslBenchScenario {
        id: "adversarial.oversized_bytes.native",
        route_family: RouteFamily::Adversarial,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Generated(oversized_keyword_query),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::TypedError,
    },
    DslBenchScenario {
        id: "adversarial.deep_nesting.native",
        route_family: RouteFamily::Adversarial,
        syntax: BenchSyntax::Native,
        query: QuerySpec::Generated(deep_nesting_query),
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::TypedError,
    },
];

/// Linear lookup of a scenario by its stable id.
#[must_use]
pub fn scenario_by_id(id: &str) -> Option<&'static DslBenchScenario> {
    SCENARIOS.iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn scenarios_non_empty() {
        assert!(!SCENARIOS.is_empty());
    }

    #[test]
    fn all_ids_unique() {
        let mut seen: BTreeSet<&'static str> = BTreeSet::new();
        for s in SCENARIOS {
            assert!(seen.insert(s.id), "duplicate scenario id: {}", s.id);
        }
    }

    #[test]
    fn scenario_by_id_round_trips() {
        let s = scenario_by_id("lexical.keyword.native").expect("known id resolves");
        assert_eq!(s.id, "lexical.keyword.native");
        assert_eq!(s.route_family, RouteFamily::Lexical);
        assert_eq!(s.syntax, BenchSyntax::Native);
    }

    #[test]
    fn scenario_by_id_rejects_bogus() {
        assert!(scenario_by_id("no.such.scenario").is_none());
    }

    #[test]
    fn every_route_family_present() {
        for family in [
            RouteFamily::Lexical,
            RouteFamily::History,
            RouteFamily::RuntimeCatalog,
            RouteFamily::Structural,
            RouteFamily::Adversarial,
        ] {
            assert!(
                SCENARIOS.iter().any(|s| s.route_family == family),
                "route family {family:?} missing from SCENARIOS"
            );
        }
    }
}
