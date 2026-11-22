//! Glue between the bench scenario authority and pinned process SDK runner.
//!
//! The pinned runner boots a daemon, publishes a deterministic fixture through
//! the public SDK, and queries its activated generation. The caller owns timing;
//! this module owns fixture digests, scenario truth, and artifact rows.
//!
//! Dispatch is keyed on [`FixtureKind`]: it selects both the fixture seeded for
//! a scenario and the query route used to serve it —
//!
//! | fixture          | SDK query route         | result read        |
//! | ---------------- | ----------------------- | ------------------ |
//! | `LexicalCorpus`  | `query_text`            | `candidates`       |
//! | `HistoryLedger`  | `query_history`         | `commit_ids`/diffs |
//! | `RuntimeCatalog` | `query_runtime_metadata`| `candidates`       |
//! | `StructuralTree` | `query_structural`      | `structural_results` |
//!
//! Warm mode seeds every fixture into one sealed generation; cold mode seeds
//! only the one fixture a scenario needs.

use anyhow::Result as AnyResult;
use quanta_index_contract::EngineTouched;

use crate::E2eErrorCode;
use crate::artifact::{BenchRowV1, LatencySummary, ResultShape};
use crate::bench_fixture::{
    RUNTIME_CATALOG_FILES, RUNTIME_CATALOG_VIRTUAL_CONTENT, STRUCTURAL_TREE_CONTENT,
    STRUCTURAL_TREE_PATH, STRUCTURAL_TREE_VIRTUAL_CONTENT,
};
use crate::history_fixture::history_fixture_corpus_files;
use crate::scenarios::{DslBenchScenario, FixtureKind, LEXICAL_CORPUS};

pub use crate::scenarios::TOP_K;

#[doc(hidden)]
pub mod pinned_process;

/// The non-timing facts the harness reports for one scenario query.
///
/// Latency is measured by the caller (warm: pinned-process SDK sample loop;
/// cold: fresh-process wall clock), so it is intentionally absent here.
pub struct QueryOutcome {
    pub result_shape: ResultShape,
    pub result_count: Option<u64>,
    pub typed_error_code: Option<E2eErrorCode>,
    pub engine_touched: Vec<String>,
    pub early_stop_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScenarioTruthMode {
    /// One pinned daemon per scenario fixture.
    ///
    /// This matches the dedicated warm authority runner and the cold runner.
    IsolatedFixture,
    /// One pinned daemon seeded with every fixture family.
    ///
    /// This matches the exploratory criterion bench and can widen lexical
    /// candidate counts because cross-family text chunks coexist.
    SharedWarmFixture,
}

/// Validate the registered scenarios through a pinned Legacy daemon and the
/// public SDK. The warm rail shares one generation and each cold scenario
/// starts a fresh process with its isolated fixture.
pub fn validate_pinned_scenario_truth(
    mode: ScenarioTruthMode,
    scenarios: impl IntoIterator<Item = &'static DslBenchScenario>,
) -> AnyResult<()> {
    use crate::legacy_reference::LegacySearchdBinary;

    let mut scenarios = scenarios.into_iter().peekable();
    anyhow::ensure!(scenarios.peek().is_some(), "DSL truth selection is empty");
    let binary = LegacySearchdBinary::from_environment()
        .map_err(|error| anyhow::anyhow!("DSL truth pinned daemon: {error}"))?;
    let validate = |client: &quanta_index_sdk::QuantaIndex,
                    pin: &quanta_index_contract::GenerationPin,
                    scenario: &DslBenchScenario|
     -> AnyResult<()> {
        pinned_process::run_scenario_query(client, pin, scenario)
            .and_then(|outcome| validate_scenario_outcome(scenario, mode, &outcome))
            .map_err(|error| anyhow::anyhow!("DSL truth scenario {}: {error:#}", scenario.id))
    };
    match mode {
        ScenarioTruthMode::SharedWarmFixture => {
            pinned_process::with_fixture(&binary, None, |client, pin, _process| {
                for scenario in scenarios {
                    validate(client, pin, scenario)?;
                }
                Ok(())
            })
        }
        ScenarioTruthMode::IsolatedFixture => {
            for scenario in scenarios {
                pinned_process::with_fixture(
                    &binary,
                    Some(scenario.fixture),
                    |client, pin, _process| validate(client, pin, scenario),
                )?;
            }
            Ok(())
        }
    }
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

/// Saturating narrowing of a `usize` count into the `u64` artifact field.
/// Counts never approach `u64::MAX`; saturation is a defensive ceiling.
fn saturating_u64(n: usize) -> u64 {
    if let Ok(value) = u64::try_from(n) {
        return value;
    }
    u64::MAX
}

/// The bytes one fixture family ingests, as `(path, content)` pairs in
/// ingest order, for the artifact's corpus digest (QI-BB-010).
///
/// A fixture that is not a plain file (the history commit, the runtime
/// catalog) is rendered canonically under a `fixture://` path so a change to
/// any field it seeds changes the digest. The pinned process fixture publisher
/// must keep the same bytes and ordering; the digest test pins every family.
#[must_use]
pub fn fixture_corpus_files(kind: FixtureKind) -> Vec<(String, String)> {
    match kind {
        FixtureKind::LexicalCorpus => LEXICAL_CORPUS
            .iter()
            .map(|(path, content)| ((*path).to_string(), (*content).to_string()))
            .collect(),
        FixtureKind::StructuralTree => {
            let mut files = fixture_corpus_files(FixtureKind::LexicalCorpus);
            files.extend(structural_corpus_files());
            files
        }
        FixtureKind::HistoryLedger => history_fixture_corpus_files(),
        FixtureKind::RuntimeCatalog => {
            let mut files: Vec<(String, String)> = RUNTIME_CATALOG_FILES
                .iter()
                .map(|(path, content)| ((*path).to_string(), (*content).to_string()))
                .collect();
            files.push((
                "fixture://runtime-catalog".to_string(),
                RUNTIME_CATALOG_VIRTUAL_CONTENT.to_string(),
            ));
            files
        }
    }
}

fn structural_corpus_files() -> [(String, String); 2] {
    [
        (
            STRUCTURAL_TREE_PATH.to_string(),
            STRUCTURAL_TREE_CONTENT.to_string(),
        ),
        (
            format!("fixture://structural/{STRUCTURAL_TREE_PATH}"),
            STRUCTURAL_TREE_VIRTUAL_CONTENT.to_string(),
        ),
    ]
}

/// Every fixture family's bytes in the order the shared warm fixture publishes
/// them: the corpus of the warm matrix and the tail rail.
#[must_use]
pub fn warm_fixture_corpus_files() -> Vec<(String, String)> {
    let mut files = fixture_corpus_files(FixtureKind::LexicalCorpus);
    files.extend(structural_corpus_files());
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

pub(crate) fn engine_labels(engines: &[EngineTouched]) -> Vec<String> {
    engines.iter().map(|engine| format!("{engine:?}")).collect()
}

pub(crate) fn candidate_outcome(count: usize, engine_touched: Vec<String>) -> QueryOutcome {
    let count = saturating_u64(count);
    QueryOutcome {
        result_shape: shape_for_count(count, ResultShape::Candidates),
        result_count: Some(count),
        typed_error_code: None,
        engine_touched,
        early_stop_reason: None,
    }
}

pub(crate) fn history_outcome(commits: usize, diffs: usize) -> QueryOutcome {
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

pub(crate) fn typed_error_outcome(code: E2eErrorCode, engine_touched: Vec<String>) -> QueryOutcome {
    QueryOutcome {
        result_shape: ResultShape::TypedError,
        result_count: None,
        typed_error_code: Some(code),
        engine_touched,
        early_stop_reason: None,
    }
}

pub(crate) fn shape_for_count(count: u64, non_empty: ResultShape) -> ResultShape {
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
