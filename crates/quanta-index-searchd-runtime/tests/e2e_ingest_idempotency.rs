//! QI-BB-032 — a batch digest names one immutable body, applied once.
//!
//! Before this, a producer that lost an ack and re-sent its batch made the
//! search plane derive, embed and build again, and a different body under
//! the same digest was applied as if it were the same. Now every
//! receipt-bearing publish runs under a durable idempotency record: a
//! replay of the same body is answered from the record with `applied =
//! false` and the original sequence, a different body is a typed
//! `BATCH_DIGEST_CONFLICT` before any mutation, concurrent duplicates
//! converge on one apply, and the record survives a daemon restart.
//!
//! Oracles are external: the receipts' `applied` / `durable_sequence`, the
//! typed refusal, and what a query serves afterwards.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::thread;

use quanta_index_contract::{
    BatchPublishReceipt, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope,
    SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope, TextQuerySyntax,
};
use quanta_index_core::BATCH_DIGEST_CONFLICT_CODE;
use quanta_index_ipc::{ClientIoPolicy, send_request};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

fn receipt_of(
    response: SearchPlaneIngestIpcResponse,
) -> Result<BatchPublishReceipt, Box<dyn Error>> {
    match response {
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt) => Ok(receipt),
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
        | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)) => {
            Err(format!("unexpected ingest response {other:?}").into())
        }
    }
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

/// The same body twice is one apply; a different body under the same
/// digest is refused and never served.
#[test]
fn a_replay_is_acked_from_the_record_and_a_conflict_never_lands() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let batch =
        rt.text_search_corpus_batch("src/idem.rs", "fn first_body() { idem_first }", "idem-1")?;
    let first = receipt_of(rt.ingest_once(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
    )?)?;
    if !first.applied || first.durable_sequence == 0 || first.batch_digest != "idem-1" {
        return Err(format!("first publish must apply under a sequence: {first:?}").into());
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

    let mut different = batch;
    different
        .replace_scopes
        .first_mut()
        .and_then(|scope| scope.chunks.first_mut())
        .ok_or("the batch carries one chunk")?
        .text = "fn second_body() { idem_second }".into();
    let refused = rt.ingest_once(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(
        different,
    ))?;
    if typed_code(&refused) != Some(BATCH_DIGEST_CONFLICT_CODE) {
        return Err(format!(
            "a different body under the same digest must be refused typed, got {refused:?}"
        )
        .into());
    }

    // What is served is the first body and only the first body.
    let _sealed = rt.seal()?;
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

/// Eight publishers of the same batch converge on one apply: exactly one
/// receipt says `applied`, every receipt carries the same sequence, and the
/// corpus holds the row once.
#[test]
fn concurrent_duplicate_publishes_converge_on_one_apply() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let batch =
        rt.text_search_corpus_batch("src/race.rs", "fn raced_body() { idem_race }", "idem-race")?;
    let socket = rt.ingest_socket_path()?;
    let publishers = (0..8_u64)
        .map(|index| {
            let socket = socket.clone();
            let batch = batch.clone();
            thread::spawn(move || {
                let envelope = SearchPlaneIngestIpcRequestEnvelope {
                    request_id: 1_000 + index,
                    payload: SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
                };
                send_request::<_, SearchPlaneIngestIpcResponseEnvelope>(
                    &socket,
                    &envelope,
                    ClientIoPolicy::default(),
                )
                .map(|response| response.payload)
            })
        })
        .collect::<Vec<_>>();
    let mut receipts = Vec::new();
    for publisher in publishers {
        let response = publisher
            .join()
            .map_err(|panic| format!("publisher panicked: {panic:?}"))??;
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
    let _sealed = rt.seal()?;
    let served = rt.query_text(TextQuerySyntax::Native, "idem_race", 5);
    if let Some(error) = served.typed_error {
        return Err(format!("the applied body must serve: {error}").into());
    }
    if served.candidate_ids.len() != 1 {
        return Err(format!(
            "eight duplicate publishes must leave one row, found {}",
            served.candidate_ids.len()
        )
        .into());
    }
    Ok(())
}

/// The record is durable: a replay after a daemon restart is still answered
/// from it, with the original sequence.
#[test]
fn a_replay_after_restart_is_still_a_replay() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let batch = rt.text_search_corpus_batch(
        "src/durable.rs",
        "fn durable_body() { idem_durable }",
        "idem-durable",
    )?;
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
