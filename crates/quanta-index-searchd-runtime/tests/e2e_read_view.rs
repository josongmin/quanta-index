//! Plan §5.6 / §7.1 — one read view per request, declared per route and
//! plan, refused typed per missing domain before any lane runs.
//!
//! A generation sealed with its lexical and semantic tracks but no history
//! authority, no commit-recency authority and no runtime authority serves
//! every plan that reads only the tracks, and refuses — with the missing
//! domain's own code, never with an empty page — every plan that reads
//! one of the absent domains. The routes that carry a `SearchExplanation`
//! name the view's domains, epochs and pinned artifacts at the head of
//! their planner trace.
//!
//! The oracle is the fixture: what was ingested is exactly what the
//! daemon may claim to have read.

#![forbid(unsafe_code)]

use std::error::Error;

use crate::e2e_harness;
use quanta_index_contract::{PlannerStage, SearchPlaneErrorCodeV2, TextQuerySyntax};
use quanta_index_core::{
    REPO_COMMIT_RECENCY_UNAVAILABLE_CODE, RUNTIME_NOT_READY_CODE, RepoMetadataAuthorityV1,
};

use e2e_harness::{E2eErrorCode, E2eHistoryFixtureSpec, E2eQueryResult, E2eRuntime};

type TestResult = Result<(), Box<dyn Error>>;

const QUERY: &str = "needle";
const TOP_K: u32 = 10;
/// The history domain's code for a generation whose lexical track is
/// materialized but whose producer never published history.
const HISTORY_PRODUCER_UNAVAILABLE: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::HistoryProducerUnavailable;

fn seeded_runtime() -> Result<E2eRuntime, Box<dyn Error>> {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo", "src/needle.rs", "fn needle() { let needle = 1; }")?;
    rt.ingest_text("repo", "src/other.rs", "fn other() { let needle = 2; }")?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

fn served(result: &E2eQueryResult, what: &str) -> TestResult {
    if let Some(error) = &result.typed_error {
        return Err(format!("{what}: refused: {error}").into());
    }
    Ok(())
}

fn refused_with(result: &E2eQueryResult, what: &str, code: SearchPlaneErrorCodeV2) -> TestResult {
    match &result.typed_error {
        Some(error) if error.code == E2eErrorCode::Remote(code) => Ok(()),
        Some(error) => Err(format!("{what}: expected {code}, got {error}").into()),
        None => {
            Err(format!("{what}: expected {code}, got {} rows", result.candidate_ids.len()).into())
        }
    }
}

/// The plan-stage trace details of a served explanation.
fn plan_trace(result: &E2eQueryResult) -> Result<Vec<String>, Box<dyn Error>> {
    let explanation = result
        .explanation
        .as_ref()
        .ok_or("a served query carries an explanation")?;
    Ok(explanation
        .planner_trace
        .iter()
        .filter(|entry| entry.stage == PlannerStage::Plan)
        .map(|entry| entry.detail.clone())
        .collect())
}

fn expect_trace_head(trace: &[String], domains: &str, pin_detail: &str, what: &str) -> TestResult {
    let head: Vec<&str> = trace.iter().take(3).map(String::as_str).collect();
    let expected = [
        format!("read_view.domains={domains}"),
        "read_view.epochs=-".to_string(),
        format!("read_view.pin={pin_detail}"),
    ];
    if head != expected.iter().map(String::as_str).collect::<Vec<_>>() {
        return Err(format!("{what}: the trace opens with the view: {trace:?}").into());
    }
    Ok(())
}

fn pin_detail(rt: &E2eRuntime) -> String {
    let pin = rt.generation_pin();
    format!(
        "{}@{}#{}",
        pin.repo_id.as_str(),
        pin.revision_id.as_str(),
        rt.current_generation().get().saturating_sub(1)
    )
}

/// A generation without history, commit recency or runtime authority
/// refuses each plan that reads one of them with that domain's code and
/// serves every plan that reads only the sealed tracks.
fn verify_missing_domains_and_track_plans(rt: &mut E2eRuntime) -> TestResult {
    // The plain lexical plan reads the lexical track only.
    let plain = rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K);
    served(&plain, "plain lexical query")?;
    if plain.candidate_ids.len() != 2 {
        return Err(format!("the fixture serves two rows: {:?}", plain.candidate_ids).into());
    }

    // `rev:at.time(...)` selects through the history authority: absent.
    let at_time =
        rt.query_text(TextQuerySyntax::Sourcegraph, "rev:at.time(2024-01-01) needle", TOP_K);
    refused_with(&at_time, "rev:at.time selection", HISTORY_PRODUCER_UNAVAILABLE)?;

    // `repo:has.commit.after(...)` reads the commit-recency authority
    // beside the lexical generation: absent.
    let commit_after = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.commit.after(2020-01-01) needle",
        TOP_K,
    );
    refused_with(
        &commit_after,
        "repo:has.commit.after predicate",
        REPO_COMMIT_RECENCY_UNAVAILABLE_CODE,
    )?;
    if REPO_COMMIT_RECENCY_UNAVAILABLE_CODE
        != RepoMetadataAuthorityV1::CommitRecency.unavailable_code()
    {
        return Err("the commit-recency code is the authority's own".into());
    }

    // The other repo-metadata authorities are absent too, each with its
    // own code.
    for (query_text, authority) in [
        ("repo:has.meta(team:core) needle", RepoMetadataAuthorityV1::Meta),
        ("repo:has.topic(security) needle", RepoMetadataAuthorityV1::Topic),
        ("repo:has.description(\"search\") needle", RepoMetadataAuthorityV1::Description),
        ("file:has.owner(@alice) needle", RepoMetadataAuthorityV1::FileOwnership),
        ("select:file.owners needle", RepoMetadataAuthorityV1::FileOwnership),
        ("file:has.contributor(alice) needle", RepoMetadataAuthorityV1::Contributor),
    ] {
        let result = rt.query_text(TextQuerySyntax::Sourcegraph, query_text, TOP_K);
        refused_with(&result, query_text, authority.unavailable_code())?;
    }

    // The history route reads the history authority: absent.
    let history = rt.query_history(TextQuerySyntax::Native, "type:commit needle", TOP_K);
    match &history.typed_error {
        Some(error) if error.code == E2eErrorCode::Remote(HISTORY_PRODUCER_UNAVAILABLE) => {}
        other => {
            return Err(format!(
                "history route: expected {HISTORY_PRODUCER_UNAVAILABLE}, got {other:?}"
            )
            .into());
        }
    }

    // The runtime-metadata route reads the runtime overlay: absent.
    let runtime = rt.query_runtime_metadata(TextQuerySyntax::Native, "dirty:yes needle", TOP_K);
    refused_with(&runtime, "runtime-metadata route", RUNTIME_NOT_READY_CODE)?;

    // The plans that read only the tracks still serve after the refusals:
    // nothing was left half-acquired.
    let again = rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K);
    served(&again, "plain lexical query after refusals")?;
    if again.candidate_ids != plain.candidate_ids {
        return Err(format!(
            "the plain plan is unchanged by refused ones: {:?} vs {:?}",
            again.candidate_ids, plain.candidate_ids
        )
        .into());
    }
    Ok(())
}

/// The routes that carry an explanation open their planner trace with the
/// view's domains, epochs, pin and pinned artifacts.
fn verify_explanation_read_view_identity(rt: &mut E2eRuntime) -> TestResult {
    let pin_detail = pin_detail(rt);

    let hybrid = rt.query_hybrid(TextQuerySyntax::Native, QUERY, QUERY, TOP_K);
    served(&hybrid, "hybrid query")?;
    let trace = plan_trace(&hybrid)?;
    expect_trace_head(&trace, "lexical,semantic", &pin_detail, "hybrid")?;
    for prefix in [
        "read_view.lexical_artifact=",
        "read_view.normalizer=",
        "read_view.semantic_artifact=",
        "read_view.profile=",
    ] {
        if !trace.iter().any(|detail| detail.starts_with(prefix)) {
            return Err(format!("hybrid: the trace names {prefix}: {trace:?}").into());
        }
    }

    let semantic = rt.query_semantic(QUERY, TOP_K, None);
    served(&semantic, "semantic query")?;
    let trace = plan_trace(&semantic)?;
    expect_trace_head(&trace, "semantic", &pin_detail, "semantic")?;
    if trace
        .iter()
        .any(|detail| detail.starts_with("read_view.lexical_artifact="))
    {
        return Err(
            format!("an unscoped semantic view holds no lexical artifact: {trace:?}").into()
        );
    }

    let scoped = rt.query_semantic(QUERY, TOP_K, Some((TextQuerySyntax::Native, QUERY, 2)));
    served(&scoped, "scoped semantic query")?;
    let trace = plan_trace(&scoped)?;
    expect_trace_head(&trace, "lexical,semantic", &pin_detail, "scoped semantic")?;

    let candidate = plain_first_candidate(rt)?;
    let explain = rt.explain_candidate(candidate);
    if let Some(error) = &explain.typed_error {
        return Err(format!("explain: refused: {error}").into());
    }
    let explanation = explain
        .explanation
        .as_ref()
        .ok_or("a served explain carries an explanation")?;
    let trace: Vec<String> = explanation
        .planner_trace
        .iter()
        .filter(|entry| entry.stage == PlannerStage::Plan)
        .map(|entry| entry.detail.clone())
        .collect();
    expect_trace_head(&trace, "lexical", &pin_detail, "explain")?;
    Ok(())
}

#[test]
fn sealed_track_read_view_scenarios_share_one_fixture() -> TestResult {
    let mut rt = seeded_runtime()?;
    verify_missing_domains_and_track_plans(&mut rt)
        .map_err(|error| -> Box<dyn Error> { format!("missing_domains: {error}").into() })?;
    verify_explanation_read_view_identity(&mut rt)
        .map_err(|error| -> Box<dyn Error> { format!("explanation_identity: {error}").into() })?;
    Ok(())
}

fn plain_first_candidate(
    rt: &mut E2eRuntime,
) -> Result<quanta_index_contract::LexicalCandidate, Box<dyn Error>> {
    let plain = rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K);
    served(&plain, "plain lexical query")?;
    plain
        .candidates
        .first()
        .cloned()
        .ok_or_else(|| "the fixture serves a row to explain".into())
}

/// Once the history authority is ingested the same plans serve, and the
/// history page names the epoch the view pinned.
#[test]
fn an_ingested_domain_serves_and_the_response_names_its_epoch() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo", "src/needle.rs", "fn needle() { let needle = 1; }")?;
    rt.ingest_history_fixture_spec(&E2eHistoryFixtureSpec {
        commit_sha: "1111111111111111111111111111111111111111",
        file_path: "src/needle.rs",
        author: "alice",
        committer: "alice",
        message: "needle commit",
        author_time_ms: 1_000,
        committer_time_ms: 1_000,
        applied_at_ms: 13,
        ref_name: "refs/heads/main",
        tag_name: "v1",
        added_text: "needle",
        removed_text: "",
        touched_text: "needle",
    })?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    let history = rt.query_history(TextQuerySyntax::Native, "type:commit needle", TOP_K);
    if let Some(error) = &history.typed_error {
        return Err(format!("history route with history: refused: {error}").into());
    }
    if history.commit_ids.len() != 1 {
        return Err(format!("the one commit is served: {:?}", history.commit_ids).into());
    }
    let Some(read_epoch) = history.read_epoch else {
        return Err("a served history page names the epoch the view pinned".into());
    };
    if read_epoch.get() == 0 {
        return Err("the first history mutation is epoch 1, not genesis".into());
    }

    // The commit-recency authority is still absent: the domain codes are
    // per domain, not per route.
    let commit_after = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        "repo:has.commit.after(2020-01-01) needle",
        TOP_K,
    );
    refused_with(
        &commit_after,
        "repo:has.commit.after with history but no commit recency",
        REPO_COMMIT_RECENCY_UNAVAILABLE_CODE,
    )?;
    Ok(())
}
