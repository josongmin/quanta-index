//! Bench-owned scenario authority.
//!
//! A static table mirroring the shipped DSL surface. Query strings here are
//! verbatim copies of the repo's real test scenario truth and must not be
//! invented or altered; the harness benchmarks exactly what ships.

use crate::artifact::{BenchSyntax, ResultShape, RouteFamily};

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
    pub query_text: &'static str,
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
        query_text: "parity_needle_alpha",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "lexical.phrase.native",
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Native,
        query_text: "\"sphinx of quartz\"",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "lexical.regex.native",
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Native,
        query_text: "/v\\d+\\.\\d+\\.\\d+/",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "lexical.file_contains.native",
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Native,
        query_text: "file.contains('oo_ba')",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "lexical.repo_has_file.sourcegraph",
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "repo:has.file(path:src/lib.rs) needle",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    // --- HISTORY (HistoryLedger / Commits, diff.* -> DiffPaths) ---
    // query_history requires the `type:commit` / `type:diff` route discriminator.
    DslBenchScenario {
        id: "history.since_time.native",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Native,
        query_text: "type:commit since.time:1970-01-01T00:00:00.011Z fix",
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::Commits,
    },
    DslBenchScenario {
        id: "history.since_commit.native",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Native,
        query_text: "type:commit since.commit:refs/heads/main alpha_content_needle",
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::Commits,
    },
    DslBenchScenario {
        id: "history.after.sourcegraph",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "type:commit after:1970-01-01T00:00:00.011Z alpha_content_needle",
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::Commits,
    },
    DslBenchScenario {
        id: "history.until.sourcegraph",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "type:commit until:1970-01-01T00:00:00.013Z alpha_content_needle",
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::Commits,
    },
    DslBenchScenario {
        id: "history.diff_added.native",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Native,
        query_text: "type:diff diff.added:history",
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::DiffPaths,
    },
    DslBenchScenario {
        id: "history.diff_removed.native",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Native,
        query_text: "type:diff diff.removed:history",
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::DiffPaths,
    },
    DslBenchScenario {
        id: "history.diff_touched.native",
        route_family: RouteFamily::History,
        syntax: BenchSyntax::Native,
        query_text: "type:diff diff.touched:history",
        fixture: FixtureKind::HistoryLedger,
        expected_shape: ResultShape::DiffPaths,
    },
    // --- RUNTIME_CATALOG (RuntimeCatalog / Candidates) ---
    DslBenchScenario {
        id: "runtime.dirty_no.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "dirty:no quartz",
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.changed.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "changed:since=1970-01-01T00:00:00.010Z",
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.stale.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "stale:before=1970-01-01T00:00:00.030Z",
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.snapshot.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "snapshot:active",
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.meta_owner.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "meta.owner:team-a",
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.meta_service.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "meta.service:search",
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.meta_layer.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "meta.layer:index",
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.meta_surface.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "meta.surface:lexical",
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.affected.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "affected:rebuild=lexical",
        fixture: FixtureKind::RuntimeCatalog,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "runtime.invalidated_by.sourcegraph",
        route_family: RouteFamily::RuntimeCatalog,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "invalidated_by:rebuild=lexical",
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
        query_text: "parity_needle_alpha OR documentation",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "structural.mixed_and_not.native",
        route_family: RouteFamily::Structural,
        syntax: BenchSyntax::Native,
        query_text: "parity_needle_alpha NOT helper",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "structural.tree_match.native",
        route_family: RouteFamily::Structural,
        syntax: BenchSyntax::Native,
        query_text: "match { function_item { { identifier :[name] } } }",
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
        query_text: "parity_needle_alpha",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "structural.mixed_or.sourcegraph",
        route_family: RouteFamily::Structural,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "parity_needle_alpha OR documentation",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::Candidates,
    },
    DslBenchScenario {
        id: "structural.tree_match.sourcegraph",
        route_family: RouteFamily::Structural,
        syntax: BenchSyntax::Sourcegraph,
        query_text: "patterntype:structural \"function_item { { identifier :[name] } }\"",
        fixture: FixtureKind::StructuralTree,
        expected_shape: ResultShape::Candidates,
    },
    // --- ADVERSARIAL (malformed / cap-boundary -> typed error, fail-closed) ---
    // These exercise the *typed-error path latency*: fail-closed must be fast.
    // The two `oversized_*` / `deep_*` queries are generated at runtime in
    // `bench_support` (they exceed the 16 KiB / depth-32 parser caps); their
    // `query_text` here is a descriptive placeholder, not the literal sent.
    DslBenchScenario {
        id: "adversarial.unterminated_phrase.native",
        route_family: RouteFamily::Adversarial,
        syntax: BenchSyntax::Native,
        query_text: "\"unterminated",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::TypedError,
    },
    DslBenchScenario {
        id: "adversarial.bad_regex.native",
        route_family: RouteFamily::Adversarial,
        syntax: BenchSyntax::Native,
        query_text: "/[/",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::TypedError,
    },
    DslBenchScenario {
        id: "adversarial.dangling_operator.native",
        route_family: RouteFamily::Adversarial,
        syntax: BenchSyntax::Native,
        query_text: "parity_needle_alpha AND",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::TypedError,
    },
    DslBenchScenario {
        id: "adversarial.oversized_bytes.native",
        route_family: RouteFamily::Adversarial,
        syntax: BenchSyntax::Native,
        query_text: "<generated: keyword query exceeding the 16 KiB input cap>",
        fixture: FixtureKind::LexicalCorpus,
        expected_shape: ResultShape::TypedError,
    },
    DslBenchScenario {
        id: "adversarial.deep_nesting.native",
        route_family: RouteFamily::Adversarial,
        syntax: BenchSyntax::Native,
        query_text: "<generated: parenthesis nesting exceeding the depth-32 cap>",
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
    #[expect(
        clippy::expect_used,
        reason = "test asserts a known scenario id resolves"
    )]
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
