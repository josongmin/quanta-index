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

/// Which fast verification lane owns a scenario as golden-truth authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum HellgateLane {
    TextRoute,
    StructuralRoute,
}

/// Verification metadata carried by each scenario row.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "scenario verification is a flag bag of independent per-row capability requirements; one bool per capability keeps the const scenario table readable"
)]
pub struct ScenarioVerification {
    pub bench_truth: bool,
    pub perf_compare: bool,
    pub fast_hellgate_lane: Option<HellgateLane>,
    pub requires_frontdoor: bool,
    pub requires_history: bool,
    pub requires_structural: bool,
    pub requires_cross_repo: bool,
}

#[expect(
    clippy::fn_params_excessive_bools,
    reason = "const constructor mirrors the ScenarioVerification flag bag one-to-one; a builder would defeat the const scenario-table construction"
)]
const fn verification(
    fast_hellgate_lane: Option<HellgateLane>,
    requires_frontdoor: bool,
    requires_history: bool,
    requires_structural: bool,
    requires_cross_repo: bool,
) -> ScenarioVerification {
    ScenarioVerification {
        bench_truth: true,
        perf_compare: true,
        fast_hellgate_lane,
        requires_frontdoor,
        requires_history,
        requires_structural,
        requires_cross_repo,
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
    pub expected_count: Option<u64>,
    pub expected_warm_count: Option<u64>,
    pub expected_typed_error_code: Option<&'static str>,
    pub verification: ScenarioVerification,
}

const fn ok_scenario(
    id: &'static str,
    route_family: RouteFamily,
    syntax: BenchSyntax,
    query: QuerySpec,
    fixture: FixtureKind,
    expected_shape: ResultShape,
    expected_count: u64,
) -> DslBenchScenario {
    DslBenchScenario {
        id,
        route_family,
        syntax,
        query,
        fixture,
        expected_shape,
        expected_count: Some(expected_count),
        expected_warm_count: None,
        expected_typed_error_code: None,
        verification: verification(
            Some(match route_family {
                RouteFamily::Structural => HellgateLane::StructuralRoute,
                RouteFamily::Lexical
                | RouteFamily::Semantic
                | RouteFamily::Hybrid
                | RouteFamily::Symbol
                | RouteFamily::RepoMap
                | RouteFamily::History
                | RouteFamily::RuntimeCatalog
                | RouteFamily::Adversarial => HellgateLane::TextRoute,
            }),
            false,
            matches!(route_family, RouteFamily::History),
            matches!(route_family, RouteFamily::Structural),
            false,
        ),
    }
}

const fn ok_scenario_warm_override(
    id: &'static str,
    route_family: RouteFamily,
    syntax: BenchSyntax,
    query: QuerySpec,
    fixture: FixtureKind,
    expected_shape: ResultShape,
    expected_count: u64,
    expected_warm_count: u64,
) -> DslBenchScenario {
    DslBenchScenario {
        id,
        route_family,
        syntax,
        query,
        fixture,
        expected_shape,
        expected_count: Some(expected_count),
        expected_warm_count: Some(expected_warm_count),
        expected_typed_error_code: None,
        verification: verification(
            Some(match route_family {
                RouteFamily::Structural => HellgateLane::StructuralRoute,
                RouteFamily::Lexical
                | RouteFamily::Semantic
                | RouteFamily::Hybrid
                | RouteFamily::Symbol
                | RouteFamily::RepoMap
                | RouteFamily::History
                | RouteFamily::RuntimeCatalog
                | RouteFamily::Adversarial => HellgateLane::TextRoute,
            }),
            false,
            matches!(route_family, RouteFamily::History),
            matches!(route_family, RouteFamily::Structural),
            false,
        ),
    }
}

const fn typed_error_scenario(
    id: &'static str,
    route_family: RouteFamily,
    syntax: BenchSyntax,
    query: QuerySpec,
    fixture: FixtureKind,
    expected_typed_error_code: &'static str,
) -> DslBenchScenario {
    DslBenchScenario {
        id,
        route_family,
        syntax,
        query,
        fixture,
        expected_shape: ResultShape::TypedError,
        expected_count: None,
        expected_warm_count: None,
        expected_typed_error_code: Some(expected_typed_error_code),
        verification: verification(
            Some(HellgateLane::TextRoute),
            false,
            matches!(route_family, RouteFamily::History),
            false,
            false,
        ),
    }
}

/// The full bench scenario table, mirroring the shipped DSL surface.
pub const SCENARIOS: &[DslBenchScenario] = &[
    // --- LEXICAL (LexicalCorpus / Candidates) ---
    ok_scenario_warm_override(
        "lexical.keyword.native",
        RouteFamily::Lexical,
        BenchSyntax::Native,
        QuerySpec::Literal("parity_needle_alpha"),
        FixtureKind::LexicalCorpus,
        ResultShape::Candidates,
        3,
        4,
    ),
    ok_scenario(
        "lexical.phrase.native",
        RouteFamily::Lexical,
        BenchSyntax::Native,
        QuerySpec::Literal("\"sphinx of quartz\""),
        FixtureKind::LexicalCorpus,
        ResultShape::Candidates,
        1,
    ),
    ok_scenario(
        "lexical.regex.native",
        RouteFamily::Lexical,
        BenchSyntax::Native,
        QuerySpec::Literal("/v\\d+\\.\\d+\\.\\d+/"),
        FixtureKind::LexicalCorpus,
        ResultShape::Candidates,
        1,
    ),
    ok_scenario(
        "lexical.file_contains.native",
        RouteFamily::Lexical,
        BenchSyntax::Native,
        QuerySpec::Literal("file.contains('oo_ba')"),
        FixtureKind::LexicalCorpus,
        ResultShape::Candidates,
        1,
    ),
    // QI-BB-011: a keyword matches whole tokens of the shared normalizer, in
    // which `_` never splits an identifier, so `needle` finds only the file
    // holding the standalone word (`src/lib.rs`), not the two whose
    // `parity_needle_alpha` the old analyzer split.
    ok_scenario(
        "lexical.repo_has_file.sourcegraph",
        RouteFamily::Lexical,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("repo:has.file(path:src/lib.rs) needle"),
        FixtureKind::LexicalCorpus,
        ResultShape::Candidates,
        1,
    ),
    // --- HISTORY (HistoryLedger / Commits, diff.* -> DiffPaths) ---
    // query_history requires the `type:commit` / `type:diff` route discriminator.
    ok_scenario(
        "history.since_time.native",
        RouteFamily::History,
        BenchSyntax::Native,
        QuerySpec::Literal("type:commit since.time:1970-01-01T00:00:00.011Z fix"),
        FixtureKind::HistoryLedger,
        ResultShape::Commits,
        1,
    ),
    ok_scenario(
        "history.since_commit.native",
        RouteFamily::History,
        BenchSyntax::Native,
        QuerySpec::Literal("type:commit since.commit:refs/heads/main alpha_content_needle"),
        FixtureKind::HistoryLedger,
        ResultShape::Commits,
        1,
    ),
    ok_scenario(
        "history.after.sourcegraph",
        RouteFamily::History,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("type:commit after:1970-01-01T00:00:00.011Z alpha_content_needle"),
        FixtureKind::HistoryLedger,
        ResultShape::Commits,
        1,
    ),
    ok_scenario(
        "history.until.sourcegraph",
        RouteFamily::History,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("type:commit until:1970-01-01T00:00:00.013Z alpha_content_needle"),
        FixtureKind::HistoryLedger,
        ResultShape::Commits,
        1,
    ),
    ok_scenario(
        "history.diff_added.native",
        RouteFamily::History,
        BenchSyntax::Native,
        QuerySpec::Literal("type:diff diff.added:history"),
        FixtureKind::HistoryLedger,
        ResultShape::DiffPaths,
        1,
    ),
    ok_scenario(
        "history.diff_removed.native",
        RouteFamily::History,
        BenchSyntax::Native,
        QuerySpec::Literal("type:diff diff.removed:history"),
        FixtureKind::HistoryLedger,
        ResultShape::DiffPaths,
        1,
    ),
    ok_scenario(
        "history.diff_touched.native",
        RouteFamily::History,
        BenchSyntax::Native,
        QuerySpec::Literal("type:diff diff.touched:history"),
        FixtureKind::HistoryLedger,
        ResultShape::DiffPaths,
        1,
    ),
    // --- RUNTIME_CATALOG (RuntimeCatalog / Candidates) ---
    ok_scenario(
        "runtime.dirty_no.sourcegraph",
        RouteFamily::RuntimeCatalog,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("dirty:no quartz"),
        FixtureKind::RuntimeCatalog,
        ResultShape::Candidates,
        1,
    ),
    ok_scenario(
        "runtime.changed.sourcegraph",
        RouteFamily::RuntimeCatalog,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("changed:since=1970-01-01T00:00:00.010Z"),
        FixtureKind::RuntimeCatalog,
        ResultShape::Candidates,
        2,
    ),
    ok_scenario(
        "runtime.stale.sourcegraph",
        RouteFamily::RuntimeCatalog,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("stale:before=1970-01-01T00:00:00.030Z"),
        FixtureKind::RuntimeCatalog,
        ResultShape::Candidates,
        9,
    ),
    ok_scenario(
        "runtime.snapshot.sourcegraph",
        RouteFamily::RuntimeCatalog,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("snapshot:active"),
        FixtureKind::RuntimeCatalog,
        ResultShape::Candidates,
        2,
    ),
    ok_scenario(
        "runtime.meta_owner.sourcegraph",
        RouteFamily::RuntimeCatalog,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("meta.owner:team-a"),
        FixtureKind::RuntimeCatalog,
        ResultShape::Candidates,
        4,
    ),
    ok_scenario(
        "runtime.meta_service.sourcegraph",
        RouteFamily::RuntimeCatalog,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("meta.service:search"),
        FixtureKind::RuntimeCatalog,
        ResultShape::Candidates,
        4,
    ),
    ok_scenario(
        "runtime.meta_layer.sourcegraph",
        RouteFamily::RuntimeCatalog,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("meta.layer:index"),
        FixtureKind::RuntimeCatalog,
        ResultShape::Candidates,
        4,
    ),
    ok_scenario(
        "runtime.meta_surface.sourcegraph",
        RouteFamily::RuntimeCatalog,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("meta.surface:lexical"),
        FixtureKind::RuntimeCatalog,
        ResultShape::Candidates,
        4,
    ),
    ok_scenario(
        "runtime.affected.sourcegraph",
        RouteFamily::RuntimeCatalog,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("affected:rebuild=lexical"),
        FixtureKind::RuntimeCatalog,
        ResultShape::Candidates,
        1,
    ),
    ok_scenario(
        "runtime.invalidated_by.sourcegraph",
        RouteFamily::RuntimeCatalog,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("invalidated_by:rebuild=lexical"),
        FixtureKind::RuntimeCatalog,
        ResultShape::Candidates,
        1,
    ),
    // --- STRUCTURAL ---
    // Boolean OR / NOT are lexical-route queries (`query_text`) over the
    // multi-file corpus; the genuine tree pattern is a `query_structural` route.
    ok_scenario_warm_override(
        "structural.mixed_or.native",
        RouteFamily::Structural,
        BenchSyntax::Native,
        QuerySpec::Literal("parity_needle_alpha OR documentation"),
        FixtureKind::LexicalCorpus,
        ResultShape::Candidates,
        4,
        5,
    ),
    ok_scenario_warm_override(
        "structural.mixed_and_not.native",
        RouteFamily::Structural,
        BenchSyntax::Native,
        QuerySpec::Literal("parity_needle_alpha NOT helper"),
        FixtureKind::LexicalCorpus,
        ResultShape::Candidates,
        2,
        3,
    ),
    ok_scenario(
        "structural.tree_match.native",
        RouteFamily::Structural,
        BenchSyntax::Native,
        QuerySpec::Literal("match { function_item { { identifier :[name] } } }"),
        FixtureKind::StructuralTree,
        ResultShape::Candidates,
        1,
    ),
    // --- NATIVE <-> SOURCEGRAPH PARITY ---
    // Same semantic surface, sourcegraph syntax (translated through the lq
    // bridge) vs the native scenarios above (direct parser). Pair these with
    // their `.native` twins by `route_family` to compare cross-syntax latency.
    ok_scenario_warm_override(
        "lexical.keyword.sourcegraph",
        RouteFamily::Lexical,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("parity_needle_alpha"),
        FixtureKind::LexicalCorpus,
        ResultShape::Candidates,
        3,
        4,
    ),
    ok_scenario_warm_override(
        "structural.mixed_or.sourcegraph",
        RouteFamily::Structural,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("parity_needle_alpha OR documentation"),
        FixtureKind::LexicalCorpus,
        ResultShape::Candidates,
        4,
        5,
    ),
    ok_scenario(
        "structural.tree_match.sourcegraph",
        RouteFamily::Structural,
        BenchSyntax::Sourcegraph,
        QuerySpec::Literal("patterntype:structural \"function_item { { identifier :[name] } }\""),
        FixtureKind::StructuralTree,
        ResultShape::Candidates,
        1,
    ),
    // --- ADVERSARIAL (malformed / cap-boundary -> typed error, fail-closed) ---
    // These exercise the *typed-error path latency*: fail-closed must be fast.
    // The oversized / deep-nesting queries exceed the 16 KiB / depth-32 parser
    // caps, so they carry a `QuerySpec::Generated` rather than a literal.
    typed_error_scenario(
        "adversarial.unterminated_phrase.native",
        RouteFamily::Adversarial,
        BenchSyntax::Native,
        QuerySpec::Literal("\"unterminated"),
        FixtureKind::LexicalCorpus,
        "PARSE_FAIL",
    ),
    typed_error_scenario(
        "adversarial.bad_regex.native",
        RouteFamily::Adversarial,
        BenchSyntax::Native,
        QuerySpec::Literal("/[/"),
        FixtureKind::LexicalCorpus,
        "PARSE_FAIL",
    ),
    typed_error_scenario(
        "adversarial.dangling_operator.native",
        RouteFamily::Adversarial,
        BenchSyntax::Native,
        QuerySpec::Literal("parity_needle_alpha AND"),
        FixtureKind::LexicalCorpus,
        "PARSE_FAIL",
    ),
    typed_error_scenario(
        "adversarial.oversized_bytes.native",
        RouteFamily::Adversarial,
        BenchSyntax::Native,
        QuerySpec::Generated(oversized_keyword_query),
        FixtureKind::LexicalCorpus,
        "PARSE_FAIL",
    ),
    typed_error_scenario(
        "adversarial.deep_nesting.native",
        RouteFamily::Adversarial,
        BenchSyntax::Native,
        QuerySpec::Generated(deep_nesting_query),
        FixtureKind::LexicalCorpus,
        "PARSE_FAIL",
    ),
];

/// Linear lookup of a scenario by its stable id.
#[must_use]
pub fn scenario_by_id(id: &str) -> Option<&'static DslBenchScenario> {
    SCENARIOS.iter().find(|s| s.id == id)
}

pub fn hellgate_scenarios(lane: HellgateLane) -> impl Iterator<Item = &'static DslBenchScenario> {
    SCENARIOS.iter().filter(move |scenario| {
        scenario.verification.bench_truth && scenario.verification.fast_hellgate_lane == Some(lane)
    })
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

    #[test]
    fn every_hellgate_lane_present() {
        for lane in [HellgateLane::TextRoute, HellgateLane::StructuralRoute] {
            assert!(
                hellgate_scenarios(lane).next().is_some(),
                "hellgate lane {lane:?} missing from SCENARIOS"
            );
        }
    }

    #[test]
    fn every_scenario_carries_exact_golden_truth() {
        for scenario in SCENARIOS {
            match scenario.expected_shape {
                ResultShape::TypedError => {
                    assert_eq!(scenario.expected_count, None, "{}", scenario.id);
                    assert_eq!(scenario.expected_warm_count, None, "{}", scenario.id);
                    assert!(
                        scenario.expected_typed_error_code.is_some(),
                        "{} missing typed error code",
                        scenario.id
                    );
                }
                ResultShape::Candidates
                | ResultShape::Commits
                | ResultShape::DiffPaths
                | ResultShape::Empty => {
                    assert!(
                        scenario.expected_count.is_some(),
                        "{} missing expected_count",
                        scenario.id
                    );
                    assert_eq!(
                        scenario.expected_typed_error_code, None,
                        "{} unexpectedly carries typed error code",
                        scenario.id
                    );
                }
            }
        }
    }
}
