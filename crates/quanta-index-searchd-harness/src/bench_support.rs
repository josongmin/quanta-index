//! Glue between the bench scenario authority and the runtime harness.
//!
//! Each scenario is driven through the real harness — boot, ingest a
//! deterministic fixture, seal, activate, query. The caller owns timing;
//! this module owns *what* gets run and how the harness result maps onto an
//! artifact row.
//!
//! v1 wires the **lexical** family end-to-end. The history / runtime-catalog /
//! structural families require their dedicated fixture-seeding paths
//! (`ingest_history_fixture_spec`, `ingest_runtime_catalog`,
//! `ingest_structural_tree`); until those are wired here, such scenarios are
//! reported with an explicit `early_stop_reason = "fixture_not_seeded"` and a
//! null latency — never a fabricated number and never a silent skip.

use anyhow::Result as AnyResult;
use quanta_index_contract::TextQuerySyntax;

use crate::artifact::{BenchSyntax, ResultShape};
use crate::harness::{E2eQueryResult, E2eRuntime};
use crate::scenarios::{DslBenchScenario, FixtureKind};

/// Result cap requested for every benchmark query.
pub const TOP_K: u32 = 10;

/// Explicit marker carried on rows whose fixture seeding is not wired yet.
pub const FIXTURE_NOT_SEEDED: &str = "fixture_not_seeded";

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

impl QueryOutcome {
    /// The scenario's fixture family is not seeded in this harness yet.
    #[must_use]
    pub fn not_seeded() -> Self {
        QueryOutcome {
            result_shape: ResultShape::Empty,
            result_count: None,
            typed_error_code: None,
            engine_touched: Vec::new(),
            early_stop_reason: Some(FIXTURE_NOT_SEEDED.to_string()),
        }
    }

    /// True when the scenario actually executed a query (latency is meaningful).
    #[must_use]
    pub fn measured(&self) -> bool {
        self.early_stop_reason.is_none()
    }
}

fn to_text_syntax(syntax: BenchSyntax) -> TextQuerySyntax {
    match syntax {
        BenchSyntax::Native => TextQuerySyntax::Native,
        BenchSyntax::Sourcegraph => TextQuerySyntax::Sourcegraph,
    }
}

/// Deterministic lexical corpus covering every lexical scenario token.
///
/// Includes the keyword `parity_needle_alpha`, the phrase `sphinx of quartz`,
/// a semver-shaped `v1.2.3` (regex), the `foo_bar` substring (`file.contains`),
/// and `needle` under `src/lib.rs` (`repo:has.file`).
pub fn seed_lexical_corpus(rt: &mut E2eRuntime) -> AnyResult<()> {
    rt.ingest_text(
        "repo-bench",
        "src/lib.rs",
        "fn parity_needle_alpha() {\n    // sphinx of quartz\n    // release v1.2.3 foo_bar\n    let documentation = \"needle\";\n    let helper = documentation;\n}\n",
    )
}

/// Boot a single runtime for the warm matrix: lexical corpus, sealed + active.
///
/// v1 seeds the lexical fixture only; non-lexical scenarios short-circuit to
/// [`QueryOutcome::not_seeded`] in [`run_scenario_query`].
pub fn prepare_warm_runtime() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    seed_lexical_corpus(&mut rt)?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

/// Boot a fresh runtime for one cold scenario, seeding only its fixture.
///
/// Returns `Ok(None)` when the scenario's fixture family is not wired yet, so
/// the cold runner emits an explicit `fixture_not_seeded` row.
pub fn prepare_cold_runtime(scenario: &DslBenchScenario) -> AnyResult<Option<E2eRuntime>> {
    match scenario.fixture {
        FixtureKind::LexicalCorpus => {
            let mut rt = E2eRuntime::boot()?;
            seed_lexical_corpus(&mut rt)?;
            let _generation = rt.seal()?;
            rt.activate_last_sealed_generation()?;
            Ok(Some(rt))
        }
        FixtureKind::HistoryLedger | FixtureKind::RuntimeCatalog | FixtureKind::StructuralTree => {
            Ok(None)
        }
    }
}

/// Run one scenario query against a prepared runtime and classify the result.
pub fn run_scenario_query(rt: &mut E2eRuntime, scenario: &DslBenchScenario) -> QueryOutcome {
    match scenario.fixture {
        FixtureKind::LexicalCorpus => {
            let result = rt.query_text(to_text_syntax(scenario.syntax), scenario.query_text, TOP_K);
            outcome_from_query(&result)
        }
        FixtureKind::HistoryLedger | FixtureKind::RuntimeCatalog | FixtureKind::StructuralTree => {
            QueryOutcome::not_seeded()
        }
    }
}

/// Saturating narrowing of a `usize` candidate count into the `u64` artifact
/// field. Candidate counts never approach `u64::MAX`; saturation is a
/// defensive ceiling rather than an expected path.
fn saturating_u64(n: usize) -> u64 {
    if let Ok(value) = u64::try_from(n) {
        return value;
    }
    u64::MAX
}

fn outcome_from_query(result: &E2eQueryResult) -> QueryOutcome {
    let engine_touched = result
        .engines_touched
        .iter()
        .map(|engine| format!("{engine:?}"))
        .collect();

    if let Some(error) = &result.typed_error {
        return QueryOutcome {
            result_shape: ResultShape::TypedError,
            result_count: None,
            typed_error_code: Some(error.code.clone()),
            engine_touched,
            early_stop_reason: None,
        };
    }

    let count = saturating_u64(result.candidates.len());
    let result_shape = if count == 0 {
        ResultShape::Empty
    } else {
        ResultShape::Candidates
    };
    QueryOutcome {
        result_shape,
        result_count: Some(count),
        typed_error_code: None,
        engine_touched,
        early_stop_reason: None,
    }
}
