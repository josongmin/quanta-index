//! QI-BB-032 — a batch digest is the canonical digest of one body, applied
//! once.
//!
//! Before this, a producer that lost an ack and re-sent its batch made the
//! search plane derive, embed and build again, and a different body under
//! the same digest was applied as if it were the same. Now `batch_digest`
//! is the canonical digest of the body and every receipt-bearing publish
//! runs under a durable idempotency record: a replay of the same body is
//! answered from the record with `applied = false` and the original
//! sequence, a body that is not what its digest names is a typed
//! `BATCH_DIGEST_MISMATCH` before any mutation or record, 8 and 32
//! concurrent duplicates converge on one apply, and the record survives a
//! daemon restart.
//!
//! Oracles are external: the receipts' `applied` / `durable_sequence`, the
//! typed refusal, the idempotency table's row count read straight from the
//! catalog file, and what a query serves afterwards.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crate::e2e_harness;
use quanta_index_contract::{
    BatchPublishReceipt, ERR_SERVER_OVERLOADED, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, TextQuerySyntax,
};
use quanta_index_core::BATCH_DIGEST_MISMATCH_CODE;
use quanta_index_ipc::{ClientIoPolicy, send_request};

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

/// The idempotency table's row count, read straight from the catalog file
/// the daemon writes (`state_root/catalog/catalog-v1.sqlite`).
fn idempotency_rows(rt: &E2eRuntime) -> Result<u64, Box<dyn Error>> {
    let path = quanta_index_catalog::catalog_dir(rt.state_root())
        .join(quanta_index_catalog::CATALOG_FILE_NAME);
    let connection =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let rows: i64 =
        connection.query_row("SELECT COUNT(*) FROM idempotency_v2", [], |row| row.get(0))?;
    Ok(u64::try_from(rows)?)
}

fn receipt_of(
    response: SearchPlaneIngestIpcResponse,
) -> Result<BatchPublishReceipt, Box<dyn Error>> {
    match response {
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(outcome) => Ok(outcome.receipt),
        SearchPlaneIngestIpcResponse::Error(error) => {
            Err(format!("refused: {}: {}", error.code, error.message).into())
        }
        other @ (SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)) => {
            Err(format!("unexpected ingest response {other:?}").into())
        }
    }
}

fn typed_code(response: &SearchPlaneIngestIpcResponse) -> Option<&str> {
    match response {
        SearchPlaneIngestIpcResponse::Error(error) => Some(error.code.as_wire_str()),
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
        | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => None,
    }
}

/// The same body twice is one apply; a different body wearing the first
/// body's digest is refused, records nothing, and is never served.
#[test]
fn a_replay_is_acked_from_the_record_and_a_forged_digest_never_lands() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let batch = rt.text_search_corpus_batch("src/idem.rs", "fn first_body() { idem_first }")?;
    let first = receipt_of(rt.ingest_once(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
    )?)?;
    if !first.applied || first.durable_sequence == 0 || first.batch_digest != batch.batch_digest {
        return Err(format!("first publish must apply under a sequence: {first:?}").into());
    }
    let rows_after_apply = idempotency_rows(&rt)?;
    if rows_after_apply != 1 {
        return Err(format!("one apply must leave one record, found {rows_after_apply}").into());
    }

    let replay = receipt_of(rt.ingest_once(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
    )?)?;
    if replay.applied {
        return Err(format!("a replay must not report a new apply: {replay:?}").into());
    }
    if replay.durable_sequence != first.durable_sequence
        || replay.generation != first.generation
        || replay.manifest_digest != first.manifest_digest
        || replay.accepted_replace_scopes != first.accepted_replace_scopes
    {
        return Err(format!(
            "a replay must carry the original apply's receipt: first={first:?} replay={replay:?}"
        )
        .into());
    }

    // One byte of the body changes; the carried digest does not.
    let mut forged = batch.clone();
    forged
        .replace_scopes
        .first_mut()
        .and_then(|scope| scope.chunks.first_mut())
        .ok_or("the batch carries one chunk")?
        .text = "fn second_body() { idem_second }".into();
    let refused = rt.ingest_once(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(
        forged,
    ))?;
    if typed_code(&refused) != Some(BATCH_DIGEST_MISMATCH_CODE.as_wire_str()) {
        return Err(format!(
            "a body that is not what its digest names must be refused typed, got {refused:?}"
        )
        .into());
    }
    if idempotency_rows(&rt)? != rows_after_apply {
        return Err("a refused forged batch must leave the catalog untouched".into());
    }

    // What is served is the first body and only the first body.
    rt.publish_search_corpus_batch(batch)?;
    rt.activate_last_sealed_generation()?;
    let served_first = rt.query_text(TextQuerySyntax::Native, "idem_first", 5);
    if let Some(error) = served_first.typed_error {
        return Err(format!("the applied body must serve: {error}").into());
    }
    if served_first.candidate_ids.len() != 1 {
        return Err(format!(
            "the applied body must serve exactly once, served {}",
            served_first.candidate_ids.len()
        )
        .into());
    }
    let served_second = rt.query_text(TextQuerySyntax::Native, "idem_second", 5);
    if !served_second.candidate_ids.is_empty() {
        return Err("the refused body must never be served".into());
    }
    Ok(())
}

/// One producer publishing `request` the way the admission contract asks.
///
/// A typed `SERVER_OVERLOADED` (QI-BB-002) — the serial ingest slot was
/// busy past the queue budget — is retried with backoff, and every retry
/// of the same body is itself a replay of the same idempotency key.
fn publish_with_backoff(
    socket: &Path,
    request_id: u64,
    request: &SearchPlaneIngestIpcRequest,
) -> Result<SearchPlaneIngestIpcResponse, Box<dyn Error + Send + Sync>> {
    const PATIENCE: Duration = Duration::from_secs(120);
    let started = Instant::now();
    loop {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id,
            payload: request.clone(),
        };
        let response = send_request::<_, SearchPlaneIngestIpcResponseEnvelope>(
            socket,
            &envelope,
            ClientIoPolicy::default(),
        )?
        .payload;
        let overloaded = matches!(
            &response,
            SearchPlaneIngestIpcResponse::Error(error) if error.code == ERR_SERVER_OVERLOADED
        );
        if !overloaded || started.elapsed() >= PATIENCE {
            return Ok(response);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// `publishers` publishers of the same batch converge on one apply.
///
/// Exactly one receipt says `applied`, every receipt carries the same
/// sequence, the catalog holds one record, and the corpus holds the row
/// once.
fn duplicate_publishes_converge_on_one_apply(publishers: u64) -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let batch = rt.text_search_corpus_batch("src/race.rs", "fn raced_body() { idem_race }")?;
    let socket = rt.ingest_socket_path()?;
    let publishers = (0..publishers)
        .map(|index| {
            let socket = socket.clone();
            let batch = batch.clone();
            thread::spawn(move || {
                publish_with_backoff(
                    &socket,
                    1_000_u64.saturating_add(index),
                    &SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
                )
            })
        })
        .collect::<Vec<_>>();
    let mut receipts = Vec::new();
    for publisher in publishers {
        let response = publisher
            .join()
            .map_err(|panic| format!("publisher panicked: {panic:?}"))?
            .map_err(|err| -> Box<dyn Error> { err })?;
        receipts.push(receipt_of(response)?);
    }
    let applied = receipts.iter().filter(|receipt| receipt.applied).count();
    if applied != 1 {
        return Err(format!("exactly one publisher may apply, {applied} did: {receipts:?}").into());
    }
    let sequences: BTreeSet<u64> = receipts
        .iter()
        .map(|receipt| receipt.durable_sequence)
        .collect();
    if sequences.len() != 1 {
        return Err(format!(
            "every receipt must carry the one apply's sequence, got {sequences:?}"
        )
        .into());
    }
    if idempotency_rows(&rt)? != 1 {
        return Err("duplicate publishes must leave exactly one record".into());
    }
    rt.publish_search_corpus_batch(batch)?;
    rt.activate_last_sealed_generation()?;
    let served = rt.query_text(TextQuerySyntax::Native, "idem_race", 5);
    if let Some(error) = served.typed_error {
        return Err(format!("the applied body must serve: {error}").into());
    }
    if served.candidate_ids.len() != 1 {
        return Err(format!(
            "duplicate publishes must leave one row, found {}",
            served.candidate_ids.len()
        )
        .into());
    }
    Ok(())
}

#[test]
fn eight_concurrent_duplicate_publishes_converge_on_one_apply() -> TestResult {
    duplicate_publishes_converge_on_one_apply(8)
}

#[test]
fn thirty_two_concurrent_duplicate_publishes_converge_on_one_apply() -> TestResult {
    duplicate_publishes_converge_on_one_apply(32)
}

/// The record is durable: a replay after a daemon restart is still answered
/// from it, with the original sequence.
#[test]
fn a_replay_after_restart_is_still_a_replay() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let batch =
        rt.text_search_corpus_batch("src/durable.rs", "fn durable_body() { idem_durable }")?;
    let first = receipt_of(rt.ingest_once(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
    )?)?;
    let mut rt = rt.reopen();
    let replay =
        receipt_of(rt.ingest_once(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch))?)?;
    if replay.applied || replay.durable_sequence != first.durable_sequence {
        return Err(format!(
            "a replay across restart must be the recorded apply: first={first:?} replay={replay:?}"
        )
        .into());
    }
    Ok(())
}
