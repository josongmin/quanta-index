//! QI-BB-005 — ranked pages from a pinned old-Q process through the public SDK.
//!
//! All files contain identical text. The full page and each cursor walk must
//! prove the path tie-break across the public Source publication boundary.

#![forbid(unsafe_code)]

use std::{error::Error, path::Path, time::Duration};

use quanta_index_contract::{
    ContinuationTokenV2, GenerationPin, ManifestGeneration, RepoId, RevisionId,
    SearchCorpusActiveHeadV1, SearchPlaneErrorCodeV2, TextQueryResponse,
};
use quanta_index_sdk::{QuantaIndex, SdkError};
use quanta_index_search_plane::ResponsePayloadBudget;
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, LegacySearchdCommand, SearchdBinaryProcess, publish_legacy_source,
    },
    private_tempdir,
};

use crate::fail_closed_wait::{RealTicker, wait_for};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const FILES: usize = 9;
const QUERY: &str = "page_needle";
const BODY: &str = "fn tied() { let page_needle = \"the same text in every file\"; }";

fn path(file: usize) -> String {
    format!("src/file_{file:02}.rs")
}

fn configured_command<'binary>(
    binary: &'binary LegacySearchdBinary,
    state_root: &Path,
    budget: Option<ResponsePayloadBudget>,
) -> TestResult<LegacySearchdCommand<'binary>> {
    let mut command = binary.command()?;
    let _serve = command.arg("serve").arg("--state-root").arg(state_root);
    let _configured = command.apply_searchd_env(8);
    if let Some(budget) = budget {
        let _configured = command.env(
            "QUANTA_INDEX_QUERY_RESPONSE_MAX_BYTES",
            budget.max_payload_bytes().to_string(),
        );
    }
    Ok(command)
}

fn publish_tied_corpus(
    client: &QuantaIndex,
    repo: &RepoId,
    revision: &RevisionId,
) -> TestResult<SearchCorpusActiveHeadV1> {
    let files = (0..FILES)
        .rev()
        .map(|file| Ok((path(file), BODY, u64::try_from(file)? + 1)))
        .collect::<Result<Vec<_>, std::num::TryFromIntError>>()?;
    let borrowed = files
        .iter()
        .map(|(file, body, id)| (file.as_str(), *body, *id))
        .collect::<Vec<_>>();
    // The public batch must encode scopes in canonical ascending order. The
    // fixed fixture owns that ordering and verifies durable activation.
    let (head, _receipt) = publish_legacy_source(client, repo, revision, 1, None, &borrowed, None)?;
    Ok(head)
}

fn with_published_reference<T>(
    budget: Option<ResponsePayloadBudget>,
    check: impl FnOnce(&QuantaIndex, &GenerationPin, &SearchCorpusActiveHeadV1) -> TestResult<T>,
) -> TestResult<T> {
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let process = SearchdBinaryProcess::start_with_pinned_command(
        &state_root,
        configured_command(&binary, &state_root, budget)?,
    )?;
    let result = (|| -> TestResult<T> {
        let client = process.connect()?;
        let repo = RepoId::new("repo-e2e")?;
        let revision = RevisionId::new("rev-e2e")?;
        let head = publish_tied_corpus(&client, &repo, &revision)?;
        let _ready = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "pinned old-Q ranked-page query backend ready",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| true,
        )?;
        let pin = GenerationPin::new(repo, revision, ManifestGeneration::new(1));
        check(&client, &pin, &head)
    })();
    let stopped = process.stop();
    let value = result?;
    stopped?;
    binary.verify()?;
    Ok(value)
}

fn query_text_page(
    client: &QuantaIndex,
    pin: &GenerationPin,
    top_k: u32,
    cursor: Option<ContinuationTokenV2>,
) -> Result<TextQueryResponse, SdkError> {
    let request = client
        .lexical()
        .query()
        .native(QUERY)
        .pinned(pin.clone())
        .top_k(top_k);
    match cursor {
        Some(cursor) => request.after(cursor).execute(),
        None => request.execute(),
    }
}

fn paths(page: &TextQueryResponse) -> Vec<String> {
    page.results
        .iter()
        .map(|row| row.repo_relative_path.as_str().to_string())
        .collect()
}

fn walk(client: &QuantaIndex, pin: &GenerationPin, top_k: u32) -> TestResult<(Vec<String>, usize)> {
    let mut rows = Vec::new();
    let mut pages = 0_usize;
    let mut cursor = None;
    loop {
        let page = query_text_page(client, pin, top_k, cursor)?;
        pages = pages.saturating_add(1);
        if page.window.has_more() != Some(page.next_cursor.is_some()) {
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
    with_published_reference(None, |client, pin, _head| {
        let whole = query_text_page(client, pin, 100, None)?;
        if paths(&whole) != every_path() || whole.next_cursor.is_some() {
            return Err(format!(
                "the whole page is not every file in path order: {:?}",
                paths(&whole)
            )
            .into());
        }
        for top_k in [1_u32, 2, 4] {
            let (rows, pages) = walk(client, pin, top_k)?;
            if rows != every_path() {
                return Err(format!("the {top_k}-row walk drifted: {rows:?}").into());
            }
            let expected_pages = FILES.div_ceil(usize::try_from(top_k)?);
            if pages != expected_pages {
                return Err(
                    format!("{top_k}-row pages: {pages}, expected {expected_pages}").into(),
                );
            }
        }
        Ok(())
    })
}

#[test]
fn a_byte_budget_cuts_pages_with_an_explicit_continuation() -> TestResult {
    let (one_bytes, full_bytes) = with_published_reference(None, |client, pin, _head| {
        let one = query_text_page(client, pin, 1, None)?;
        let full = query_text_page(client, pin, 100, None)?;
        if one.results.len() != 1 || full.results.len() != FILES {
            return Err("the budget oracle requires one-row and full-row pages".into());
        }
        Ok((
            quanta_index_ipc::cbor_payload_len(&one)?,
            quanta_index_ipc::cbor_payload_len(&full)?,
        ))
    })?;
    let lower = one_bytes
        .checked_add(1_024)
        .ok_or("one-row budget overflow")?;
    if lower >= full_bytes {
        return Err(format!(
            "the fixture has no byte-cut interval: one={one_bytes} full={full_bytes}"
        )
        .into());
    }
    let budget = ResponsePayloadBudget::new(lower.midpoint(full_bytes))?;
    with_published_reference(Some(budget), |client, pin, _head| {
        let first = query_text_page(client, pin, 100, None)?;
        let encoded = quanta_index_ipc::cbor_payload_len(&first)?;
        if encoded > budget.max_payload_bytes() {
            return Err(format!("a cut page encodes to {encoded} bytes").into());
        }
        if first.results.is_empty()
            || first.results.len() >= FILES
            || first.window.has_more() != Some(true)
        {
            return Err(format!(
                "nine rows do not fit the measured byte cap but one does: {} rows",
                first.results.len()
            )
            .into());
        }
        let (rows, pages) = walk(client, pin, 100)?;
        if rows != every_path() || pages < 2 {
            return Err(format!("the byte-cut walk drifted: {rows:?} over {pages} pages").into());
        }
        Ok(())
    })
}

#[test]
fn a_cursor_does_not_continue_in_another_generation() -> TestResult {
    with_published_reference(None, |client, first_pin, first_head| {
        let page = query_text_page(client, first_pin, 2, None)?;
        let cursor = page.next_cursor.ok_or("a two-row page of nine continues")?;
        let original = path(0);
        let (next_head, _receipt) = publish_legacy_source(
            client,
            &first_pin.repo_id,
            &first_pin.revision_id,
            2,
            Some((&original, BODY, 1)),
            &[
                (&original, BODY, 1),
                ("src/late.rs", "fn late() { page_needle }", 10),
            ],
            Some(first_head.clone()),
        )?;
        let next = next_head.generation.lexical.manifest_generation;
        if next == first_pin.manifest_generation {
            return Err("the second publication did not activate a new generation".into());
        }
        let next_pin = GenerationPin::new(
            first_pin.repo_id.clone(),
            first_pin.revision_id.clone(),
            next,
        );
        match query_text_page(client, &next_pin, 2, Some(cursor)) {
            Err(SdkError::Remote {
                code: SearchPlaneErrorCodeV2::CursorContextMismatch,
                ..
            }) => Ok(()),
            other => {
                Err(format!("a cursor from the first generation must be refused: {other:?}").into())
            }
        }
    })
}
