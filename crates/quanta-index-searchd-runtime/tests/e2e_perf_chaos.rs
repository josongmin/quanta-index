//! E2E-07 — boundedness / fail-closed / observability owner rail.
//!
//! This is not a stopwatch benchmark. The rail exercises the hard runtime
//! shapes that must stay bounded and typed:
//! - regex false positives must be rejected by exact verify,
//! - unsupported regex syntax must fail typed and not poison the next query,
//! - hybrid over-fetch / fuse paths must surface a truthful early-stop reason.

#![forbid(unsafe_code)]

#[path = "common/e2e_harness.rs"]
mod e2e_harness;

use anyhow::Result as AnyResult;
use quanta_index_contract::{
    EarlyStopReason, EngineTouched, HistoryIngestBatch, HistoryRefMutation, SearchPlaneTrackKind,
    TextQuerySyntax,
};

use crate::e2e_harness::{E2eRuntime, E2eTypedError};

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

fn assert_closed_metric_suffix(
    rt: &E2eRuntime,
    expected_suffix: &[&str],
    allowed: &[&str],
    leaked_terms: &[&str],
) -> AnyResult<()> {
    let errors = rt.query_metric_errors()?;
    if !errors.is_empty() {
        return Err(anyhow::anyhow!(
            "unexpected runtime metric errors: {errors:?}"
        ));
    }
    let samples = rt.query_metrics_snapshot()?;
    let names = samples
        .iter()
        .map(|sample| sample.name.as_ref().to_string())
        .collect::<Vec<_>>();
    if names.iter().any(|name| !allowed.contains(&name.as_str())) {
        return Err(anyhow::anyhow!(
            "runtime metric names escaped closed set: {names:?}"
        ));
    }
    let suffix = names
        .get(names.len().saturating_sub(expected_suffix.len())..)
        .unwrap_or_default()
        .to_vec();
    let expected = expected_suffix
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    if suffix != expected {
        return Err(anyhow::anyhow!(
            "unexpected runtime metric suffix: names={names:?} expected_suffix={expected:?}"
        ));
    }
    for sample in &samples {
        if sample.dimensions.ticket_id.as_ref() != "LXE-10"
            || sample.dimensions.wave_id.as_ref() != "8"
            || sample.dimensions.tenant_id.as_ref() != "local"
            || sample.dimensions.repo_id.as_ref() != "repo-e2e"
            || sample.dimensions.generation_id != 1
        {
            return Err(anyhow::anyhow!(
                "unexpected runtime metric dimensions: {:?}",
                sample.dimensions
            ));
        }
        for leaked in leaked_terms {
            if sample.name.contains(leaked) {
                return Err(anyhow::anyhow!(
                    "runtime metric leaked query content `{leaked}` in name={}",
                    sample.name
                ));
            }
        }
    }
    Ok(())
}

fn seed_regex_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    rt.ingest_text("repo-e2e", "src/exact.txt", "needle_x exact")?;
    rt.ingest_text("repo-e2e", "src/bait.txt", "needle_xx bait")?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn seed_hybrid_count_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    rt.ingest_text("repo-e2e", "src/alpha.rs", "scope alpha keep")?;
    rt.ingest_text("repo-e2e", "src/beta.rs", "scope beta keep")?;
    rt.ingest_text("repo-e2e", "src/gamma.rs", "scope gamma keep")?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;
    Ok(())
}

fn seed_hybrid_tie_fixture(rt: &mut E2eRuntime, count: usize) -> AnyResult<()> {
    for idx in 0..count {
        let path = format!("src/tie-{idx:02}.rs");
        rt.ingest_text("repo-e2e", &path, "scope tie keep")?;
    }
    _ = rt.seal()?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;
    Ok(())
}

fn seed_structural_boolean_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    let path = "src/structural.rs";
    let content = "fn chaos_structural_alpha() {}";
    rt.ingest_text("repo-e2e", path, content)?;
    rt.ingest_structural_function_tree(path, content, "chaos_structural_alpha")?;
    _ = rt.seal_lexical_generation_for_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;
    Ok(())
}

fn seed_history_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    let path = "src/history.rs";
    rt.ingest_text("repo-e2e", path, "history lexical proof")?;
    rt.ingest_history_fixture(path)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn history_partial_shard_batch(
    rt: &E2eRuntime,
    file_path: &str,
    include_ref: bool,
    include_tag: bool,
) -> HistoryIngestBatch {
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
            "history-partial:{}:{}",
            file_path,
            rt.current_generation().get()
        )),
        batch_digest: format!(
            "history-partial-batch:{file_path}:{}",
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
        refs: if include_ref {
            vec![HistoryRefMutation::Upsert(
                quanta_index_contract::HistoryRefUpsert {
                    name: "refs/heads/main".to_string().into_boxed_str(),
                    sha: commit_sha,
                },
            )]
        } else {
            Vec::new()
        },
        tags: if include_tag {
            vec![HistoryRefMutation::Upsert(
                quanta_index_contract::HistoryRefUpsert {
                    name: "v1.0.0".to_string().into_boxed_str(),
                    sha: commit_sha,
                },
            )]
        } else {
            Vec::new()
        },
        diff_hunks: Vec::new(),
    }
}

fn seed_history_partial_shard_fixture(
    rt: &mut E2eRuntime,
    include_ref: bool,
    include_tag: bool,
) -> AnyResult<()> {
    let path = "src/history.rs";
    rt.ingest_text("repo-e2e", path, "history lexical proof")?;
    rt.publish_history_batch(history_partial_shard_batch(
        rt,
        path,
        include_ref,
        include_tag,
    ))?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn seed_runtime_dirty_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    let path = "src/dirty.rs";
    rt.ingest_text("repo-e2e", path, "todo dirty scope")?;
    rt.ingest_dirty_for_path(path, 100)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn oversized_raw_substring_query() -> String {
    "abcdefghijklmnopq".repeat(1024)
}

fn trigram_limit_raw_substring_query() -> String {
    trigram_plan_limit_raw_substring_query()
}

fn trigram_plan_limit_raw_substring_query() -> String {
    let alphabet = *b"abcdefghijklmnopq";
    let mut out = String::new();
    for a in alphabet {
        for b in alphabet {
            for c in alphabet {
                out.push(char::from(a));
                out.push(char::from(b));
                out.push(char::from(c));
            }
        }
    }
    out
}

#[test]
fn regex_false_positive_candidate_is_rejected_by_exact_verify() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let bait_id = rt.candidate_id_for_path("src/bait.txt")?;

    let result = rt.query_text(TextQuerySyntax::Native, "/needle_x\\b/", 10);
    require_no_typed_error(result.typed_error, "regex exact-verify query")?;
    if result.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "expected only exact regex hit, got {:?}",
            result.candidate_ids
        ));
    }
    if result.candidate_ids.contains(&bait_id) {
        return Err(anyhow::anyhow!(
            "regex verify leaked trigram false positive {bait_id}"
        ));
    }
    Ok(())
}

#[test]
fn regex_typed_rejection_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let invalid = rt.query_text(TextQuerySyntax::Native, "/(?<=needle_)x/", 10);
    let error = invalid
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed regex rejection"))?;
    if error.code != "PARSE_FAIL" {
        return Err(anyhow::anyhow!("expected PARSE_FAIL, got {}", error.code));
    }
    if !error.message.contains("regex") {
        return Err(anyhow::anyhow!(
            "regex rejection lost fail-closed regex detail: {}",
            error.message
        ));
    }

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let follow_up = rt.query_text(TextQuerySyntax::Native, "needle_x", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after regex reject",
    )?;
    if follow_up.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after regex reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn regex_timeout_is_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let timed_out = rt.query_text(TextQuerySyntax::Native, "timeout:0ms /needle_x\\b/", 10);
    let error = timed_out
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed regex timeout"))?;
    if error.code != "QUERY_TIMEOUT" {
        return Err(anyhow::anyhow!(
            "expected QUERY_TIMEOUT, got {}",
            error.code
        ));
    }
    if !error.message.contains("timed out") {
        return Err(anyhow::anyhow!(
            "regex timeout lost timeout detail: {}",
            error.message
        ));
    }

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let follow_up = rt.query_text(TextQuerySyntax::Native, "needle_x", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after regex timeout",
    )?;
    if follow_up.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after regex timeout: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn oversized_raw_substring_query_fails_parse_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let query = format!("'{}'", oversized_raw_substring_query());
    let limited = rt.query_text(TextQuerySyntax::Native, &query, 10);
    let error = limited.typed_error.ok_or_else(|| {
        anyhow::anyhow!(
            "expected oversized raw-substring parse rejection, observed success ids={:?}",
            limited.candidate_ids
        )
    })?;
    if error.code != "PARSE_FAIL" {
        return Err(anyhow::anyhow!("expected PARSE_FAIL, got {}", error.code));
    }
    if !error.message.contains("16 KiB cap") {
        return Err(anyhow::anyhow!(
            "oversized raw-substring rejection lost parser byte-cap detail: {}",
            error.message
        ));
    }

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let follow_up = rt.query_text(TextQuerySyntax::Native, "needle_x", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after oversized raw-substring reject",
    )?;
    if follow_up.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after oversized raw-substring reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn raw_substring_trigram_plan_limit_is_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let query = format!("'{}'", trigram_plan_limit_raw_substring_query());
    let limited = rt.query_text(TextQuerySyntax::Native, &query, 10);
    let error = limited.typed_error.ok_or_else(|| {
        anyhow::anyhow!(
            "expected trigram plan-limit rejection, observed success ids={:?}",
            limited.candidate_ids
        )
    })?;
    if error.code != "LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED" {
        return Err(anyhow::anyhow!(
            "expected LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED, got {}",
            error.code
        ));
    }
    if !error.message.contains("distinct trigrams") {
        return Err(anyhow::anyhow!(
            "trigram plan-limit rejection lost distinct-trigram detail: {}",
            error.message
        ));
    }

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let follow_up = rt.query_text(TextQuerySyntax::Native, "needle_x", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after trigram plan-limit reject",
    )?;
    if follow_up.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after trigram plan-limit reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn trigram_plan_limit_query_fails_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let query = format!("'{}'", trigram_limit_raw_substring_query());
    let limited = rt.query_text(TextQuerySyntax::Native, &query, 10);
    let error = limited.typed_error.ok_or_else(|| {
        anyhow::anyhow!(
            "expected trigram plan-limit rejection, observed success ids={:?}",
            limited.candidate_ids
        )
    })?;
    if error.code != "LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED" {
        return Err(anyhow::anyhow!(
            "expected LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED, got {}",
            error.code
        ));
    }
    if !error.message.contains("dimension=trigram-set") {
        return Err(anyhow::anyhow!(
            "trigram plan-limit rejection lost dimension detail: {}",
            error.message
        ));
    }

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let follow_up = rt.query_text(TextQuerySyntax::Native, "needle_x", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after trigram plan-limit reject",
    )?;
    if follow_up.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after trigram plan-limit reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn hybrid_count_cap_surfaces_truthful_early_stop_reason() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_hybrid_count_fixture(&mut rt)?;

    let result = rt.query_hybrid(TextQuerySyntax::Native, "scope", "scope", 2);
    require_no_typed_error(result.typed_error, "hybrid count-cap query")?;
    if result.candidate_ids.len() != 2 {
        return Err(anyhow::anyhow!(
            "expected exactly 2 hybrid results after top_k cap, got {:?}",
            result.candidate_ids
        ));
    }
    let explanation = result
        .explanation
        .ok_or_else(|| anyhow::anyhow!("hybrid count-cap query returned no explanation"))?;
    if explanation.early_stop_reason != Some(EarlyStopReason::CountReached) {
        return Err(anyhow::anyhow!(
            "expected CountReached, got {:?}",
            explanation.early_stop_reason
        ));
    }
    if explanation.engines_touched != vec![EngineTouched::Lexical, EngineTouched::Semantic] {
        return Err(anyhow::anyhow!(
            "unexpected hybrid count-cap engines_touched: {:?}",
            explanation.engines_touched
        ));
    }
    if explanation.summary != "hybrid fused 3 lexical and 3 semantic candidates into 2 results" {
        return Err(anyhow::anyhow!(
            "hybrid count-cap summary missing fused-count detail: {}",
            explanation.summary
        ));
    }
    Ok(())
}

#[test]
fn hybrid_runtime_metrics_use_closed_labels_without_query_leakage() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_hybrid_count_fixture(&mut rt)?;

    let result = rt.query_hybrid(TextQuerySyntax::Native, "scope", "scope", 2);
    require_no_typed_error(result.typed_error, "hybrid runtime metrics query")?;

    let errors = rt.query_metric_errors()?;
    if !errors.is_empty() {
        return Err(anyhow::anyhow!(
            "unexpected runtime metric errors: {errors:?}"
        ));
    }
    let samples = rt.query_metrics_snapshot()?;
    let names = samples
        .iter()
        .map(|sample| sample.name.as_ref().to_string())
        .collect::<Vec<_>>();
    let expected_success_suffix = vec![
        "lq_query_intake_total".to_string(),
        "lq_planner_total".to_string(),
        "lq_engine_fanout_count".to_string(),
        "lq_merge_result_count".to_string(),
        "lq_early_stop_total".to_string(),
    ];
    let allowed = [
        "lq_query_intake_total",
        "lq_typed_error_not_ready_total",
        "lq_planner_total",
        "lq_engine_fanout_count",
        "lq_merge_result_count",
        "lq_early_stop_total",
    ];
    if names.iter().any(|name| !allowed.contains(&name.as_str())) {
        return Err(anyhow::anyhow!(
            "runtime metric names escaped closed set: {names:?}"
        ));
    }
    let success_suffix = names
        .get(names.len().saturating_sub(expected_success_suffix.len())..)
        .unwrap_or_default()
        .to_vec();
    if success_suffix != expected_success_suffix {
        return Err(anyhow::anyhow!(
            "unexpected runtime metric names: {names:?}"
        ));
    }
    for sample in &samples {
        if sample.dimensions.ticket_id.as_ref() != "LXE-10"
            || sample.dimensions.wave_id.as_ref() != "8"
            || sample.dimensions.tenant_id.as_ref() != "local"
            || sample.dimensions.repo_id.as_ref() != "repo-e2e"
            || sample.dimensions.generation_id != 1
        {
            return Err(anyhow::anyhow!(
                "unexpected runtime metric dimensions: {:?}",
                sample.dimensions
            ));
        }
        if sample.name.contains("scope") || sample.name.contains("1.0 0.0") {
            return Err(anyhow::anyhow!(
                "runtime metric leaked query content in name={}",
                sample.name
            ));
        }
    }
    Ok(())
}

#[test]
fn history_runtime_metrics_use_closed_labels_without_query_leakage() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_fixture(&mut rt)?;

    let result = rt.query_history(TextQuerySyntax::Sourcegraph, "type:commit fix", 10);
    require_no_typed_error(result.typed_error, "history metrics query")?;
    if result.commit_ids.len() != 1 || !result.diff_paths.is_empty() {
        return Err(anyhow::anyhow!(
            "unexpected history query result commit_ids={:?} diff_paths={:?}",
            result.commit_ids,
            result.diff_paths
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["fix", "history", "alice"],
    )
}

#[test]
fn runtime_metadata_metrics_use_closed_labels_without_query_leakage() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_runtime_dirty_fixture(&mut rt)?;

    let result = rt.query_runtime_metadata(TextQuerySyntax::Native, "dirty:yes todo", 10);
    require_no_typed_error(result.typed_error, "runtime metadata metrics query")?;
    if result.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "unexpected runtime metadata candidate ids: {:?}",
            result.candidate_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["dirty", "todo", "src/dirty.rs"],
    )
}

#[test]
fn structural_runtime_metrics_use_closed_labels_without_query_leakage() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_structural_boolean_fixture(&mut rt)?;

    let result = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
        10,
    );
    require_no_typed_error(result.typed_error, "structural metrics query")?;
    if result.candidate_ids.len() != 1 {
        return Err(anyhow::anyhow!(
            "unexpected structural candidate ids: {:?}",
            result.candidate_ids
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &[
            "lq_query_intake_total",
            "lq_typed_error_not_ready_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
        ],
        &["function_item", "name.expr", "chaos_structural_alpha"],
    )
}

#[test]
fn lexical_timeout_runtime_metrics_use_plan_limit_bucket_without_query_leakage() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let result = rt.query_text(TextQuerySyntax::Native, "timeout:0ms /needle_x\\b/", 10);
    let error = result
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected lexical timeout typed error"))?;
    if error.code != "QUERY_TIMEOUT" {
        return Err(anyhow::anyhow!(
            "expected QUERY_TIMEOUT, got {}",
            error.code
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &["lq_query_intake_total", "lq_typed_error_plan_limit_total"],
        &["lq_query_intake_total", "lq_typed_error_plan_limit_total"],
        &["needle_x", "timeout:0ms", "regex"],
    )
}

#[test]
fn history_missing_ref_shard_uses_closed_unavailable_metric() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_partial_shard_fixture(&mut rt, false, false)?;

    let result = rt.query_history(
        TextQuerySyntax::Sourcegraph,
        "type:commit rev:refs/heads/main fix",
        10,
    );
    let error = result
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected history ref-shard typed error"))?;
    if error.code != "HISTORY_SHARD_UNAVAILABLE" {
        return Err(anyhow::anyhow!(
            "expected HISTORY_SHARD_UNAVAILABLE, got {}",
            error.code
        ));
    }
    if !error.message.contains("ref shard is unavailable") {
        return Err(anyhow::anyhow!(
            "unexpected history ref-shard message: {}",
            error.message
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        &["refs/heads/main", "fix", "history"],
    )
}

#[test]
fn history_missing_tag_shard_uses_closed_unavailable_metric() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_history_partial_shard_fixture(&mut rt, true, false)?;

    let result = rt.query_history(
        TextQuerySyntax::Sourcegraph,
        "type:commit rev:v1.0.0 fix",
        10,
    );
    let error = result
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected history tag-shard typed error"))?;
    if error.code != "HISTORY_SHARD_UNAVAILABLE" {
        return Err(anyhow::anyhow!(
            "expected HISTORY_SHARD_UNAVAILABLE, got {}",
            error.code
        ));
    }
    if !error.message.contains("tag shard is unavailable") {
        return Err(anyhow::anyhow!(
            "unexpected history tag-shard message: {}",
            error.message
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        &["v1.0.0", "fix", "history"],
    )
}

#[test]
fn hybrid_large_tied_result_set_keeps_order_stable() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_hybrid_tie_fixture(&mut rt, 24)?;
    let mut baseline: Option<Vec<String>> = None;

    for run in 0..5 {
        let result = rt.query_hybrid(TextQuerySyntax::Native, "scope", "scope", 10);
        require_no_typed_error(result.typed_error, "hybrid tied-order query")?;
        if result.candidate_ids.len() != 10 {
            return Err(anyhow::anyhow!(
                "hybrid tied-order query returned {} results on run {run}, expected 10",
                result.candidate_ids.len()
            ));
        }
        let explanation = result
            .explanation
            .ok_or_else(|| anyhow::anyhow!("hybrid tied-order query returned no explanation"))?;
        if explanation.early_stop_reason != Some(EarlyStopReason::CountReached) {
            return Err(anyhow::anyhow!(
                "expected CountReached on hybrid tied-order query, got {:?}",
                explanation.early_stop_reason
            ));
        }
        match baseline.as_ref() {
            Some(previous) if previous != &result.candidate_ids => {
                return Err(anyhow::anyhow!(
                    "hybrid tied-order query drifted across runs: baseline={previous:?} current={:?}",
                    result.candidate_ids
                ));
            }
            None => baseline = Some(result.candidate_ids),
            Some(_) => {}
        }
    }
    Ok(())
}

#[test]
fn structural_missing_parse_tree_fails_typed_generation_not_ready() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo-e2e", "src/tree.rs", "fn orphaned() {}")?;
    _ = rt.seal_lexical_generation_for_tracks(&[SearchPlaneTrackKind::Lexical])?;
    rt.activate_last_sealed_generation_with_tracks(&[SearchPlaneTrackKind::Lexical])?;

    let result = rt.query_structural(TextQuerySyntax::Native, "match { :[x] }", 10);
    let error = result
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed structural readiness error"))?;
    if error.code != "STR_GENERATION_NOT_READY" {
        return Err(anyhow::anyhow!(
            "expected STR_GENERATION_NOT_READY, got {}",
            error.code
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &["lq_query_intake_total", "lq_typed_error_not_ready_total"],
        &["lq_query_intake_total", "lq_typed_error_not_ready_total"],
        &["match", "tree.rs"],
    )
}

#[test]
fn structural_orphan_chunk_authority_fails_typed_shard_unavailable() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    let path = "src/tree.rs";
    let content = "fn orphaned() {}";
    rt.ingest_text("repo-e2e", path, content)?;
    rt.ingest_structural_function_tree(path, content, "orphaned")?;
    rt.delete_chunk_for_path(path)?;
    _ = rt.seal_lexical_generation_for_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Structural,
    ])?;

    let result = rt.query_structural(TextQuerySyntax::Native, "match { :[x] }", 10);
    let error = result
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed structural shard-unavailable error"))?;
    if error.code != "STR_SHARD_UNAVAILABLE" {
        return Err(anyhow::anyhow!(
            "expected STR_SHARD_UNAVAILABLE, got {}",
            error.code
        ));
    }
    assert_closed_metric_suffix(
        &rt,
        &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        &["match", "tree.rs"],
    )
}

#[test]
fn structural_mixed_lexical_boolean_rejects_typed_and_does_not_poison_next_query() -> AnyResult<()>
{
    let mut rt = E2eRuntime::boot()?;
    seed_structural_boolean_fixture(&mut rt)?;

    let invalid = rt.query_structural(
        TextQuerySyntax::Native,
        "chaos_structural_alpha AND match { function_item }",
        10,
    );
    let error = invalid
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed mixed lexical/structural rejection"))?;
    if error.code != "STR_INVALID_REQUEST" {
        return Err(anyhow::anyhow!(
            "expected STR_INVALID_REQUEST, got {}",
            error.code
        ));
    }
    if !error
        .message
        .contains("structural-only boolean tree of `match { ... }` leaves")
    {
        return Err(anyhow::anyhow!(
            "mixed lexical/structural rejection lost exact detail: {}",
            error.message
        ));
    }

    let expected_id = rt.candidate_id_for_path("src/structural.rs")?;
    let follow_up = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up structural query after mixed lexical/structural reject",
    )?;
    if follow_up.candidate_ids != vec![expected_id] {
        return Err(anyhow::anyhow!(
            "follow-up structural query diverged after mixed lexical/structural reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn structural_pure_negative_boolean_rejects_typed_and_does_not_poison_next_query() -> AnyResult<()>
{
    let mut rt = E2eRuntime::boot()?;
    seed_structural_boolean_fixture(&mut rt)?;

    let invalid = rt.query_structural(TextQuerySyntax::Native, "NOT match { function_item }", 10);
    let error = invalid
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed pure-negative structural rejection"))?;
    if error.code != "STR_INVALID_REQUEST" {
        return Err(anyhow::anyhow!(
            "expected STR_INVALID_REQUEST, got {}",
            error.code
        ));
    }
    if !error
        .message
        .contains("pure-negative structural boolean queries are not executable")
    {
        return Err(anyhow::anyhow!(
            "pure-negative structural rejection lost exact detail: {}",
            error.message
        ));
    }

    let expected_id = rt.candidate_id_for_path("src/structural.rs")?;
    let follow_up = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { :[name.expr] } } }",
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up structural query after pure-negative reject",
    )?;
    if follow_up.candidate_ids != vec![expected_id] {
        return Err(anyhow::anyhow!(
            "follow-up structural query diverged after pure-negative reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn structural_typed_hole_kind_rejects_typed_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_structural_boolean_fixture(&mut rt)?;

    let invalid = rt.query_structural(
        TextQuerySyntax::Native,
        "match { function_item { { :[name.lambda] } } }",
        10,
    );
    let error = invalid
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed structural typed-hole rejection"))?;
    if error.code != "STR_HOLE_KIND_UNSUPPORTED" {
        return Err(anyhow::anyhow!(
            "expected STR_HOLE_KIND_UNSUPPORTED, got {}",
            error.code
        ));
    }
    if !error.message.contains("typed hole kind `lambda`") {
        return Err(anyhow::anyhow!(
            "typed-hole rejection lost exact unsupported-kind detail: {}",
            error.message
        ));
    }

    let expected_id = rt.candidate_id_for_path("src/structural.rs")?;
    let follow_up = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural "function_item { { :[name.expr] } }""#,
        10,
    );
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up structural query after typed-hole reject",
    )?;
    if follow_up.candidate_ids != vec![expected_id] {
        return Err(anyhow::anyhow!(
            "follow-up structural query diverged after typed-hole reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}
