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
    EarlyStopReason, EngineTouched, SearchPlaneTrackKind, TextQuerySyntax,
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
    _ = rt.seal_tracks(&[SearchPlaneTrackKind::Lexical])?;
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
    Ok(())
}

#[test]
fn structural_orphan_chunk_authority_fails_typed_shard_unavailable() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    let path = "src/tree.rs";
    let content = "fn orphaned() {}";
    rt.ingest_text("repo-e2e", path, content)?;
    rt.ingest_structural_function_tree(path, content, "orphaned")?;
    rt.delete_chunk_for_path(path)?;
    _ = rt.seal_tracks(&[
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
    Ok(())
}
