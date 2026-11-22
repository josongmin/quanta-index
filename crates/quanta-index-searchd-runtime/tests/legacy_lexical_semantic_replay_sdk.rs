//! E2E-05 lexical, semantic and hybrid determinism through a pinned old-Q SDK.

#![forbid(unsafe_code)]

use std::{error::Error, path::Path};

use quanta_index_contract::{
    ChunkId, ChunkRecord, EarlyStopReason, EngineTouched, ExplanationRow, GenerationPin,
    HybridQueryRequest, ManifestGeneration, PlannerTraceEntry, QueryConstraintSetV1,
    QueryStageKindV1, RepoId, RepoRelativePath, RevisionId, SearchExplanation,
    SemanticQueryRequest, TextQueryRequest, TextQuerySyntax, lex::LanguageCode,
};
use quanta_index_sdk::QuantaIndex;
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, SearchdBinaryProcess, legacy_source_replacement_from_chunks,
        publish_legacy_source_batch, request_when_ready,
    },
    private_tempdir,
    scale_runtime_policy::DEFAULT_HISTORY_MAX_GENERATIONS,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const LEXICAL: &[(&str, &str)] = &[
    ("src/lib.rs", "fn restart_alpha_needle() {} // restart"),
    (
        "src/helper.rs",
        "fn restart_beta_needle() {} // restart restart",
    ),
    ("docs/readme.md", "restart restart restart alpha docs"),
];
const DUAL_TRACK: &[(&str, &str)] = &[
    ("src/alpha.rs", "scope alpha keep"),
    ("src/beta.rs", "scope beta keep"),
    ("src/gamma.rs", "scope gamma keep"),
];

fn pin() -> TestResult<GenerationPin> {
    Ok(GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    ))
}

fn publish_fixture(
    client: &QuantaIndex,
    pin: &GenerationPin,
    files: &[(&str, &str)],
) -> TestResult {
    let chunks = files
        .iter()
        .enumerate()
        .map(|(offset, &(path, text))| {
            Ok(ChunkRecord {
                chunk_id: ChunkId::new(format!("e2e-{}-{path}", offset + 1)),
                repo_relative_path: RepoRelativePath::new(path),
                language: LanguageCode::new(if path.ends_with(".md") {
                    "markdown"
                } else {
                    "rust"
                })?,
                start_byte: 0,
                end_byte: u32::try_from(text.len())?,
                start_line: 1,
                end_line: 2,
                text: text.to_owned().into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            })
        })
        .collect::<TestResult<Vec<_>>>()?;
    let batch = legacy_source_replacement_from_chunks(
        &pin.repo_id,
        &pin.revision_id,
        pin.manifest_generation.get(),
        chunks,
    )?;
    let (_head, receipt) = publish_legacy_source_batch(client, &batch, None)?;
    if receipt.generation != pin.manifest_generation || !receipt.applied {
        return Err("fixture Source generation was not applied".into());
    }
    Ok(())
}

fn with_process<T>(
    binary: &LegacySearchdBinary,
    state_root: &Path,
    inspect: impl FnOnce(&QuantaIndex) -> TestResult<T>,
) -> TestResult<T> {
    let command = binary.benchmark_command(state_root, DEFAULT_HISTORY_MAX_GENERATIONS)?;
    let process = SearchdBinaryProcess::start_with_pinned_command(state_root, command)?;
    let result = process.connect().and_then(|client| inspect(&client));
    let stopped = process.stop();
    let value = result?;
    stopped?;
    binary.verify()?;
    Ok(value)
}

fn with_fresh_fixture<T>(
    binary: &LegacySearchdBinary,
    files: &[(&str, &str)],
    inspect: impl FnOnce(&QuantaIndex, &GenerationPin) -> TestResult<T>,
) -> TestResult<T> {
    let root = private_tempdir()?;
    let pin = pin()?;
    with_process(binary, &root.path().join("state"), |client| {
        publish_fixture(client, &pin, files)?;
        inspect(client, &pin)
    })
}

fn text_request(pin: &GenerationPin, text: &str, top_k: u32) -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: text.to_owned(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(pin.clone()),
        generation_selector: None,
        top_k,
        cursor: None,
    }
}

fn require_pin(observed: &GenerationPin, expected: &GenerationPin, route: &str) -> TestResult {
    if observed != expected {
        return Err(format!("{route} used another generation: {observed:?}").into());
    }
    Ok(())
}

fn require_stamped(explanation: &SearchExplanation, context: &str) -> TestResult {
    if explanation.request_id == 0 {
        return Err(format!("{context}: explanation carries no transport request_id").into());
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
struct StageSemantics {
    stage: QueryStageKindV1,
    calls: u32,
    returned_candidates: Option<u64>,
}

#[derive(Debug, PartialEq)]
struct DeterministicExplanation<'a> {
    planner_trace: &'a [PlannerTraceEntry],
    engines_touched: &'a [EngineTouched],
    engines_executed: &'a [EngineTouched],
    stage_timings: Option<Vec<StageSemantics>>,
    early_stop_reason: &'a Option<EarlyStopReason>,
    contributions: &'a [ExplanationRow],
    ranker_weights_hash: &'a [u8; 32],
    strategy: &'a str,
    summary: &'a str,
}

/// Exclude only request correlation and elapsed nanoseconds from equality.
fn deterministic_explanation(
    explanation: &SearchExplanation,
) -> TestResult<DeterministicExplanation<'_>> {
    require_stamped(explanation, "determinism projection")?;
    let SearchExplanation {
        planner_trace,
        engines_touched,
        engines_executed,
        request_id: _,
        stage_timings,
        early_stop_reason,
        contributions,
        ranker_weights_hash,
        strategy,
        summary,
    } = explanation;
    let stage_timings = stage_timings.as_ref().map(|stages| {
        stages
            .iter()
            .map(|stage| {
                let quanta_index_contract::QueryStageTimingV1 {
                    stage,
                    elapsed_ns: _,
                    calls,
                    returned_candidates,
                } = stage;
                StageSemantics {
                    stage: *stage,
                    calls: *calls,
                    returned_candidates: *returned_candidates,
                }
            })
            .collect::<Vec<_>>()
    });
    if stage_timings
        .as_ref()
        .is_some_and(|stages| stages.is_empty() || stages.iter().any(|stage| stage.calls == 0))
    {
        return Err("invalid measured stage availability or call count".into());
    }
    Ok(DeterministicExplanation {
        planner_trace,
        engines_touched,
        engines_executed,
        stage_timings,
        early_stop_reason,
        contributions,
        ranker_weights_hash,
        strategy,
        summary,
    })
}

fn lexical_ids_and_explanation(
    client: &QuantaIndex,
    pin: &GenerationPin,
) -> TestResult<(Vec<String>, SearchExplanation)> {
    let result = request_when_ready(|| {
        client
            .lexical()
            .query_request(text_request(pin, "restart_alpha_needle", 10))
    })?;
    require_pin(&result.generation, pin, "lexical")?;
    let first = result
        .results
        .first()
        .cloned()
        .ok_or("query_text returned zero candidates")?;
    let ids = result
        .results
        .iter()
        .map(|row| row.candidate_id.clone())
        .collect();
    let explained = request_when_ready(|| client.search().explain(pin.clone(), first.clone()))?;
    require_pin(&explained.generation, pin, "explain")?;
    Ok((ids, explained.explanation))
}

fn hybrid_ids_and_explanation(
    client: &QuantaIndex,
    pin: &GenerationPin,
) -> TestResult<(Vec<String>, SearchExplanation)> {
    let result = request_when_ready(|| {
        client.search().hybrid_request(HybridQueryRequest {
            text_query: text_request(pin, "scope", 50),
            semantic_query_text: "scope".to_owned(),
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k: 2,
        })
    })?;
    require_pin(&result.generation, pin, "hybrid")?;
    let ids = result
        .results
        .iter()
        .map(|row| row.candidate.candidate_id.clone())
        .collect();
    Ok((ids, result.explanation))
}

fn semantic_scope_ids_and_explanation(
    client: &QuantaIndex,
    pin: &GenerationPin,
) -> TestResult<(Vec<String>, SearchExplanation)> {
    let result = request_when_ready(|| {
        client.semantic().query_request(SemanticQueryRequest {
            query_text: "scope".to_owned(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: Some(text_request(pin, "scope", 10)),
            top_k: 2,
        })
    })?;
    require_pin(&result.generation, pin, "semantic")?;
    let explanation = result.explanation;
    let expected = [
        (QueryStageKindV1::SemanticPrepare, None),
        (QueryStageKindV1::SemanticReadView, None),
        (QueryStageKindV1::SemanticLexicalScope, Some(3)),
        (QueryStageKindV1::SemanticEmbedding, None),
        (QueryStageKindV1::SemanticDenseSearch, Some(3)),
        (QueryStageKindV1::SemanticProject, Some(2)),
    ]
    .map(|(stage, returned_candidates)| StageSemantics {
        stage,
        calls: 1,
        returned_candidates,
    });
    let projection = deterministic_explanation(&explanation)?;
    if projection.stage_timings.as_deref() != Some(expected.as_slice()) {
        return Err(format!(
            "semantic fixture lost measured stage truth: {:?}",
            projection.stage_timings
        )
        .into());
    }
    let ids = result
        .results
        .iter()
        .map(|row| row.candidate_id.clone())
        .collect();
    Ok((ids, explanation))
}

#[test]
fn explanation_projection_excludes_only_request_id_and_elapsed_time() -> TestResult {
    use quanta_index_contract::QueryStageTimingV1;
    let mut before = SearchExplanation::empty();
    before.request_id = 1;
    before.stage_timings = Some(vec![
        QueryStageTimingV1 {
            stage: QueryStageKindV1::SemanticPrepare,
            elapsed_ns: 7,
            calls: 1,
            returned_candidates: None,
        },
        QueryStageTimingV1 {
            stage: QueryStageKindV1::SemanticProject,
            elapsed_ns: 9,
            calls: 1,
            returned_candidates: Some(2),
        },
    ]);
    let mut after = before.clone();
    after.request_id = 2;
    if let Some(stages) = &mut after.stage_timings {
        for stage in stages {
            stage.elapsed_ns = 99;
        }
    }
    if deterministic_explanation(&before)? != deterministic_explanation(&after)? {
        return Err("request/elapsed-only differences must be excluded".into());
    }
    let mutations: [fn(&mut SearchExplanation); 11] = [
        |value| value.request_id = 0,
        |value| value.stage_timings = None,
        |value| value.stage_timings = Some(Vec::new()),
        |value| {
            if let Some(stages) = &mut value.stage_timings {
                stages.reverse();
            }
        },
        |value| {
            if let Some(stage) = value
                .stage_timings
                .as_mut()
                .and_then(|stages| stages.first_mut())
            {
                stage.stage = QueryStageKindV1::LexicalPrepare;
            }
        },
        |value| {
            if let Some(stage) = value
                .stage_timings
                .as_mut()
                .and_then(|stages| stages.first_mut())
            {
                stage.calls = 2;
            }
        },
        |value| {
            if let Some(stage) = value
                .stage_timings
                .as_mut()
                .and_then(|stages| stages.first_mut())
            {
                stage.returned_candidates = Some(8);
            }
        },
        |value| value.engines_executed.push(EngineTouched::Semantic),
        |value| value.summary = "changed".to_owned(),
        |value| value.strategy = "changed".to_owned(),
        |value| value.ranker_weights_hash = [1; 32],
    ];
    for (index, mutate) in mutations.into_iter().enumerate() {
        let mut mutant = after.clone();
        mutate(&mut mutant);
        if let Ok(projection) = deterministic_explanation(&mutant)
            && projection == deterministic_explanation(&before)?
        {
            return Err(format!("projection hid deterministic mutation {index}").into());
        }
    }
    Ok(())
}

#[test]
fn reopen_preserves_lexical_ids_and_explanation() -> TestResult {
    let binary = LegacySearchdBinary::from_environment()?;
    let root = private_tempdir()?;
    let state_root = root.path().join("state");
    let pin = pin()?;
    let (before_ids, before_explanation) = with_process(&binary, &state_root, |client| {
        publish_fixture(client, &pin, LEXICAL)?;
        lexical_ids_and_explanation(client, &pin)
    })?;
    let (after_ids, after_explanation) = with_process(&binary, &state_root, |client| {
        lexical_ids_and_explanation(client, &pin)
    })?;
    if before_ids != after_ids {
        return Err(format!(
            "reopen changed lexical ids: before={before_ids:?} after={after_ids:?}"
        )
        .into());
    }
    require_stamped(&before_explanation, "reopen before")?;
    require_stamped(&after_explanation, "reopen after")?;
    if deterministic_explanation(&before_explanation)?
        != deterministic_explanation(&after_explanation)?
    {
        return Err(format!(
            "reopen changed explanation: before={before_explanation:?} after={after_explanation:?}"
        )
        .into());
    }
    if !before_explanation.summary.contains("present") {
        return Err(format!(
            "expected explanation summary to contain `present`, got {}",
            before_explanation.summary
        )
        .into());
    }
    Ok(())
}

#[test]
fn fresh_reingest_replays_equivalent_lexical_ids_and_explanation() -> TestResult {
    let binary = LegacySearchdBinary::from_environment()?;
    let baseline = with_fresh_fixture(&binary, LEXICAL, lexical_ids_and_explanation)?;
    let replay = with_fresh_fixture(&binary, LEXICAL, lexical_ids_and_explanation)?;
    require_stamped(&baseline.1, "fresh re-ingest baseline")?;
    require_stamped(&replay.1, "fresh re-ingest replay")?;
    if baseline.0 != replay.0
        || deterministic_explanation(&baseline.1)? != deterministic_explanation(&replay.1)?
    {
        return Err(format!(
            "fresh re-ingest diverged from baseline: baseline={baseline:?} replay={replay:?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn reopen_preserves_semantic_scope_ids_and_explanation() -> TestResult {
    let binary = LegacySearchdBinary::from_environment()?;
    let root = private_tempdir()?;
    let state_root = root.path().join("state");
    let pin = pin()?;
    let (before_ids, before_explanation) = with_process(&binary, &state_root, |client| {
        publish_fixture(client, &pin, DUAL_TRACK)?;
        semantic_scope_ids_and_explanation(client, &pin)
    })?;
    let (after_ids, after_explanation) = with_process(&binary, &state_root, |client| {
        semantic_scope_ids_and_explanation(client, &pin)
    })?;
    if before_ids != after_ids {
        return Err(format!(
            "reopen changed semantic scoped ids: before={before_ids:?} after={after_ids:?}"
        )
        .into());
    }
    require_stamped(&before_explanation, "semantic reopen before")?;
    require_stamped(&after_explanation, "semantic reopen after")?;
    if deterministic_explanation(&before_explanation)?
        != deterministic_explanation(&after_explanation)?
    {
        return Err(format!("reopen changed semantic scoped explanation: before={before_explanation:?} after={after_explanation:?}").into());
    }
    if before_explanation.engines_touched != [EngineTouched::Lexical, EngineTouched::Semantic] {
        return Err(format!(
            "semantic scoped explanation lost engines_touched truth: {:?}",
            before_explanation.engines_touched
        )
        .into());
    }
    if before_explanation.early_stop_reason != Some(EarlyStopReason::CountReached) {
        return Err(format!(
            "expected CountReached on semantic scoped reopen proof, got {:?}",
            before_explanation.early_stop_reason
        )
        .into());
    }
    Ok(())
}

#[test]
fn fresh_reingest_replays_equivalent_hybrid_ids_and_early_stop_truth() -> TestResult {
    let binary = LegacySearchdBinary::from_environment()?;
    let baseline = with_fresh_fixture(&binary, DUAL_TRACK, hybrid_ids_and_explanation)?;
    let replay = with_fresh_fixture(&binary, DUAL_TRACK, hybrid_ids_and_explanation)?;
    if baseline.0 != replay.0 {
        return Err(format!("fresh re-ingest diverged for hybrid replay proof: baseline={baseline:?} replay={replay:?}").into());
    }
    if baseline.1.strategy != replay.1.strategy
        || baseline.1.engines_touched != replay.1.engines_touched
    {
        return Err(format!(
            "hybrid replay lost high-level explanation truth: baseline={:?} replay={:?}",
            baseline.1, replay.1
        )
        .into());
    }
    if baseline.1.early_stop_reason != Some(EarlyStopReason::CountReached)
        || replay.1.early_stop_reason != Some(EarlyStopReason::CountReached)
    {
        return Err(format!(
            "expected CountReached on hybrid replay proof, got baseline={:?} replay={:?}",
            baseline.1.early_stop_reason, replay.1.early_stop_reason
        )
        .into());
    }
    Ok(())
}
