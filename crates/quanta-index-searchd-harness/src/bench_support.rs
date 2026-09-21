//! Glue between the bench scenario authority and the runtime harness.
//!
//! Each scenario is driven through the real harness — boot, ingest a
//! deterministic fixture, seal, activate, query. The caller owns timing; this
//! module owns *what* gets run, *which* fixture seeds it, and how the harness
//! result maps onto an artifact row.
//!
//! Dispatch is keyed on [`FixtureKind`]: it selects both the fixture seeded for
//! a scenario and the query route used to serve it —
//!
//! | fixture          | query route             | result read        |
//! | ---------------- | ----------------------- | ------------------ |
//! | `LexicalCorpus`  | `query_text`            | `candidates`       |
//! | `HistoryLedger`  | `query_history`         | `commit_ids`/diffs |
//! | `RuntimeCatalog` | `query_runtime_metadata`| `candidates`       |
//! | `StructuralTree` | `query_structural`      | `structural_results` |
//!
//! Warm mode seeds every fixture into one sealed generation; cold mode seeds
//! only the one fixture a scenario needs.

use anyhow::Result as AnyResult;
use quanta_index_contract::TextQuerySyntax;

use crate::artifact::{BenchRowV1, BenchSyntax, LatencySummary, ResultShape};
use crate::harness::{E2eErrorCode, E2eHistoryResult, E2eQueryResult, E2eRuntime};
use crate::scenarios::{DslBenchScenario, FixtureKind};

/// Result cap requested for every benchmark query.
pub const TOP_K: u32 = 10;

/// Repo id used across every benchmark fixture.
const BENCH_REPO: &str = "repo-bench";

/// The non-timing facts the harness reports for one scenario query.
///
/// Latency is measured by the caller (warm: in-process sample loop; cold:
/// fresh-process wall clock), so it is intentionally absent here.
pub struct QueryOutcome {
    pub result_shape: ResultShape,
    pub result_count: Option<u64>,
    pub typed_error_code: Option<E2eErrorCode>,
    pub engine_touched: Vec<String>,
    pub early_stop_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScenarioTruthMode {
    /// One runtime per scenario fixture.
    ///
    /// This matches the dedicated warm authority runner and the cold runner.
    IsolatedFixture,
    /// One shared runtime seeded with every fixture family.
    ///
    /// This matches the exploratory criterion bench and can widen lexical
    /// candidate counts because cross-family text chunks coexist.
    SharedWarmFixture,
}

/// Fail-fast golden-truth validation for one bench scenario outcome.
///
/// Bench runners call this outside the timed inner loop so latency artifacts
/// cannot silently drift away from shipped behavior. The same helper also
/// powers the small bench-truth smoke rail, keeping correctness authority
/// shared with the latency scenario table.
pub fn validate_scenario_outcome(
    scenario: &DslBenchScenario,
    mode: ScenarioTruthMode,
    outcome: &QueryOutcome,
) -> AnyResult<()> {
    if outcome.result_shape != scenario.expected_shape {
        return Err(anyhow::anyhow!(
            "scenario {} returned shape {:?}, expected {:?}",
            scenario.id,
            outcome.result_shape,
            scenario.expected_shape
        ));
    }
    let expected_count = match mode {
        ScenarioTruthMode::IsolatedFixture => scenario.expected_count,
        ScenarioTruthMode::SharedWarmFixture => scenario.expected_warm_count,
    };
    if expected_count.is_some() && outcome.result_count != expected_count {
        return Err(anyhow::anyhow!(
            "scenario {} returned count {:?}, expected {:?} under {:?}",
            scenario.id,
            outcome.result_count,
            expected_count,
            mode
        ));
    }
    if outcome.typed_error_code.map(E2eErrorCode::as_str) != scenario.expected_typed_error_code {
        return Err(anyhow::anyhow!(
            "scenario {} returned typed_error_code {:?}, expected {:?}",
            scenario.id,
            outcome.typed_error_code.map(E2eErrorCode::as_str),
            scenario.expected_typed_error_code
        ));
    }
    if outcome.early_stop_reason.is_some() {
        return Err(anyhow::anyhow!(
            "scenario {} unexpectedly early-stopped with {:?}",
            scenario.id,
            outcome.early_stop_reason
        ));
    }
    Ok(())
}

fn to_text_syntax(syntax: BenchSyntax) -> TextQuerySyntax {
    match syntax {
        BenchSyntax::Native => TextQuerySyntax::Native,
        BenchSyntax::Sourcegraph => TextQuerySyntax::Sourcegraph,
    }
}

/// Saturating narrowing of a `usize` count into the `u64` artifact field.
/// Counts never approach `u64::MAX`; saturation is a defensive ceiling.
fn saturating_u64(n: usize) -> u64 {
    if let Ok(value) = u64::try_from(n) {
        return value;
    }
    u64::MAX
}

// ---------------------------------------------------------------------------
// Fixtures (ingest only — sealing/activation is done once by the caller).
// ---------------------------------------------------------------------------

/// Multi-file lexical corpus covering the lexical and boolean-structural tokens.
///
/// The spread across files makes `OR` / `NOT` non-trivial: `src/with_helper.rs`
/// carries `helper` so `parity_needle_alpha NOT helper` must exclude it, while
/// `docs/intro.md` carries only `documentation` so `parity_needle_alpha OR
/// documentation` must widen to it.
const LEXICAL_CORPUS: &[(&str, &str)] = &[
    (
        "src/lib.rs",
        "fn parity_needle_alpha() {\n    // sphinx of quartz\n    // release v1.2.3 foo_bar\n    let documentation = \"needle\";\n}\n",
    ),
    ("src/other.rs", "fn parity_needle_alpha() {}\n"),
    ("docs/intro.md", "parity documentation lives here\n"),
    (
        "src/with_helper.rs",
        "fn parity_needle_alpha() {\n    let helper = 1;\n}\n",
    ),
];

fn ingest_lexical_corpus(rt: &mut E2eRuntime) -> AnyResult<()> {
    for (path, content) in LEXICAL_CORPUS {
        rt.ingest_text(BENCH_REPO, path, content)?;
    }
    Ok(())
}

/// Add a genuine `function_item` parse tree so the tree-pattern query
/// `match { function_item { { identifier :[name] } } }` matches.
///
/// Lives on its own `src/tree.rs` path: the text and the parse tree must share
/// byte-for-byte content (the harness checks a source hash), and keeping it off
/// `src/lib.rs` avoids clobbering the richer lexical corpus there.
fn ingest_structural_trees(rt: &mut E2eRuntime) -> AnyResult<()> {
    const TREE_SRC: &str = "fn parity_needle_alpha() {}";
    rt.ingest_text(BENCH_REPO, "src/tree.rs", TREE_SRC)?;
    rt.ingest_structural_function_tree("src/tree.rs", TREE_SRC, "parity_needle_alpha")
}

/// A single deterministic commit so the history predicates resolve.
///
/// The message carries `fix` and `alpha_content_needle`; the diff text carries
/// `history`; timestamps straddle the `.011Z`/`.012Z` scenario bounds.
fn ingest_history_ledger(rt: &mut E2eRuntime) -> AnyResult<()> {
    use crate::harness::E2eHistoryFixtureSpec;

    let path = "src/history.rs";
    rt.ingest_text(
        BENCH_REPO,
        path,
        "history lexical proof alpha_content_needle\n",
    )?;
    rt.ingest_history_fixture_spec(&E2eHistoryFixtureSpec {
        commit_sha: "0123456789abcdef0123456789abcdef01234567",
        file_path: path,
        author: "alice",
        committer: "alice",
        message: "fix: sample history alpha_content_needle",
        author_time_ms: 11,
        committer_time_ms: 12,
        applied_at_ms: 12,
        ref_name: "refs/heads/main",
        tag_name: "v1.0.0",
        added_text: "history added line",
        removed_text: "history removed line",
        touched_text: "history touched line",
    })
}

/// Runtime catalog covering dirty/changed/stale/snapshot/meta/affected edges.
/// Text chunks back every catalog path; `src/clean.rs` is deliberately left
/// un-dirtied so `dirty:no quartz` resolves to it.
fn ingest_runtime_catalog_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    use crate::harness::{
        E2eRuntimeCatalogSpec, E2eRuntimeChangedSpec, E2eRuntimeEdgeSpec, E2eRuntimeFacetSpec,
        E2eRuntimeSnapshotSpec,
    };

    let facet = |path: &str| E2eRuntimeFacetSpec {
        path: path.to_string(),
        owner: Some("team-a".to_string()),
        service: Some("search".to_string()),
        layer: Some("index".to_string()),
        surface: Some("lexical".to_string()),
    };

    for (path, content) in [
        ("src/clean.rs", "clean scope quartz"),
        ("src/dirty.rs", "dirty scope todo"),
        ("src/changed.rs", "fn catalog_changed_needle() {}"),
        ("src/stale.rs", "fn catalog_stale_needle() {}"),
        ("src/owner.rs", "fn catalog_owner_needle() {}"),
        ("src/service.rs", "fn catalog_service_needle() {}"),
        ("src/layer.rs", "fn catalog_layer_needle() {}"),
        ("src/surface.rs", "fn catalog_surface_needle() {}"),
        ("src/snap.rs", "fn catalog_snapshot_needle() {}"),
    ] {
        rt.ingest_text(BENCH_REPO, path, content)?;
    }
    rt.ingest_dirty_for_path("src/dirty.rs", 100)?;

    rt.ingest_runtime_catalog(&E2eRuntimeCatalogSpec {
        producer_head_applied_at_ms: 100,
        generation_materialized_at_ms: 20,
        changed: vec![
            E2eRuntimeChangedSpec {
                path: "src/changed.rs".to_string(),
                applied_at_ms: 25,
            },
            E2eRuntimeChangedSpec {
                path: "src/stale.rs".to_string(),
                applied_at_ms: 15,
            },
        ],
        facets: vec![
            facet("src/owner.rs"),
            facet("src/service.rs"),
            facet("src/layer.rs"),
            facet("src/surface.rs"),
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
    })
}

fn seal_and_activate(rt: &mut E2eRuntime) -> AnyResult<()> {
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()
}

/// The bytes one fixture family ingests, as `(path, content)` pairs in
/// ingest order, for the artifact's corpus digest (QI-BB-010).
///
/// A fixture that is not a plain file (the history commit, the runtime
/// catalog) is rendered canonically under a `fixture://` path so a change to
/// any field it seeds changes the digest. This is the same data the
/// `ingest_*` functions above seed; the two must move together, and the
/// digest test in this module pins that every family is covered.
#[must_use]
pub fn fixture_corpus_files(kind: FixtureKind) -> Vec<(String, String)> {
    match kind {
        FixtureKind::LexicalCorpus => LEXICAL_CORPUS
            .iter()
            .map(|(path, content)| ((*path).to_string(), (*content).to_string()))
            .collect(),
        FixtureKind::StructuralTree => {
            let mut files = fixture_corpus_files(FixtureKind::LexicalCorpus);
            files.push((
                "src/tree.rs".to_string(),
                "fn parity_needle_alpha() {}".to_string(),
            ));
            files.push((
                "fixture://structural/src/tree.rs".to_string(),
                "function_item parity_needle_alpha".to_string(),
            ));
            files
        }
        FixtureKind::HistoryLedger => vec![
            (
                "src/history.rs".to_string(),
                "history lexical proof alpha_content_needle\n".to_string(),
            ),
            (
                "fixture://history/0123456789abcdef0123456789abcdef01234567".to_string(),
                "file=src/history.rs author=alice committer=alice message=fix: sample history alpha_content_needle author_time_ms=11 committer_time_ms=12 applied_at_ms=12 ref=refs/heads/main tag=v1.0.0 added=history added line removed=history removed line touched=history touched line".to_string(),
            ),
        ],
        FixtureKind::RuntimeCatalog => {
            let mut files: Vec<(String, String)> = [
                ("src/clean.rs", "clean scope quartz"),
                ("src/dirty.rs", "dirty scope todo"),
                ("src/changed.rs", "fn catalog_changed_needle() {}"),
                ("src/stale.rs", "fn catalog_stale_needle() {}"),
                ("src/owner.rs", "fn catalog_owner_needle() {}"),
                ("src/service.rs", "fn catalog_service_needle() {}"),
                ("src/layer.rs", "fn catalog_layer_needle() {}"),
                ("src/surface.rs", "fn catalog_surface_needle() {}"),
                ("src/snap.rs", "fn catalog_snapshot_needle() {}"),
            ]
            .iter()
            .map(|(path, content)| ((*path).to_string(), (*content).to_string()))
            .collect();
            files.push((
                "fixture://runtime-catalog".to_string(),
                "dirty=src/dirty.rs@100 producer_head_applied_at_ms=100 generation_materialized_at_ms=20 changed=src/changed.rs@25,src/stale.rs@15 facets=src/owner.rs,src/service.rs,src/layer.rs,src/surface.rs:team-a/search/index/lexical snapshots=active:src/changed.rs,src/snap.rs affected=rebuild=lexical:src/changed.rs invalidated_by=rebuild=lexical:src/changed.rs".to_string(),
            ));
            files
        }
    }
}

/// Every fixture family's bytes in the order `prepare_warm_runtime` seeds
/// them: the corpus of the warm matrix and the tail rail.
#[must_use]
pub fn warm_fixture_corpus_files() -> Vec<(String, String)> {
    let mut files = fixture_corpus_files(FixtureKind::LexicalCorpus);
    files.extend(
        fixture_corpus_files(FixtureKind::StructuralTree)
            .into_iter()
            .filter(|(path, _)| path.contains("tree")),
    );
    files.extend(fixture_corpus_files(FixtureKind::HistoryLedger));
    files.extend(fixture_corpus_files(FixtureKind::RuntimeCatalog));
    files
}

/// One artifact row for a scenario from the harness's classification of
/// its query and the caller's latency measurement.
#[must_use]
pub fn bench_row(
    scenario: &DslBenchScenario,
    outcome: QueryOutcome,
    latency: Option<LatencySummary>,
) -> BenchRowV1 {
    BenchRowV1 {
        scenario_id: scenario.id.to_string(),
        route_family: scenario.route_family,
        syntax: scenario.syntax,
        result_shape: outcome.result_shape,
        latency,
        qps: None,
        error_count: u64::from(outcome.typed_error_code.is_some()),
        timeout_count: 0,
        result_count: outcome.result_count,
        typed_error_code: outcome
            .typed_error_code
            .map(|code| code.as_str().to_owned()),
        engine_touched: outcome.engine_touched,
        early_stop_reason: outcome.early_stop_reason,
    }
}

// ---------------------------------------------------------------------------
// Preparation entry points.
// ---------------------------------------------------------------------------

/// Boot one runtime for the warm matrix: every fixture, one sealed + active
/// generation. All scenario families serve from this single runtime.
pub fn prepare_warm_runtime() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    ingest_lexical_corpus(&mut rt)?;
    ingest_structural_trees(&mut rt)?;
    ingest_history_ledger(&mut rt)?;
    ingest_runtime_catalog_fixture(&mut rt)?;
    seal_and_activate(&mut rt)?;
    Ok(rt)
}

/// Boot a fresh runtime for one cold scenario, seeding only its fixture.
pub fn prepare_cold_runtime(scenario: &DslBenchScenario) -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    match scenario.fixture {
        FixtureKind::LexicalCorpus => ingest_lexical_corpus(&mut rt)?,
        FixtureKind::StructuralTree => {
            ingest_lexical_corpus(&mut rt)?;
            ingest_structural_trees(&mut rt)?;
        }
        FixtureKind::HistoryLedger => ingest_history_ledger(&mut rt)?,
        FixtureKind::RuntimeCatalog => ingest_runtime_catalog_fixture(&mut rt)?,
    }
    seal_and_activate(&mut rt)?;
    Ok(rt)
}

// ---------------------------------------------------------------------------
// Query dispatch + result classification.
// ---------------------------------------------------------------------------

/// Run one scenario query against a prepared runtime and classify the result.
pub fn run_scenario_query(rt: &mut E2eRuntime, scenario: &DslBenchScenario) -> QueryOutcome {
    let syntax = to_text_syntax(scenario.syntax);
    let resolved = scenario.query.resolve();
    let query = resolved.as_ref();
    match scenario.fixture {
        FixtureKind::LexicalCorpus => outcome_from_query(&rt.query_text(syntax, query, TOP_K)),
        FixtureKind::RuntimeCatalog => {
            outcome_from_query(&rt.query_runtime_metadata(syntax, query, TOP_K))
        }
        FixtureKind::HistoryLedger => outcome_from_history(&rt.query_history(syntax, query, TOP_K)),
        FixtureKind::StructuralTree => {
            outcome_from_structural(&rt.query_structural(syntax, query, TOP_K))
        }
    }
}

fn engine_labels(engines: &[quanta_index_contract::EngineTouched]) -> Vec<String> {
    engines.iter().map(|engine| format!("{engine:?}")).collect()
}

fn outcome_from_query(result: &E2eQueryResult) -> QueryOutcome {
    let engine_touched = engine_labels(&result.engines_touched);
    if let Some(error) = &result.typed_error {
        return typed_error_outcome(error.code, engine_touched);
    }
    let count = saturating_u64(result.candidates.len());
    QueryOutcome {
        result_shape: shape_for_count(count, ResultShape::Candidates),
        result_count: Some(count),
        typed_error_code: None,
        engine_touched,
        early_stop_reason: None,
    }
}

fn outcome_from_structural(result: &E2eQueryResult) -> QueryOutcome {
    let engine_touched = engine_labels(&result.engines_touched);
    if let Some(error) = &result.typed_error {
        return typed_error_outcome(error.code, engine_touched);
    }
    let count = saturating_u64(result.structural_results.len());
    QueryOutcome {
        result_shape: shape_for_count(count, ResultShape::Candidates),
        result_count: Some(count),
        typed_error_code: None,
        engine_touched,
        early_stop_reason: None,
    }
}

fn outcome_from_history(result: &E2eHistoryResult) -> QueryOutcome {
    if let Some(error) = &result.typed_error {
        return typed_error_outcome(error.code, Vec::new());
    }
    let commits = result.commit_ids.len();
    let diffs = result.diff_paths.len();
    let total = saturating_u64(commits.saturating_add(diffs));
    let shape = if total == 0 {
        ResultShape::Empty
    } else if diffs > commits {
        ResultShape::DiffPaths
    } else {
        ResultShape::Commits
    };
    QueryOutcome {
        result_shape: shape,
        result_count: Some(total),
        typed_error_code: None,
        engine_touched: Vec::new(),
        early_stop_reason: None,
    }
}

fn typed_error_outcome(code: E2eErrorCode, engine_touched: Vec<String>) -> QueryOutcome {
    QueryOutcome {
        result_shape: ResultShape::TypedError,
        result_count: None,
        typed_error_code: Some(code),
        engine_touched,
        early_stop_reason: None,
    }
}

fn shape_for_count(count: u64, non_empty: ResultShape) -> ResultShape {
    if count == 0 {
        ResultShape::Empty
    } else {
        non_empty
    }
}

#[cfg(test)]
mod tests {
    //! The fixture digest inputs cover every family the runners seed.
    use super::*;
    use crate::artifact::corpus_digest;

    #[test]
    fn every_fixture_family_contributes_distinct_bytes_to_the_digest() {
        let families = [
            FixtureKind::LexicalCorpus,
            FixtureKind::StructuralTree,
            FixtureKind::HistoryLedger,
            FixtureKind::RuntimeCatalog,
        ];
        let mut digests = std::collections::BTreeSet::new();
        for kind in families {
            let files = fixture_corpus_files(kind);
            assert!(!files.is_empty(), "{kind:?} seeds something");
            assert!(
                digests.insert(corpus_digest("dsl-cold", &files)),
                "{kind:?} is distinct"
            );
        }
        let warm = warm_fixture_corpus_files();
        assert_eq!(
            warm.len(),
            LEXICAL_CORPUS.len() + 2 + 2 + 10,
            "the warm corpus is every family once"
        );
        assert!(digests.insert(corpus_digest("dsl-warm", &warm)));
    }
}
