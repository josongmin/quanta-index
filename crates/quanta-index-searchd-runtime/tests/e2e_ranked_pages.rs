//! QI-BB-005 — ranked text pages at the daemon front door: cut in the one
//! total order, continued by their cursor, cut by the response byte budget
//! with an explicit continuation, and never continued in another
//! generation.
//!
//! The fixture's files all hold the same text, so every row ties on score
//! and the order below the score is the path; the files are ingested in
//! reverse path order, so index order is the opposite of page order. The
//! oracle for every walk is the single page that holds every row, checked
//! to be the files in path order.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::{
    GenerationPin, LexicalCursor, ManifestGeneration, QUERY_CURSOR_GENERATION_MISMATCH_CODE,
    TextQueryResponse, TextQuerySyntax,
};
use quanta_index_search_plane::ResponsePayloadBudget;
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::{E2eRoutePage, E2eRuntime};

type TestResult = Result<(), Box<dyn Error>>;

const FILES: usize = 9;
const QUERY: &str = "page_needle";

fn path(file: usize) -> String {
    format!("src/file_{file:02}.rs")
}

/// Every file the same text, ingested in reverse path order; sealed and
/// activated.
fn seeded(mut rt: E2eRuntime) -> Result<(E2eRuntime, GenerationPin), Box<dyn Error>> {
    for file in (0..FILES).rev() {
        rt.ingest_text(
            "repo",
            &path(file),
            "fn tied() { let page_needle = \"the same text in every file\"; }",
        )?;
    }
    let generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let pin = GenerationPin::new(rt.repo(), rt.revision(), generation);
    Ok((rt, pin))
}

fn served(page: E2eRoutePage<TextQueryResponse>) -> Result<TextQueryResponse, Box<dyn Error>> {
    Ok(page.served("text page")?)
}

fn paths(page: &TextQueryResponse) -> Vec<String> {
    page.results
        .iter()
        .map(|row| row.repo_relative_path.as_str().to_string())
        .collect()
}

/// Walk every page of `top_k`, checking each page's continuation.
fn walk(
    rt: &mut E2eRuntime,
    pin: &GenerationPin,
    top_k: u32,
) -> Result<(Vec<String>, usize), Box<dyn Error>> {
    let mut rows: Vec<String> = Vec::new();
    let mut pages = 0_usize;
    let mut cursor: Option<LexicalCursor> = None;
    loop {
        let page = served(rt.query_text_page(
            TextQuerySyntax::Native,
            QUERY,
            top_k,
            Some(pin.clone()),
            cursor.clone(),
        )?)?;
        pages = pages.saturating_add(1);
        if page.window.has_more() != page.next_cursor.is_some() {
            return Err(format!("page {pages}: has_more and the cursor disagree").into());
        }
        rows.extend(paths(&page));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok((rows, pages)),
        }
        if pages > FILES.saturating_add(1) {
            return Err("the walk did not end".into());
        }
    }
}

fn every_path() -> Vec<String> {
    (0..FILES).map(path).collect()
}

#[test]
fn a_tied_corpus_walks_page_by_page_in_path_order() -> TestResult {
    let (mut rt, pin) = seeded(E2eRuntime::boot()?)?;
    let whole = served(rt.query_text_page(
        TextQuerySyntax::Native,
        QUERY,
        100,
        Some(pin.clone()),
        None,
    )?)?;
    if paths(&whole) != every_path() || whole.next_cursor.is_some() {
        return Err(format!(
            "the whole page is not every file in path order: {:?}",
            paths(&whole)
        )
        .into());
    }
    for top_k in [1_u32, 2, 4] {
        let (rows, pages) = walk(&mut rt, &pin, top_k)?;
        if rows != every_path() {
            return Err(format!("the {top_k}-row walk drifted: {rows:?}").into());
        }
        let expected_pages = FILES.div_ceil(usize::try_from(top_k)?);
        if pages != expected_pages {
            return Err(format!("{top_k}-row pages: {pages}, expected {expected_pages}").into());
        }
    }
    Ok(())
}

/// Under a small response budget a wide page is cut by its bytes, not by
/// `top_k`: each page stays under the budget, says more rows exist, and the
/// cursor walk still returns every row once in order.
#[test]
fn a_byte_budget_cuts_pages_with_an_explicit_continuation() -> TestResult {
    let budget = ResponsePayloadBudget::new(1_200)?;
    let (mut rt, pin) = seeded(E2eRuntime::boot_with_query_response_budget(budget)?)?;
    let first = served(rt.query_text_page(
        TextQuerySyntax::Native,
        QUERY,
        100,
        Some(pin.clone()),
        None,
    )?)?;
    let encoded = quanta_index_ipc::cbor_payload_len(&first)?;
    if encoded > budget.max_payload_bytes() {
        return Err(format!("a cut page encodes to {encoded} bytes").into());
    }
    if first.results.is_empty() || first.results.len() >= FILES || !first.window.has_more() {
        return Err(format!(
            "nine rows do not fit 1200 bytes but one does: {} rows",
            first.results.len()
        )
        .into());
    }
    let (rows, pages) = walk(&mut rt, &pin, 100)?;
    if rows != every_path() || pages < 2 {
        return Err(format!("the byte-cut walk drifted: {rows:?} over {pages} pages").into());
    }
    Ok(())
}

/// A cursor names the generation it was cut from; continuing it against
/// the next generation is refused typed.
#[test]
fn a_cursor_does_not_continue_in_another_generation() -> TestResult {
    let (mut rt, first_pin) = seeded(E2eRuntime::boot()?)?;
    let page =
        served(rt.query_text_page(TextQuerySyntax::Native, QUERY, 2, Some(first_pin), None)?)?;
    let cursor = page.next_cursor.ok_or("a two-row page of nine continues")?;
    rt.ingest_text("repo", "src/late.rs", "fn late() { page_needle }")?;
    let next = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    if next == ManifestGeneration::new(cursor.manifest_generation.get()) {
        return Err("the second seal is a new generation".into());
    }
    let next_pin = GenerationPin::new(rt.repo(), rt.revision(), next);
    match rt.query_text_page(
        TextQuerySyntax::Native,
        QUERY,
        2,
        Some(next_pin),
        Some(cursor),
    )? {
        E2eRoutePage::Refused(error) if error.code == QUERY_CURSOR_GENERATION_MISMATCH_CODE => {
            Ok(())
        }
        other @ (E2eRoutePage::Served(_) | E2eRoutePage::Refused(_)) => {
            Err(format!("a cursor from the first generation must be refused: {other:?}").into())
        }
    }
}
