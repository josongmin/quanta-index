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

use crate::artifact::{BenchSyntax, ResultShape};
use crate::harness::{E2eHistoryResult, E2eQueryResult, E2eRuntime};
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
    pub typed_error_code: Option<String>,
    pub engine_touched: Vec<String>,
    pub early_stop_reason: Option<String>,
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

/// Runtime-generated queries for adversarial scenarios, keyed by id.
///
/// Some adversarial cases exceed the parser's hard caps and are too large to
/// embed as a literal, so they are generated here. Every other scenario uses
/// its literal `query_text`.
fn adversarial_query(id: &str) -> Option<String> {
    match id {
        "adversarial.oversized_bytes.native" => Some(oversized_keyword_query()),
        "adversarial.deep_nesting.native" => Some(deep_nesting_query()),
        _ => None,
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

fn resolve_query<'a>(scenario: &'a DslBenchScenario, generated: Option<&'a str>) -> &'a str {
    if let Some(query) = generated {
        return query;
    }
    scenario.query_text
}

/// Run one scenario query against a prepared runtime and classify the result.
pub fn run_scenario_query(rt: &mut E2eRuntime, scenario: &DslBenchScenario) -> QueryOutcome {
    let syntax = to_text_syntax(scenario.syntax);
    let generated = adversarial_query(scenario.id);
    let query = resolve_query(scenario, generated.as_deref());
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
        return typed_error_outcome(error.code.clone(), engine_touched);
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
        return typed_error_outcome(error.code.clone(), engine_touched);
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
        return typed_error_outcome(error.code.clone(), Vec::new());
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

fn typed_error_outcome(code: String, engine_touched: Vec<String>) -> QueryOutcome {
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
