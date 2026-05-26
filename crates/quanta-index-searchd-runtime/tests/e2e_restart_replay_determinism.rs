//! E2E-05 — restart/replay determinism over persisted lexical state.
//!
//! This rail proves that the public lexical query path and the public
//! `Explain` response remain stable across:
//! - runtime reopen against the same persisted state root, and
//! - a fresh re-ingest of the same fixture corpus into a new state root.

#![forbid(unsafe_code)]

#[path = "common/e2e_harness.rs"]
mod e2e_harness;

use anyhow::Result as AnyResult;
use quanta_index_contract::{
    EarlyStopReason, EngineTouched, HistoryIngestBatch, HistoryRefDelete, HistoryRefMutation,
    SearchExplanation, SearchPlaneTrackKind, TextQuerySyntax,
};

use crate::e2e_harness::{E2eRuntime, E2eTypedError};

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
    Ok((result.candidate_ids, explanation))
}

fn ingest_structural_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    let path = "src/structural.rs";
    let content = "fn restart_structural_alpha() {}";
    rt.ingest_text("repo-e2e", path, content)?;
    rt.ingest_structural_function_tree(path, content, "restart_structural_alpha")?;
    _ = rt.seal_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;
    rt.activate_last_sealed_generation_with_tracks(&[
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
    if error.code != expected_code {
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
            committer: "alice".to_string().into_boxed_str(),
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
    if before_explanation != after_explanation {
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

    if baseline != replay {
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
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;

    let (before_ids, before_explanation) = query_semantic_scope_ids_and_explanation(&mut rt)?;
    let mut rt = rt.reopen();
    let (after_ids, after_explanation) = query_semantic_scope_ids_and_explanation(&mut rt)?;

    if before_ids != after_ids {
        return Err(anyhow::anyhow!(
            "reopen changed semantic scoped ids: before={before_ids:?} after={after_ids:?}"
        ));
    }
    if before_explanation != after_explanation {
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
    baseline.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;
    let baseline = query_hybrid_ids_and_explanation(&mut baseline)?;

    let mut replay = E2eRuntime::boot()?;
    ingest_dual_track_fixture(&mut replay)?;
    _ = replay.seal()?;
    replay.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;
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
    rt.ingest_text("repo-e2e", path, content)?;
    rt.ingest_structural_function_tree(path, content, "restart_structural_tombstone")?;
    _ = rt.seal_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;

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

    rt.ingest_text("repo-e2e", path, content)?;
    rt.ingest_structural_function_tree(path, content, "restart_structural_tombstone")?;
    rt.tombstone_structural_for_path(path)?;
    _ = rt.seal_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;

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
