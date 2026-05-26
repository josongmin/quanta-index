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
use quanta_index_contract::{SearchExplanation, TextQuerySyntax};

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

#[test]
fn reopen_preserves_lexical_ids_and_explanation() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_fixture(&mut rt)?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    let (before_ids, before_explanation) = query_ids_and_explanation(&mut rt)?;
    let mut rt = rt.reopen()?;
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
