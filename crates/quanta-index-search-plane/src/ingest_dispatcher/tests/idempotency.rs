//! QI-BB-032 — every receipt-bearing route runs under its idempotency
//! record, proven at the dispatcher with one live route and the rest
//! unreachable.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use quanta_index_contract::lex::DirtyRecord;
use quanta_index_contract::{
    BatchPublishReceipt, ChunkId, DirtyIngestBatch, DirtyMutation, FileContributorIngestBatch,
    FileOwnershipIngestBatch, HistoryIngestBatch, ManifestGeneration, RepoCommitRecencyIngestBatch,
    RepoDescriptionIngestBatch, RepoId, RepoMapSourceBundle, RepoMetaIngestBatch,
    RepoTopicIngestBatch, RevisionId, RuntimeCatalogIngestBatch, SearchCorpusIngestBatch,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, StructuralIngestBatch,
};
use quanta_index_core::{
    BATCH_DIGEST_CONFLICT_CODE, CoreError, FileContributorIngestPort, FileOwnershipIngestPort,
    RepoCommitRecencyIngestPort, RepoDescriptionIngestPort, RepoMapBundleIngestPort,
    RepoMetaIngestPort, RepoTopicIngestPort, RequestBudgetV1, SearchCorpusIngestPort,
};

use super::support::MemoryIdempotencyCatalog;
use crate::ingest_dispatcher::auxiliary::dirty_publish_receipt_v1;
use crate::ingest_dispatcher::dispatcher::SearchPlaneIngestDispatcher;
use crate::ingest_dispatcher::ports::{
    HistoryIngestPort, RuntimeMetadataIngestPort, StructuralIngestPort,
};

type TestRes = Result<(), Box<dyn std::error::Error>>;

/// A route the test never expects to reach.
struct Unreachable;

fn unreachable_route(route: &str) -> CoreError {
    CoreError::Storage(format!("test: {route} route must not be reached"))
}

impl SearchCorpusIngestPort for Unreachable {
    fn publish_batch(
        &self,
        _batch: &SearchCorpusIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("search corpus"))
    }
}
impl HistoryIngestPort for Unreachable {
    fn publish_batch(&self, _batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("history"))
    }
}
impl RepoCommitRecencyIngestPort for Unreachable {
    fn publish_batch(
        &self,
        _batch: &RepoCommitRecencyIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("repo commit recency"))
    }
}
impl RepoTopicIngestPort for Unreachable {
    fn publish_batch(
        &self,
        _batch: &RepoTopicIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("repo topic"))
    }
}
impl RepoDescriptionIngestPort for Unreachable {
    fn publish_batch(
        &self,
        _batch: &RepoDescriptionIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("repo description"))
    }
}
impl FileOwnershipIngestPort for Unreachable {
    fn publish_batch(
        &self,
        _batch: &FileOwnershipIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("file ownership"))
    }
}
impl FileContributorIngestPort for Unreachable {
    fn publish_batch(
        &self,
        _batch: &FileContributorIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("file contributor"))
    }
}
impl RepoMetaIngestPort for Unreachable {
    fn publish_batch(
        &self,
        _batch: &RepoMetaIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("repo meta"))
    }
}
impl StructuralIngestPort for Unreachable {
    fn publish_batch(
        &self,
        _batch: &StructuralIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("structural"))
    }
}
impl RepoMapBundleIngestPort for Unreachable {
    fn ingest_bundle(&self, _bundle: &RepoMapSourceBundle) -> Result<(), CoreError> {
        Err(unreachable_route("repo map bundle"))
    }
}

/// The one live route: counts applies and answers the dirty receipt.
struct CountingRuntime {
    applies: AtomicUsize,
}

impl RuntimeMetadataIngestPort for CountingRuntime {
    fn publish_batch(&self, batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let _prior = self.applies.fetch_add(1, Ordering::SeqCst);
        Ok(dirty_publish_receipt_v1(batch))
    }
    fn publish_catalog_batch(
        &self,
        _batch: &RuntimeCatalogIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("runtime catalog"))
    }
}

fn dispatcher(
    runtime: Arc<CountingRuntime>,
    catalog: Arc<MemoryIdempotencyCatalog>,
) -> SearchPlaneIngestDispatcher {
    let unreachable = Arc::new(Unreachable);
    SearchPlaneIngestDispatcher::new(
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        runtime,
        unreachable.clone(),
        unreachable,
        catalog,
    )
}

fn dirty_batch(digest: &str, doc: &str) -> DirtyIngestBatch {
    DirtyIngestBatch {
        repo_id: RepoId::new("repo-idem"),
        revision_id: RevisionId::new("rev-idem"),
        generation: ManifestGeneration::new(4),
        overlay_epoch_ms: 11,
        batch_digest: digest.to_string(),
        entries: vec![DirtyMutation::Upsert(DirtyRecord {
            wire_version: 1,
            doc_id: ChunkId::new(doc),
            applied_at_ms: 11,
            payload_hash: [3; 32],
        })],
    }
}

fn typed_code_of(response: &SearchPlaneIngestIpcResponse) -> Option<String> {
    match response {
        SearchPlaneIngestIpcResponse::Error(error) => Some(error.code.clone()),
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

fn receipt_of(response: SearchPlaneIngestIpcResponse) -> Result<BatchPublishReceipt, String> {
    match response {
        SearchPlaneIngestIpcResponse::DirtyReceipt(receipt) => Ok(receipt),
        SearchPlaneIngestIpcResponse::Error(error) => {
            Err(format!("{}: {}", error.code, error.message))
        }
        other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
        | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)) => {
            Err(format!("unexpected response {other:?}"))
        }
    }
}

/// The same body twice applies once; a different body is refused.
///
/// The second receipt is the first one's counts and sequence with
/// `applied = false`, and the route was not reached again. A different
/// body under the same key is refused typed and the route is not reached
/// at all.
#[test]
fn a_replay_is_answered_from_the_record_and_a_conflict_is_refused() -> TestRes {
    let runtime = Arc::new(CountingRuntime {
        applies: AtomicUsize::new(0),
    });
    let catalog = Arc::new(MemoryIdempotencyCatalog::default());
    let dispatcher = dispatcher(Arc::clone(&runtime), Arc::clone(&catalog));
    let budget = RequestBudgetV1::unbounded();

    let first = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(dirty_batch("d1", "src/a.rs")),
        &budget,
    ))?;
    if !first.applied || first.durable_sequence != 1 || first.batch_digest != "d1" {
        return Err(format!("first publish must apply at sequence 1: {first:?}").into());
    }
    let replay = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(dirty_batch("d1", "src/a.rs")),
        &budget,
    ))?;
    if replay.applied || replay.durable_sequence != 1 {
        return Err(format!("replay must be the recorded apply, not a new one: {replay:?}").into());
    }
    if replay.accepted_replace_scopes != first.accepted_replace_scopes
        || replay.generation != first.generation
    {
        return Err("replay counts must be the original apply's".into());
    }
    if runtime.applies.load(Ordering::SeqCst) != 1 {
        return Err(format!(
            "the route ran {} times for one body",
            runtime.applies.load(Ordering::SeqCst)
        )
        .into());
    }

    let conflict = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(dirty_batch("d1", "src/b.rs")),
        &budget,
    );
    match typed_code_of(&conflict) {
        Some(code) if code == BATCH_DIGEST_CONFLICT_CODE => {}
        other => {
            return Err(format!(
                "a different body under the same digest must be refused typed, got {other:?}"
            )
            .into());
        }
    }
    if runtime.applies.load(Ordering::SeqCst) != 1 {
        return Err("a conflicting body must not reach the route".into());
    }
    if catalog.records() != 1 {
        return Err(format!("one key must hold one record, found {}", catalog.records()).into());
    }

    let second = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(dirty_batch("d2", "src/b.rs")),
        &budget,
    ))?;
    if !second.applied || second.durable_sequence != 2 {
        return Err(format!("a new key applies at the next sequence: {second:?}").into());
    }
    Ok(())
}
