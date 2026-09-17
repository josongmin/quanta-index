//! QI-BB-021 — a batch outside the ingest resource envelope is refused
//! typed with zero bytes changed, and the envelope does not touch batches
//! that fit.
//!
//! The daemon runs under a deliberately tight envelope so a small batch
//! overruns it. Oracles are external: the typed refusal code on the ingest
//! socket, the absence of any generation directory on disk for the refused
//! batch, and that a batch within the envelope applies and serves.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::PathBuf;

use quanta_index_contract::{
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, TextQuerySyntax,
};
use quanta_index_core::{
    GenerationStorageKeyV1, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, IngestResourcePolicy,
};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

/// Vector bytes one embedded record expands into under the harness's
/// default embedder: the daemon's hash embedder has
/// `SEARCH_OWNED_SEMANTIC_DIMENSION` components of four bytes each.
fn one_record_vector_bytes() -> Result<u64, Box<dyn Error>> {
    Ok(
        u64::try_from(quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_DIMENSION)?
            .saturating_mul(4),
    )
}

fn pair_dir(rt: &E2eRuntime, track_root: &str) -> PathBuf {
    rt.state_root()
        .join(track_root)
        .join(GenerationStorageKeyV1::for_repo_revision(&rt.repo(), &rt.revision()).as_str())
}

fn typed_code(response: &SearchPlaneIngestIpcResponse) -> Option<&str> {
    match response {
        SearchPlaneIngestIpcResponse::Error(error) => Some(error.code.as_str()),
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
        | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => None,
    }
}

/// Under an envelope that admits one record's vectors but not two, a
/// two-record batch is refused typed before either track writes, and a
/// one-record batch still applies, seals and serves.
#[test]
fn a_batch_past_the_vector_envelope_is_refused_before_any_track_writes() -> TestResult {
    // The harness builds one chunk and one typed semantic source per text
    // batch; the source is what the daemon embeds, so one batch is one
    // embedded record.
    let policy = IngestResourcePolicy::new(
        usize::MAX,
        u64::MAX,
        one_record_vector_bytes()?
            .saturating_mul(2)
            .saturating_sub(1),
    )?;
    let mut rt = E2eRuntime::boot_with_ingest_resource_policy(policy)?;
    rt.start()?;

    let mut oversized = rt.text_search_corpus_batch(
        "src/envelope.rs",
        "fn envelope_body() { envelope_needle }",
        "envelope-2",
    )?;
    // A second scope doubles the embedded records: two sources, two vectors.
    let mut second = rt.text_search_corpus_batch(
        "src/envelope_two.rs",
        "fn envelope_second() { envelope_second_needle }",
        "envelope-2",
    )?;
    oversized.replace_scopes.append(&mut second.replace_scopes);
    oversized
        .semantic_replace_scopes
        .append(&mut second.semantic_replace_scopes);

    let refused = rt.ingest_once(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(
        oversized,
    ))?;
    if typed_code(&refused) != Some(INGEST_RESOURCE_BUDGET_EXCEEDED_CODE) {
        return Err(format!(
            "a two-record batch under a one-record envelope must be refused typed, got {refused:?}"
        )
        .into());
    }
    for track in ["indexes/lexical", "indexes/semantic"] {
        let pair = pair_dir(&rt, track);
        if pair.exists() {
            return Err(format!(
                "a refused batch must leave no generation on disk, found {}",
                pair.display()
            )
            .into());
        }
    }

    // One record fits: the same daemon applies it and serves it.
    let fits = rt.text_search_corpus_batch(
        "src/envelope.rs",
        "fn envelope_body() { envelope_needle }",
        "envelope-1",
    )?;
    let accepted = rt.ingest_once(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(fits))?;
    match &accepted {
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt) if receipt.applied => {}
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
        | SearchPlaneIngestIpcResponse::Error(_)
        | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => {
            return Err(format!("a fitting batch must apply, got {accepted:?}").into());
        }
    }
    let _sealed = rt.seal()?;
    let served = rt.query_text(TextQuerySyntax::Native, "envelope_needle", 5);
    if let Some(error) = served.typed_error {
        return Err(format!("the fitting batch must serve: {error}").into());
    }
    if served.candidate_ids.len() != 1 {
        return Err(format!(
            "the fitting batch must serve its one chunk, served {}",
            served.candidate_ids.len()
        )
        .into());
    }
    Ok(())
}

/// The record ceiling counts every carried row, independent of vector
/// bytes: the harness's batch carries one chunk and one typed source, two
/// rows, and does not fit a one-row envelope.
#[test]
fn the_record_ceiling_counts_every_carried_row() -> TestResult {
    let policy = IngestResourcePolicy::new(1, u64::MAX, u64::MAX)?;
    let mut rt = E2eRuntime::boot_with_ingest_resource_policy(policy)?;
    let batch =
        rt.text_search_corpus_batch("src/rows.rs", "fn rows_body() { rows_needle }", "rows-1")?;
    let carried = batch
        .replace_scopes
        .iter()
        .map(|s| s.chunks.len())
        .sum::<usize>()
        + batch
            .semantic_replace_scopes
            .iter()
            .map(|s| s.sources.len())
            .sum::<usize>();
    if carried != 2 {
        return Err(format!("the fixture must carry two rows, carries {carried}").into());
    }
    let refused = rt.ingest_once(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch))?;
    if typed_code(&refused) != Some(INGEST_RESOURCE_BUDGET_EXCEEDED_CODE) {
        return Err(format!(
            "two carried rows under a one-row envelope must be refused typed, got {refused:?}"
        )
        .into());
    }
    Ok(())
}
