//! E2E-05 — restart/replay determinism over persisted lexical state.
//!
//! This rail proves that the public lexical query path and the public
//! `Explain` response remain stable across:
//! - runtime reopen against the same persisted state root, and
//! - a fresh re-ingest of the same fixture corpus into a new state root.

#![forbid(unsafe_code)]

use anyhow::Result as AnyResult;
use quanta_index_contract::{
    EarlyStopReason, EngineTouched, ExplanationRow, HistoryIngestBatch, HistoryRefDelete,
    HistoryRefMutation, PlannerTraceEntry, QueryStageKindV1, SearchExplanation,
    SearchPlaneTrackKind, TextQuerySyntax,
};

use crate::e2e_harness::{
    E2eRuntime, E2eRuntimeCatalogSpec, E2eRuntimeChangedSpec, E2eRuntimeEdgeSpec,
    E2eRuntimeFacetSpec, E2eRuntimeSnapshotSpec, E2eTypedError,
};

fn ingest_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    rt.ingest_text(
        "repo-e2e",
        "src/lib.rs",
        "fn restart_alpha_needle() {} // restart",
    )?;
    rt.ingest_text(
        "repo-e2e",
        "src/helper.rs",
        "fn restart_beta_needle() {} // restart restart",
    )?;
    rt.ingest_text(
        "repo-e2e",
        "docs/readme.md",
        "restart restart restart alpha docs",
    )?;
    Ok(())
}

fn require_no_typed_error(error: Option<E2eTypedError>, context: &str) -> AnyResult<()> {
    if let Some(error) = error {
        return Err(anyhow::anyhow!(
            "{context}: unexpected typed error code={} message={}",
            error.code,
            error.message
        ));
    }
    Ok(())
}

/// Require transport correlation on an explanation served over the socket
/// (S21-10): a zero `request_id` here would mean the stamp regressed, not
/// that the query was deterministic.
fn require_stamped(explanation: &SearchExplanation, context: &str) -> AnyResult<()> {
    if explanation.request_id == 0 {
        return Err(anyhow::anyhow!(
            "{context}: explanation carries no transport request_id"
        ));
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
struct StageSemantics {
    stage: QueryStageKindV1,
    calls: u32,
    returned_candidates: Option<u64>,
}

/// Only request correlation and observed elapsed time are nondeterministic.
///
/// Preserve timing availability, order, kinds, calls, counts and every other
/// explanation field. Exhaustive destructuring forces review of new fields.
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

fn deterministic_explanation(
    explanation: &SearchExplanation,
) -> AnyResult<DeterministicExplanation<'_>> {
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
        return Err(anyhow::anyhow!(
            "invalid measured stage availability or call count"
        ));
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

fn query_ids_and_explanation(rt: &mut E2eRuntime) -> AnyResult<(Vec<String>, SearchExplanation)> {
    let result = rt.query_text(TextQuerySyntax::Native, "restart_alpha_needle", 10);
    require_no_typed_error(result.typed_error, "query_text")?;
    let first = result
        .candidates
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("query_text returned zero candidates"))?;
    let ids = result.candidate_ids;
    let explain = rt.explain_candidate(first);
    require_no_typed_error(explain.typed_error, "explain_candidate")?;
    let explanation = explain
        .explanation
        .ok_or_else(|| anyhow::anyhow!("explain_candidate returned no explanation"))?;
    Ok((ids, explanation))
}

fn ingest_dual_track_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    rt.ingest_text("repo-e2e", "src/alpha.rs", "scope alpha keep")?;
    rt.ingest_text("repo-e2e", "src/beta.rs", "scope beta keep")?;
    rt.ingest_text("repo-e2e", "src/gamma.rs", "scope gamma keep")?;
    Ok(())
}

fn query_hybrid_ids_and_explanation(
    rt: &mut E2eRuntime,
) -> AnyResult<(Vec<String>, SearchExplanation)> {
    let result = rt.query_hybrid(TextQuerySyntax::Native, "scope", "scope", 2);
    require_no_typed_error(result.typed_error, "query_hybrid")?;
    let explanation = result
        .explanation
        .ok_or_else(|| anyhow::anyhow!("query_hybrid returned no explanation"))?;
    Ok((result.candidate_ids, explanation))
}

fn query_semantic_scope_ids_and_explanation(
    rt: &mut E2eRuntime,
) -> AnyResult<(Vec<String>, SearchExplanation)> {
    let result = rt.query_semantic("scope", 2, Some((TextQuerySyntax::Native, "scope", 10)));
    require_no_typed_error(result.typed_error, "query_semantic")?;
    let explanation = result
        .explanation
        .ok_or_else(|| anyhow::anyhow!("query_semantic returned no explanation"))?;
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
        return Err(anyhow::anyhow!(
            "semantic fixture lost measured stage truth: {:?}",
            projection.stage_timings
        ));
    }
    Ok((result.candidate_ids, explanation))
}

#[test]
fn explanation_projection_excludes_only_request_id_and_elapsed_time() -> AnyResult<()> {
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
        return Err(anyhow::anyhow!(
            "request/elapsed-only differences must be excluded"
        ));
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
        |value| value.summary = "changed".to_string(),
        |value| value.strategy = "changed".to_string(),
        |value| value.ranker_weights_hash = [1; 32],
    ];
    for (index, mutate) in mutations.into_iter().enumerate() {
        let mut mutant = after.clone();
        mutate(&mut mutant);
        if let Ok(projection) = deterministic_explanation(&mutant)
            && projection == deterministic_explanation(&before)?
        {
            return Err(anyhow::anyhow!(
                "projection hid deterministic mutation {index}"
            ));
        }
    }
    Ok(())
}

fn ingest_structural_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    let path = "src/structural.rs";
    let content = "fn restart_structural_alpha() {}";
    rt.ingest_text("repo-e2e", path, content)?;
    rt.ingest_structural_function_tree(path, content, "restart_structural_alpha")?;
    _ = rt.seal_lexical_generation_for_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;
    Ok(())
}

fn query_structural_ids(
    rt: &mut E2eRuntime,
    syntax: TextQuerySyntax,
    query_text: &str,
) -> AnyResult<Vec<String>> {
    let result = rt.query_structural(syntax, query_text, 10);
    require_no_typed_error(result.typed_error, "query_structural")?;
    if result.candidate_ids.is_empty() {
        return Err(anyhow::anyhow!(
            "query_structural returned zero candidates for {query_text:?}"
        ));
    }
    Ok(result.candidate_ids)
}

fn query_history_commit_ids(rt: &mut E2eRuntime, query_text: &str) -> AnyResult<Vec<String>> {
    let result = rt.query_history(TextQuerySyntax::Sourcegraph, query_text, 10);
    require_no_typed_error(result.typed_error, "query_history")?;
    Ok(result.commit_ids)
}

fn query_runtime_dirty_ids(rt: &mut E2eRuntime, query_text: &str) -> AnyResult<Vec<String>> {
    let result = rt.query_runtime_metadata(TextQuerySyntax::Native, query_text, 10);
    require_no_typed_error(result.typed_error, "query_runtime_metadata")?;
    Ok(result.candidate_ids)
}

fn query_runtime_catalog_ids(rt: &mut E2eRuntime, query_text: &str) -> AnyResult<Vec<String>> {
    let result = rt.query_runtime_metadata(TextQuerySyntax::Sourcegraph, query_text, 10);
    require_no_typed_error(result.typed_error, "query_runtime_catalog")?;
    Ok(result.candidate_ids)
}

fn ingest_runtime_catalog_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    for (path, content) in [
        ("src/changed.rs", "fn catalog_changed_needle() {}"),
        ("src/changed-other.rs", "fn catalog_changed_needle() {}"),
        ("src/unchanged.rs", "fn catalog_unchanged_needle() {}"),
        ("src/owner.rs", "fn catalog_owner_needle() {}"),
        ("src/service.rs", "fn catalog_service_needle() {}"),
        ("src/layer.rs", "fn catalog_layer_needle() {}"),
        ("src/surface.rs", "fn catalog_surface_needle() {}"),
        ("src/snap.rs", "fn catalog_snapshot_needle() {}"),
        ("src/stale.rs", "fn catalog_stale_needle() {}"),
    ] {
        rt.ingest_text("repo-e2e", path, content)?;
    }
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
            E2eRuntimeFacetSpec {
                path: "src/owner.rs".to_string(),
                owner: Some("team-a".to_string()),
                service: Some("search".to_string()),
                layer: Some("index".to_string()),
                surface: Some("lexical".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/service.rs".to_string(),
                owner: Some("team-a".to_string()),
                service: Some("search".to_string()),
                layer: Some("index".to_string()),
                surface: Some("lexical".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/layer.rs".to_string(),
                owner: Some("team-a".to_string()),
                service: Some("search".to_string()),
                layer: Some("index".to_string()),
                surface: Some("lexical".to_string()),
            },
            E2eRuntimeFacetSpec {
                path: "src/surface.rs".to_string(),
                owner: Some("team-a".to_string()),
                service: Some("search".to_string()),
                layer: Some("index".to_string()),
                surface: Some("lexical".to_string()),
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
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn ingest_runtime_dirty_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    rt.ingest_text("repo-e2e", "src/dirty.rs", "todo dirty scope")?;
    rt.ingest_text("repo-e2e", "src/clean.rs", "todo clean scope")?;
    rt.ingest_dirty_for_path("src/dirty.rs", 100)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn query_runtime_catalog_changed_ids(rt: &mut E2eRuntime) -> AnyResult<Vec<String>> {
    let result = rt.query_runtime_metadata(
        TextQuerySyntax::Sourcegraph,
        "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs catalog_changed_needle",
        10,
    );
    require_no_typed_error(result.typed_error, "query_runtime_catalog_changed")?;
    Ok(result.candidate_ids)
}

fn require_structural_typed_error_code(
    rt: &mut E2eRuntime,
    syntax: TextQuerySyntax,
    query_text: &str,
    expected_code: &str,
) -> AnyResult<()> {
    let result = rt.query_structural(syntax, query_text, 10);
    let error = result
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed structural error for {query_text:?}"))?;
    if error.code.as_str() != expected_code {
        return Err(anyhow::anyhow!(
            "expected {expected_code} for structural query {query_text:?}, got {}",
            error.code
        ));
    }
    Ok(())
}

fn history_rev_delete_batch(rt: &E2eRuntime, file_path: &str) -> HistoryIngestBatch {
    use quanta_index_contract::lex::{CommitRecord, CommitSha};

    let commit_sha = CommitSha::from_bytes([
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ]);
    HistoryIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        manifest_digest: Some(format!(
            "history-delete:{}:{}",
            file_path,
            rt.current_generation().get()
        )),
        batch_digest: format!(
            "history-delete-batch:{file_path}:{}",
            rt.current_generation().get()
        ),
        commits: vec![CommitRecord {
            wire_version: 1,
            sha: commit_sha,
            parents: Vec::new(),
            author_time_ms: 11,
            committer_time_ms: 12,
            applied_at_ms: 13,
            author: "alice".to_string().into_boxed_str(),
            author_name: None,
            author_email: None,
            committer: "alice".to_string().into_boxed_str(),
            committer_name: None,
            committer_email: None,
            message: "fix: sample history".to_string().into_boxed_str(),
            is_merge: false,
            tags: vec!["v1.0.0".to_string().into_boxed_str()],
        }],
        refs: vec![
            HistoryRefMutation::Upsert(quanta_index_contract::HistoryRefUpsert {
                name: "refs/heads/main".to_string().into_boxed_str(),
                sha: commit_sha,
            }),
            HistoryRefMutation::Delete(HistoryRefDelete {
                name: "refs/heads/main".to_string().into_boxed_str(),
            }),
        ],
        tags: vec![
            HistoryRefMutation::Upsert(quanta_index_contract::HistoryRefUpsert {
                name: "v1.0.0".to_string().into_boxed_str(),
                sha: commit_sha,
            }),
            HistoryRefMutation::Delete(HistoryRefDelete {
                name: "v1.0.0".to_string().into_boxed_str(),
            }),
        ],
        diff_hunks: Vec::new(),
    }
}

#[test]
fn reopen_preserves_lexical_ids_and_explanation() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_fixture(&mut rt)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    let (before_ids, before_explanation) = query_ids_and_explanation(&mut rt)?;
    let mut rt = rt.reopen();
    let (after_ids, after_explanation) = query_ids_and_explanation(&mut rt)?;

    if before_ids != after_ids {
        return Err(anyhow::anyhow!(
            "reopen changed lexical ids: before={before_ids:?} after={after_ids:?}"
        ));
    }
    require_stamped(&before_explanation, "reopen before")?;
    require_stamped(&after_explanation, "reopen after")?;
    if deterministic_explanation(&before_explanation)?
        != deterministic_explanation(&after_explanation)?
    {
        return Err(anyhow::anyhow!(
            "reopen changed explanation: before={before_explanation:?} after={after_explanation:?}"
        ));
    }
    if !before_explanation.summary.contains("present") {
        return Err(anyhow::anyhow!(
            "expected explanation summary to contain `present`, got {}",
            before_explanation.summary
        ));
    }
    Ok(())
}

#[test]
fn fresh_reingest_replays_equivalent_lexical_ids_and_explanation() -> AnyResult<()> {
    let mut baseline = E2eRuntime::boot()?;
    ingest_fixture(&mut baseline)?;
    _ = baseline.seal()?;
    baseline.activate_last_sealed_generation()?;
    let baseline = query_ids_and_explanation(&mut baseline)?;

    let mut replay = E2eRuntime::boot()?;
    ingest_fixture(&mut replay)?;
    _ = replay.seal()?;
    replay.activate_last_sealed_generation()?;
    let replay = query_ids_and_explanation(&mut replay)?;

    require_stamped(&baseline.1, "fresh re-ingest baseline")?;
    require_stamped(&replay.1, "fresh re-ingest replay")?;
    if baseline.0 != replay.0
        || deterministic_explanation(&baseline.1)? != deterministic_explanation(&replay.1)?
    {
        return Err(anyhow::anyhow!(
            "fresh re-ingest diverged from baseline: baseline={baseline:?} replay={replay:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_semantic_scope_ids_and_explanation() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_dual_track_fixture(&mut rt)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    let (before_ids, before_explanation) = query_semantic_scope_ids_and_explanation(&mut rt)?;
    let mut rt = rt.reopen();
    let (after_ids, after_explanation) = query_semantic_scope_ids_and_explanation(&mut rt)?;

    if before_ids != after_ids {
        return Err(anyhow::anyhow!(
            "reopen changed semantic scoped ids: before={before_ids:?} after={after_ids:?}"
        ));
    }
    require_stamped(&before_explanation, "semantic reopen before")?;
    require_stamped(&after_explanation, "semantic reopen after")?;
    if deterministic_explanation(&before_explanation)?
        != deterministic_explanation(&after_explanation)?
    {
        return Err(anyhow::anyhow!(
            "reopen changed semantic scoped explanation: before={before_explanation:?} after={after_explanation:?}"
        ));
    }
    if before_explanation.engines_touched != vec![EngineTouched::Lexical, EngineTouched::Semantic] {
        return Err(anyhow::anyhow!(
            "semantic scoped explanation lost engines_touched truth: {:?}",
            before_explanation.engines_touched
        ));
    }
    if before_explanation.early_stop_reason != Some(EarlyStopReason::CountReached) {
        return Err(anyhow::anyhow!(
            "expected CountReached on semantic scoped reopen proof, got {:?}",
            before_explanation.early_stop_reason
        ));
    }
    Ok(())
}

#[test]
fn fresh_reingest_replays_equivalent_hybrid_ids_and_early_stop_truth() -> AnyResult<()> {
    let mut baseline = E2eRuntime::boot()?;
    ingest_dual_track_fixture(&mut baseline)?;
    _ = baseline.seal()?;
    baseline.activate_last_sealed_generation()?;
    let baseline = query_hybrid_ids_and_explanation(&mut baseline)?;

    let mut replay = E2eRuntime::boot()?;
    ingest_dual_track_fixture(&mut replay)?;
    _ = replay.seal()?;
    replay.activate_last_sealed_generation()?;
    let replay = query_hybrid_ids_and_explanation(&mut replay)?;

    if baseline.0 != replay.0 {
        return Err(anyhow::anyhow!(
            "fresh re-ingest diverged for hybrid replay proof: baseline={baseline:?} replay={replay:?}"
        ));
    }
    if baseline.1.strategy != replay.1.strategy
        || baseline.1.engines_touched != replay.1.engines_touched
    {
        return Err(anyhow::anyhow!(
            "hybrid replay lost high-level explanation truth: baseline={:?} replay={:?}",
            baseline.1,
            replay.1
        ));
    }
    if baseline.1.early_stop_reason != Some(EarlyStopReason::CountReached)
        || replay.1.early_stop_reason != Some(EarlyStopReason::CountReached)
    {
        return Err(anyhow::anyhow!(
            "expected CountReached on hybrid replay proof, got baseline={:?} replay={:?}",
            baseline.1.early_stop_reason,
            replay.1.early_stop_reason
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_structural_typed_hole_and_sourcegraph_boolean_ids() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_structural_fixture(&mut rt)?;

    let expected_id = rt.candidate_id_for_path("src/structural.rs")?;
    let before_typed = query_structural_ids(
        &mut rt,
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
    )?;
    let before_sourcegraph_boolean = query_structural_ids(
        &mut rt,
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural "function_item { { :[name.expr] } }" AND NOT "trait_item""#,
    )?;

    let mut rt = rt.reopen();
    let after_typed = query_structural_ids(
        &mut rt,
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
    )?;
    let after_sourcegraph_boolean = query_structural_ids(
        &mut rt,
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural "function_item { { :[name.expr] } }" AND NOT "trait_item""#,
    )?;

    if before_typed != vec![expected_id.clone()] || after_typed != vec![expected_id.clone()] {
        return Err(anyhow::anyhow!(
            "typed structural reopen proof drifted: before={before_typed:?} after={after_typed:?} expected={expected_id}"
        ));
    }
    if before_sourcegraph_boolean != vec![expected_id.clone()]
        || after_sourcegraph_boolean != vec![expected_id.clone()]
    {
        return Err(anyhow::anyhow!(
            "Sourcegraph structural boolean reopen proof drifted: before={before_sourcegraph_boolean:?} after={after_sourcegraph_boolean:?} expected={expected_id}"
        ));
    }
    Ok(())
}

#[test]
fn fresh_reingest_replays_equivalent_structural_native_and_sourcegraph_ids() -> AnyResult<()> {
    let mut baseline = E2eRuntime::boot()?;
    ingest_structural_fixture(&mut baseline)?;
    let baseline_native = query_structural_ids(
        &mut baseline,
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } } OR match { trait_item }",
    )?;
    let baseline_sourcegraph = query_structural_ids(
        &mut baseline,
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural "function_item { { :[name.expr] } }" OR patterntype:structural "trait_item""#,
    )?;

    let mut replay = E2eRuntime::boot()?;
    ingest_structural_fixture(&mut replay)?;
    let replay_native = query_structural_ids(
        &mut replay,
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } } OR match { trait_item }",
    )?;
    let replay_sourcegraph = query_structural_ids(
        &mut replay,
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural "function_item { { :[name.expr] } }" OR patterntype:structural "trait_item""#,
    )?;

    if baseline_native != replay_native {
        return Err(anyhow::anyhow!(
            "fresh re-ingest diverged for native structural replay proof: baseline={baseline_native:?} replay={replay_native:?}"
        ));
    }
    if baseline_sourcegraph != replay_sourcegraph {
        return Err(anyhow::anyhow!(
            "fresh re-ingest diverged for Sourcegraph structural replay proof: baseline={baseline_sourcegraph:?} replay={replay_sourcegraph:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_history_rev_delete_state_without_dropping_commit_matches() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    let path = "src/history.rs";
    rt.ingest_text("repo-e2e", path, "history lexical proof")?;
    rt.ingest_history_fixture(path)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    let before_rev_ref = query_history_commit_ids(&mut rt, "type:commit rev:refs/heads/main fix")?;
    let before_rev_tag = query_history_commit_ids(&mut rt, "type:commit rev:v1.0.0 fix")?;
    if before_rev_ref.len() != 1 || before_rev_tag.len() != 1 {
        return Err(anyhow::anyhow!(
            "history fixture failed to expose initial rev resolution: ref={before_rev_ref:?} tag={before_rev_tag:?}"
        ));
    }

    rt.publish_history_batch(history_rev_delete_batch(&rt, path))?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    let after_plain = query_history_commit_ids(&mut rt, "type:commit fix")?;
    let after_rev_ref = query_history_commit_ids(&mut rt, "type:commit rev:refs/heads/main fix")?;
    let after_rev_tag = query_history_commit_ids(&mut rt, "type:commit rev:v1.0.0 fix")?;
    if after_plain.len() != 1 || !after_rev_ref.is_empty() || !after_rev_tag.is_empty() {
        return Err(anyhow::anyhow!(
            "history delete-state drifted before reopen: plain={after_plain:?} ref={after_rev_ref:?} tag={after_rev_tag:?}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened_plain = query_history_commit_ids(&mut rt, "type:commit fix")?;
    let reopened_rev_ref =
        query_history_commit_ids(&mut rt, "type:commit rev:refs/heads/main fix")?;
    let reopened_rev_tag = query_history_commit_ids(&mut rt, "type:commit rev:v1.0.0 fix")?;
    if reopened_plain.len() != 1 || !reopened_rev_ref.is_empty() || !reopened_rev_tag.is_empty() {
        return Err(anyhow::anyhow!(
            "history delete-state changed after reopen: plain={reopened_plain:?} ref={reopened_rev_ref:?} tag={reopened_rev_tag:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_runtime_catalog_changed_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_runtime_catalog_fixture(&mut rt)?;

    let before = query_runtime_catalog_changed_ids(&mut rt)?;
    if before.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime catalog changed fixture failed before reopen: {before:?}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened = query_runtime_catalog_changed_ids(&mut rt)?;
    if before != reopened {
        return Err(anyhow::anyhow!(
            "runtime catalog changed query drifted after reopen: before={before:?} reopened={reopened:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_runtime_catalog_invalidated_by_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_runtime_catalog_fixture(&mut rt)?;

    let before = query_runtime_catalog_ids(
        &mut rt,
        "invalidated_by:rebuild=lexical catalog_changed_needle",
    )?;
    if before.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime catalog invalidated_by fixture failed before reopen: {before:?}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened = query_runtime_catalog_ids(
        &mut rt,
        "invalidated_by:rebuild=lexical catalog_changed_needle",
    )?;
    if before != reopened {
        return Err(anyhow::anyhow!(
            "runtime catalog invalidated_by query drifted after reopen: before={before:?} reopened={reopened:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_runtime_catalog_affected_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_runtime_catalog_fixture(&mut rt)?;

    let before =
        query_runtime_catalog_ids(&mut rt, "affected:rebuild=lexical catalog_changed_needle")?;
    if before.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime catalog affected fixture failed before reopen: {before:?}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened =
        query_runtime_catalog_ids(&mut rt, "affected:rebuild=lexical catalog_changed_needle")?;
    if before != reopened {
        return Err(anyhow::anyhow!(
            "runtime catalog affected query drifted after reopen: before={before:?} reopened={reopened:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_runtime_catalog_stale_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_runtime_catalog_fixture(&mut rt)?;

    let before = query_runtime_catalog_ids(
        &mut rt,
        "stale:before=1970-01-01T00:00:00.030Z file:src/stale.rs catalog_stale_needle",
    )?;
    if before.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime catalog stale fixture failed before reopen: {before:?}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened = query_runtime_catalog_ids(
        &mut rt,
        "stale:before=1970-01-01T00:00:00.030Z file:src/stale.rs catalog_stale_needle",
    )?;
    if before != reopened {
        return Err(anyhow::anyhow!(
            "runtime catalog stale query drifted after reopen: before={before:?} reopened={reopened:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_runtime_catalog_snapshot_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_runtime_catalog_fixture(&mut rt)?;

    let before = query_runtime_catalog_ids(
        &mut rt,
        "snapshot:active file:src/snap.rs catalog_snapshot_needle",
    )?;
    if before.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime catalog snapshot fixture failed before reopen: {before:?}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened = query_runtime_catalog_ids(
        &mut rt,
        "snapshot:active file:src/snap.rs catalog_snapshot_needle",
    )?;
    if before != reopened {
        return Err(anyhow::anyhow!(
            "runtime catalog snapshot query drifted after reopen: before={before:?} reopened={reopened:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_runtime_catalog_meta_owner_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_runtime_catalog_fixture(&mut rt)?;

    let before = query_runtime_catalog_ids(&mut rt, "meta.owner:team-a catalog_owner_needle")?;
    if before.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime catalog meta.owner fixture failed before reopen: {before:?}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened = query_runtime_catalog_ids(&mut rt, "meta.owner:team-a catalog_owner_needle")?;
    if before != reopened {
        return Err(anyhow::anyhow!(
            "runtime catalog meta.owner query drifted after reopen: before={before:?} reopened={reopened:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_runtime_catalog_meta_service_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_runtime_catalog_fixture(&mut rt)?;

    let before = query_runtime_catalog_ids(&mut rt, "meta.service:search catalog_service_needle")?;
    if before.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime catalog meta.service fixture failed before reopen: {before:?}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened =
        query_runtime_catalog_ids(&mut rt, "meta.service:search catalog_service_needle")?;
    if before != reopened {
        return Err(anyhow::anyhow!(
            "runtime catalog meta.service query drifted after reopen: before={before:?} reopened={reopened:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_runtime_catalog_meta_layer_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_runtime_catalog_fixture(&mut rt)?;

    let before = query_runtime_catalog_ids(&mut rt, "meta.layer:index catalog_layer_needle")?;
    if before.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime catalog meta.layer fixture failed before reopen: {before:?}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened = query_runtime_catalog_ids(&mut rt, "meta.layer:index catalog_layer_needle")?;
    if before != reopened {
        return Err(anyhow::anyhow!(
            "runtime catalog meta.layer query drifted after reopen: before={before:?} reopened={reopened:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_runtime_catalog_meta_surface_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_runtime_catalog_fixture(&mut rt)?;

    let before = query_runtime_catalog_ids(&mut rt, "meta.surface:lexical catalog_surface_needle")?;
    if before.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime catalog meta.surface fixture failed before reopen: {before:?}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened =
        query_runtime_catalog_ids(&mut rt, "meta.surface:lexical catalog_surface_needle")?;
    if before != reopened {
        return Err(anyhow::anyhow!(
            "runtime catalog meta.surface query drifted after reopen: before={before:?} reopened={reopened:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_runtime_dirty_no_clean_complement() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_runtime_dirty_fixture(&mut rt)?;

    let before = query_runtime_dirty_ids(&mut rt, "dirty:no clean")?;
    if before.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime dirty:no fixture failed before reopen: {before:?}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened = query_runtime_dirty_ids(&mut rt, "dirty:no clean")?;
    if before != reopened {
        return Err(anyhow::anyhow!(
            "runtime dirty:no query drifted after reopen: before={before:?} reopened={reopened:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_structural_mixed_lexical_boolean_ids() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_structural_fixture(&mut rt)?;

    let expected_id = rt.candidate_id_for_path("src/structural.rs")?;
    let before = query_structural_ids(
        &mut rt,
        TextQuerySyntax::Native,
        "restart_structural_alpha AND match { function_item }",
    )?;
    if before != vec![expected_id.clone()] {
        return Err(anyhow::anyhow!(
            "structural mixed boolean fixture failed before reopen: {before:?} expected={expected_id}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened = query_structural_ids(
        &mut rt,
        TextQuerySyntax::Native,
        "restart_structural_alpha AND match { function_item }",
    )?;
    if reopened != vec![expected_id] {
        return Err(anyhow::anyhow!(
            "structural mixed boolean query drifted after reopen: {reopened:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_runtime_dirty_evict_empty_state() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    let path = "src/dirty.rs";
    let content = "todo dirty scope";
    rt.ingest_text("repo-e2e", path, content)?;
    rt.ingest_dirty_for_path(path, 100)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    let before = query_runtime_dirty_ids(&mut rt, "dirty:yes todo")?;
    if before.len() != 1 {
        return Err(anyhow::anyhow!(
            "runtime dirty fixture failed before evict: {before:?}"
        ));
    }

    rt.ingest_text("repo-e2e", path, content)?;
    rt.ingest_dirty_for_path(path, 200)?;
    rt.evict_dirty_for_path(path)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    let after = query_runtime_dirty_ids(&mut rt, "dirty:yes todo")?;
    if !after.is_empty() {
        return Err(anyhow::anyhow!(
            "runtime dirty evict did not clear candidates before reopen: {after:?}"
        ));
    }

    let mut rt = rt.reopen();
    let reopened = query_runtime_dirty_ids(&mut rt, "dirty:yes todo")?;
    if !reopened.is_empty() {
        return Err(anyhow::anyhow!(
            "runtime dirty evict state changed after reopen: {reopened:?}"
        ));
    }
    Ok(())
}

#[test]
fn reopen_preserves_structural_tombstone_not_ready_state() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    let path = "src/structural.rs";
    let content = "fn restart_structural_tombstone() {}";
    rt.ingest_text("repo-e2e", path, content).map_err(|error| {
        anyhow::anyhow!("structural tombstone generation 1 lexical ingest: {error}")
    })?;
    rt.ingest_structural_function_tree(path, content, "restart_structural_tombstone")
        .map_err(|error| {
            anyhow::anyhow!("structural tombstone generation 1 structural ingest: {error}")
        })?;
    _ = rt
        .seal_lexical_generation_for_tracks(&[
            SearchPlaneTrackKind::Lexical,
            SearchPlaneTrackKind::Structural,
        ])
        .map_err(|error| anyhow::anyhow!("structural tombstone generation 1 seal: {error}"))?;
    rt.activate_last_sealed_generation()?;

    let before = query_structural_ids(
        &mut rt,
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
    )?;
    if before.len() != 1 {
        return Err(anyhow::anyhow!(
            "structural tombstone fixture failed before tombstone: {before:?}"
        ));
    }

    rt.ingest_text("repo-e2e", path, content).map_err(|error| {
        anyhow::anyhow!("structural tombstone generation 2 lexical ingest: {error}")
    })?;
    rt.ingest_structural_function_tree(path, content, "restart_structural_tombstone")
        .map_err(|error| {
            anyhow::anyhow!("structural tombstone generation 2 structural ingest: {error}")
        })?;
    rt.tombstone_structural_for_path(path).map_err(|error| {
        anyhow::anyhow!("structural tombstone generation 2 structural tombstone: {error}")
    })?;
    _ = rt
        .seal_lexical_generation_for_tracks(&[
            SearchPlaneTrackKind::Lexical,
            SearchPlaneTrackKind::Structural,
        ])
        .map_err(|error| anyhow::anyhow!("structural tombstone generation 2 seal: {error}"))?;

    require_structural_typed_error_code(
        &mut rt,
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
        "STR_GENERATION_NOT_READY",
    )?;

    let mut rt = rt.reopen();
    require_structural_typed_error_code(
        &mut rt,
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
        "STR_GENERATION_NOT_READY",
    )?;
    Ok(())
}
