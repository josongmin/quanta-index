//! QI-BB-005 — `count` is a window contract, not a page widener.
//!
//! At the daemon front door a `count:all` / `count:N` query keeps its rows
//! bounded by `top_k` and reports the exact match total in the result
//! window; a plain query reports only what its continuation probe proved.
//! Projections, which collapse the whole (budgeted) match set, report their
//! collapsed universe exactly whether or not a count was asked for.
//!
//! The oracle is the fixture: five files carry the needle, on five distinct
//! paths, so every exact total is known before the query runs.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::{
    CandidateCountV1, QueryConstraintSetV1, QueryResultWindowV1, SearchPlaneQueryIpcRequest,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

const FILES: u64 = 5;

fn seeded_runtime() -> Result<E2eRuntime, Box<dyn Error>> {
    let mut rt = E2eRuntime::boot()?;
    for index in 0..FILES {
        rt.ingest_text(
            "repo",
            &format!("src/item_{index}.rs"),
            &format!("fn item_{index}() {{ needle }}"),
        )?;
    }
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

struct Observed {
    rows: usize,
    window: QueryResultWindowV1,
}

fn text_query(
    rt: &mut E2eRuntime,
    query_text: &str,
    top_k: u32,
) -> Result<Observed, Box<dyn Error>> {
    let probe = rt.probe_query_route(|pin| {
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: query_text.to_string(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: pin,
            generation_selector: None,
            top_k,
            cursor: None,
        })
    })?;
    if let Some(error) = probe.typed_error {
        return Err(format!("`{query_text}` was refused: {error}").into());
    }
    let window = probe
        .window
        .ok_or_else(|| format!("`{query_text}` answered without a window"))?;
    Ok(Observed {
        rows: probe.returned_rows,
        window,
    })
}

fn expect(
    what: &str,
    observed: &Observed,
    rows: usize,
    count: CandidateCountV1,
    has_more: bool,
) -> Result<(), Box<dyn Error>> {
    if observed.rows != rows
        || observed.window.candidate_count() != count
        || observed.window.has_more() != has_more
    {
        return Err(format!(
            "{what}: expected rows={rows} count={count:?} has_more={has_more}, observed rows={} count={:?} has_more={}",
            observed.rows,
            observed.window.candidate_count(),
            observed.window.has_more()
        )
        .into());
    }
    Ok(())
}

#[test]
fn count_all_keeps_the_page_and_reports_the_exact_total() -> TestResult {
    let mut rt = seeded_runtime()?;
    let observed = text_query(&mut rt, "count:all needle", 2)?;
    expect(
        "count:all top_k=2",
        &observed,
        2,
        CandidateCountV1::Exact(FILES),
        true,
    )?;
    let whole = text_query(&mut rt, "count:all needle", 10)?;
    expect(
        "count:all top_k=10",
        &whole,
        5,
        CandidateCountV1::Exact(FILES),
        false,
    )
}

#[test]
fn a_plain_page_reports_only_what_the_probe_proved() -> TestResult {
    let mut rt = seeded_runtime()?;
    let observed = text_query(&mut rt, "needle", 2)?;
    // Two rows plus one probe row observed: at least three, more exist.
    expect(
        "plain top_k=2",
        &observed,
        2,
        CandidateCountV1::AtLeast(3),
        true,
    )
}

#[test]
fn a_bounded_count_caps_the_page_and_still_reports_the_exact_total() -> TestResult {
    let mut rt = seeded_runtime()?;
    let observed = text_query(&mut rt, "count:3 needle", 10)?;
    expect(
        "count:3 top_k=10",
        &observed,
        3,
        CandidateCountV1::Exact(FILES),
        true,
    )
}

#[test]
fn a_projection_reports_its_collapsed_universe_exactly() -> TestResult {
    let mut rt = seeded_runtime()?;
    // Five files on five distinct paths collapse to five path rows.
    let observed = text_query(&mut rt, "select:path needle", 2)?;
    expect(
        "select:path top_k=2",
        &observed,
        2,
        CandidateCountV1::Exact(FILES),
        true,
    )
}
