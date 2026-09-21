//! QI-BB-032 / QI-BB-029 — the dispatcher's idempotency protocol.
//!
//! Every receipt-bearing route runs under its idempotency record in the
//! order digest verification → preflight → intent → apply → finalize,
//! proven at the dispatcher: a replay is answered from the record, a forged
//! digest and a refused batch leave no record, and a resumed sealed batch
//! finalizes without re-embedding.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use quanta_index_contract::lex::DirtyRecord;
use quanta_index_contract::{
    BatchPublishReceipt, ChunkId, DirtyIngestBatch, DirtyMutation, FileContributorIngestBatch,
    FileOwnershipIngestBatch, HistoryIngestBatch, IngestOperationKindV1, ManifestGeneration,
    RepoCommitRecencyIngestBatch, RepoDescriptionIngestBatch, RepoId, RepoMapSourceBundle,
    RepoMetaIngestBatch, RepoTopicIngestBatch, RevisionId, RuntimeCatalogIngestBatch,
    SearchCorpusIngestBatch, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse,
    StructuralIngestBatch,
};
use quanta_index_core::{
    BATCH_DIGEST_MISMATCH_CODE, CoreError, FileContributorIngestPort, FileOwnershipIngestPort,
    IdempotencyKeyV1, IngestBatchBodyV1 as _, IngestResourcePolicy, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMapBundleIngestPort, RepoMapMutationReceiptV1,
    RepoMetaIngestPort, RepoTopicIngestPort, RequestBudgetV1, SearchCorpusIngestPort,
    SemanticIngestPort, SemanticStreamWindowPolicy, TextEmbeddingProvider,
};
use quanta_index_ipc::{canonical_batch_digest_v1, stamp_batch_digest_v1};

use super::support::{
    CountingEmbedder, FakeSearchCorpusBuilder, FakeSemanticBuilder, MemoryIdempotencyCatalog,
    RecordingSearchCorpusAuthority, always_valid_generation, fixture_search_corpus_batch,
    memory_aux_catalog, memory_catalog, no_storage_sealed_reclaim,
    test_incomplete_generation_discard,
};
use crate::ingest_dispatcher::auxiliary::{AuxiliaryMutationCoordinator, dirty_publish_receipt_v1};
use crate::ingest_dispatcher::dispatcher::SearchPlaneIngestDispatcher;
use crate::ingest_dispatcher::ports::{
    HistoryIngestPort, RuntimeMetadataIngestPort, StructuralIngestPort,
};
use crate::ingest_dispatcher::search_corpus::{
    DirectSearchCorpusMaterializer, SearchCorpusMaterializerParts,
};
use crate::ingest_dispatcher::semantic::DirectSemanticMaterializer;
use crate::{Ledger, SEARCH_OWNED_SEMANTIC_DIMENSION, SnapshotRegistries, SnapshotRegistryPolicy};

type TestRes = Result<(), Box<dyn std::error::Error>>;

/// A route the test never expects to reach.
struct Unreachable;

fn unreachable_route(route: &str) -> CoreError {
    CoreError::Storage(format!("test: {route} route must not be reached"))
}

impl SearchCorpusIngestPort for Unreachable {
    fn preflight_batch(&self, _batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        Err(unreachable_route("search corpus preflight"))
    }

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
impl RuntimeMetadataIngestPort for Unreachable {
    fn publish_batch(&self, _batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("dirty"))
    }
    fn publish_catalog_batch(
        &self,
        _batch: &RuntimeCatalogIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("runtime catalog"))
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
    fn ingest_bundle(
        &self,
        _bundle: &RepoMapSourceBundle,
    ) -> Result<RepoMapMutationReceiptV1, CoreError> {
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

/// A dispatcher whose only reachable route is a real search-corpus
/// materializer over recording fakes, sharing `catalog` with the test.
fn search_corpus_dispatcher(
    materializer: DirectSearchCorpusMaterializer,
    catalog: Arc<MemoryIdempotencyCatalog>,
) -> SearchPlaneIngestDispatcher {
    let unreachable = Arc::new(Unreachable);
    SearchPlaneIngestDispatcher::new(
        Arc::new(materializer),
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        unreachable.clone(),
        unreachable,
        catalog,
    )
}

/// The recording fakes behind one search-corpus materializer.
struct SearchCorpusFakes {
    lexical_builder: Arc<FakeSearchCorpusBuilder>,
    semantic_builder: Arc<FakeSemanticBuilder>,
    embedder: Arc<CountingEmbedder>,
    authority: Arc<RecordingSearchCorpusAuthority>,
}

fn search_corpus_materializer(
    catalog: Arc<MemoryIdempotencyCatalog>,
    authority_exact: bool,
) -> (DirectSearchCorpusMaterializer, SearchCorpusFakes) {
    let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let embedder = Arc::new(CountingEmbedder {
        dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
        calls: Mutex::new(0),
    });
    let authority = Arc::new(RecordingSearchCorpusAuthority {
        identities: Mutex::new(Vec::new()),
        exact: authority_exact,
    });
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
        Arc::new(DirectSemanticMaterializer::new(semantic_builder.clone()));
    let embedder_port: Arc<dyn TextEmbeddingProvider + Send + Sync> = embedder.clone();
    let materializer = DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
        SearchCorpusMaterializerParts {
            builder: lexical_builder.clone(),
            ledger: Arc::new(RwLock::new(Ledger::new())),
            semantic_ingest: semantic_materializer,
            semantic_embedder: embedder_port,
            authority: authority.clone(),
            lexical_generation_validator: always_valid_generation(),
            semantic_generation_validator: always_valid_generation(),
            semantic_content_roots:
                crate::content_roots_test_support::generation_keyed_content_roots(),
            lexical_incomplete_discard: test_incomplete_generation_discard(),
            semantic_incomplete_discard: test_incomplete_generation_discard(),
            lexical_reclaim: no_storage_sealed_reclaim(),
            semantic_reclaim: no_storage_sealed_reclaim(),
            snapshots: SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
            idempotency: catalog,
            resource_policy: IngestResourcePolicy::DEFAULT,
            semantic_stream_policy: SemanticStreamWindowPolicy::DEFAULT,
            auxiliary_catalog: memory_aux_catalog(),
            auxiliary_coordinator: AuxiliaryMutationCoordinator::shared(),
        },
    );
    (
        materializer,
        SearchCorpusFakes {
            lexical_builder,
            semantic_builder,
            embedder,
            authority,
        },
    )
}

/// A dirty batch carrying its canonical digest.
fn dirty_batch(doc: &str) -> Result<DirtyIngestBatch, Box<dyn std::error::Error>> {
    let mut batch = DirtyIngestBatch {
        repo_id: RepoId::new("repo-idem").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-idem")
            .expect("static fixture ID satisfies canonical policy"),
        generation: ManifestGeneration::new(4),
        overlay_epoch_ms: 11,
        batch_digest: String::new(),
        entries: vec![DirtyMutation::Upsert(DirtyRecord {
            wire_version: 1,
            doc_id: ChunkId::new(doc),
            applied_at_ms: 11,
            payload_hash: [3; 32],
        })],
    };
    stamp_batch_digest_v1(&mut batch)?;
    Ok(batch)
}

fn typed_code_of(
    response: &SearchPlaneIngestIpcResponse,
) -> Option<quanta_index_contract::SearchPlaneErrorCodeV2> {
    match response {
        SearchPlaneIngestIpcResponse::Error(error) => Some(error.code),
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
        SearchPlaneIngestIpcResponse::DirtyReceipt(receipt)
        | SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt) => Ok(receipt),
        SearchPlaneIngestIpcResponse::Error(error) => {
            Err(format!("{}: {}", error.code, error.message))
        }
        other @ (SearchPlaneIngestIpcResponse::HistoryReceipt(_)
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

/// The same body twice applies once; a forged digest is refused.
///
/// The second receipt is the first one's counts and sequence with
/// `applied = false`, and the route was not reached again. A different
/// body carrying the first body's digest is refused typed
/// `BATCH_DIGEST_MISMATCH` before the route or the record is touched; the
/// same different body under its own digest is a new key that applies.
#[test]
fn a_replay_is_answered_from_the_record_and_a_forged_digest_is_refused() -> TestRes {
    let runtime = Arc::new(CountingRuntime {
        applies: AtomicUsize::new(0),
    });
    let catalog = Arc::new(MemoryIdempotencyCatalog::default());
    let dispatcher = dispatcher(Arc::clone(&runtime), Arc::clone(&catalog));
    let budget = RequestBudgetV1::unbounded();

    let batch = dirty_batch("src/a.rs")?;
    let first = receipt_of(
        dispatcher.dispatch(SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch.clone()), &budget),
    )?;
    if !first.applied || first.durable_sequence != 1 || first.batch_digest != batch.batch_digest {
        return Err(format!("first publish must apply at sequence 1: {first:?}").into());
    }
    let replay = receipt_of(
        dispatcher.dispatch(SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch.clone()), &budget),
    )?;
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

    // A different body wearing the first body's digest.
    let mut forged = dirty_batch("src/b.rs")?;
    forged.batch_digest.clone_from(&batch.batch_digest);
    let refused =
        dispatcher.dispatch(SearchPlaneIngestIpcRequest::PublishDirtyBatch(forged), &budget);
    match typed_code_of(&refused) {
        Some(code) if code == BATCH_DIGEST_MISMATCH_CODE => {}
        other => {
            return Err(format!(
                "a body whose digest is not its own must be refused typed, got {other:?}"
            )
            .into());
        }
    }
    if runtime.applies.load(Ordering::SeqCst) != 1 {
        return Err("a forged digest must not reach the route".into());
    }
    if catalog.records() != 1 {
        return Err(format!(
            "a forged digest must leave the catalog untouched, found {} records",
            catalog.records()
        )
        .into());
    }

    let second = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(dirty_batch("src/b.rs")?),
        &budget,
    ))?;
    if !second.applied || second.durable_sequence != 2 {
        return Err(format!(
            "a new body under its own digest applies at the next sequence: {second:?}"
        )
        .into());
    }
    Ok(())
}

/// A batch whose digest has the canonical shape but the wrong value is
/// refused the same way: the shape check is the contract's, the value is
/// the dispatcher's.
#[test]
fn a_well_formed_but_wrong_digest_is_a_mismatch() -> TestRes {
    let runtime = Arc::new(CountingRuntime {
        applies: AtomicUsize::new(0),
    });
    let catalog = Arc::new(MemoryIdempotencyCatalog::default());
    let dispatcher = dispatcher(Arc::clone(&runtime), Arc::clone(&catalog));
    let mut batch = dirty_batch("src/a.rs")?;
    batch.batch_digest = "0".repeat(64);
    let refused = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch),
        &RequestBudgetV1::unbounded(),
    );
    if typed_code_of(&refused) != Some(BATCH_DIGEST_MISMATCH_CODE) {
        return Err(format!("expected BATCH_DIGEST_MISMATCH, got {refused:?}").into());
    }
    if runtime.applies.load(Ordering::SeqCst) != 0 || catalog.records() != 0 {
        return Err("nothing may run or be recorded for a mismatched digest".into());
    }
    Ok(())
}

/// QI-BB-029: a search-corpus batch the preflight refuses leaves the
/// idempotency catalog untouched.
///
/// The record is begun only after the batch has been admitted, and neither
/// builder nor the authority ran. Before this ordering, the record was
/// begun first and the refused batch left a durable in-progress row behind.
#[test]
fn a_refused_search_corpus_batch_leaves_no_record() -> TestRes {
    let catalog = memory_catalog();
    let (materializer, fakes) = search_corpus_materializer(Arc::clone(&catalog), false);
    let dispatcher = search_corpus_dispatcher(materializer, Arc::clone(&catalog));
    let budget = RequestBudgetV1::unbounded();

    // Mode/base mismatch, re-stamped so the digest is the body's and the
    // shape rule is what refuses it.
    let mut malformed = fixture_search_corpus_batch()?;
    malformed.base_generation = Some(ManifestGeneration::new(3));
    stamp_batch_digest_v1(&mut malformed)?;
    let refused = dispatcher
        .dispatch(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(malformed), &budget);
    if typed_code_of(&refused)
        != Some(quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusBatchShapeInvalid)
    {
        return Err(format!("expected a shape refusal, got {refused:?}").into());
    }
    if catalog.records() != 0 {
        return Err(format!(
            "a refused batch must leave no idempotency record, found {}",
            catalog.records()
        )
        .into());
    }
    let lexical_builds = fakes
        .lexical_builder
        .batches
        .lock()
        .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
        .len();
    let semantic_builds = fakes.semantic_builder.take()?.len();
    let recorded = fakes
        .authority
        .identities
        .lock()
        .map_err(|err| format!("recording authority poisoned: {err}"))?
        .len();
    if lexical_builds != 0 || semantic_builds != 0 || recorded != 0 {
        return Err(format!(
            "a refused batch must mutate nothing: lexical={lexical_builds} semantic={semantic_builds} authority={recorded}"
        )
        .into());
    }
    if *fakes
        .embedder
        .calls
        .lock()
        .map_err(|err| format!("counting embedder poisoned: {err}"))?
        != 0
    {
        return Err("a refused batch must not embed".into());
    }

    // The corrected batch under its own digest applies and is recorded.
    let corrected = fixture_search_corpus_batch()?;
    let receipt = receipt_of(
        dispatcher
            .dispatch(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(corrected), &budget),
    )?;
    if !receipt.applied || !receipt.sealed || catalog.records() != 1 {
        return Err(format!("the corrected batch must apply and be recorded: {receipt:?}").into());
    }
    Ok(())
}

/// A record left in progress by a crash after the sealed pair was
/// materialized resumes without re-embedding or rebuilding.
///
/// Both tracks and the authority already hold the exact identity, so the
/// apply is finalize-only, and the resumed publish is recorded as the
/// apply.
#[test]
fn a_resumed_sealed_batch_finalizes_without_re_embedding() -> TestRes {
    let catalog = memory_catalog();
    let (materializer, fakes) = search_corpus_materializer(Arc::clone(&catalog), true);
    let dispatcher = search_corpus_dispatcher(materializer, Arc::clone(&catalog));
    let budget = RequestBudgetV1::unbounded();

    // The crash left the record begun under the body's digest and never
    // finalized, while the tracks and the authority are already exact.
    let mut batch = fixture_search_corpus_batch()?;
    let body_sha256 = canonical_batch_digest_v1(&mut batch)?;
    catalog.seed_in_progress(
        IdempotencyKeyV1 {
            kind: IngestOperationKindV1::SearchCorpus,
            repo_id: batch.repo_id().clone(),
            revision_id: batch.revision_id().clone(),
            generation: batch.generation(),
            batch_digest: batch.batch_digest().to_string(),
        },
        body_sha256,
    )?;

    let resumed =
        receipt_of(dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
            &budget,
        ))?;
    if !resumed.applied || !resumed.sealed || resumed.durable_sequence != 1 {
        return Err(format!("the resumed publish must finalize as the apply: {resumed:?}").into());
    }
    let embed_calls = *fakes
        .embedder
        .calls
        .lock()
        .map_err(|err| format!("counting embedder poisoned: {err}"))?;
    let lexical_builds = fakes
        .lexical_builder
        .batches
        .lock()
        .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
        .len();
    let semantic_builds = fakes.semantic_builder.take()?.len();
    if embed_calls != 0 || lexical_builds != 0 || semantic_builds != 0 {
        return Err(format!(
            "a resume over an existing durable end state must not re-materialize: embed_calls={embed_calls} lexical={lexical_builds} semantic={semantic_builds}"
        )
        .into());
    }
    let replay = receipt_of(
        dispatcher.dispatch(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch), &budget),
    )?;
    if replay.applied || replay.durable_sequence != 1 {
        return Err(
            format!("after the resume, a replay is answered from the record: {replay:?}").into()
        );
    }
    Ok(())
}
