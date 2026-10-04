//! QI-BB-032 / QI-BB-029 — the dispatcher's idempotency protocol.
//!
//! Every receipt-bearing route runs under its idempotency record in the
//! order digest verification → preflight → intent → apply → finalize,
//! proven at the dispatcher: a replay is answered from the record, a forged
//! digest and a refused batch leave no record, and a resumed sealed batch
//! finalizes without re-embedding.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::Duration;

use quanta_index_contract::lex::DirtyRecord;
use quanta_index_contract::{
    BatchPublishReceipt, ChunkId, DirtyIngestBatch, DirtyMutation, FileContributorIngestBatch,
    FileOwnershipIngestBatch, HistoryIngestBatch, IngestOperationKindV1, ManifestGeneration,
    RepoCommitRecencyIngestBatch, RepoDescriptionIngestBatch, RepoId, RepoMapExactnessSummary,
    RepoMapGraphCoverage, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability,
    RepoMapMutationAck, RepoMapMutationPhaseV2, RepoMapPublishBundleRequestV2,
    RepoMapRedactionState, RepoMapSourceBundle, RepoMapTerminalReceiptV2, RepoMetaIngestBatch,
    RepoTopicIngestBatch, RevisionId, RuntimeCatalogIngestBatch, SearchCorpusIngestBatch,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, StructuralIngestBatch,
};
use quanta_index_core::{
    BATCH_DIGEST_MISMATCH_CODE, CoreError, FileContributorIngestPort, FileOwnershipIngestPort,
    IdempotencyKeyV1, IngestBatchBodyV1 as _, IngestResourcePolicy, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMapBundleIngestPort, RepoMetaIngestPort, RepoTopicIngestPort,
    RequestBudgetV1, RequestProviderStageV1, RequestStageDiagnosticPortV1, SearchCorpusIngestPort,
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

#[derive(Debug, Default)]
struct RecordingProviderStages(Mutex<Vec<RequestProviderStageV1>>);

impl RequestStageDiagnosticPortV1 for RecordingProviderStages {
    fn record_provider_stage_v1(&self, stage: RequestProviderStageV1) {
        self.0.lock().expect("provider stage recorder").push(stage);
    }
}

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
        _budget: &RequestBudgetV1,
    ) -> Result<quanta_index_contract::SearchCorpusPublishOutcome, CoreError> {
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
        _request: &RepoMapPublishBundleRequestV2,
    ) -> Result<RepoMapTerminalReceiptV2, CoreError> {
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

fn dispatcher<R: RuntimeMetadataIngestPort + Send + Sync + 'static>(
    runtime: Arc<R>,
    catalog: Arc<MemoryIdempotencyCatalog>,
) -> SearchPlaneIngestDispatcher {
    let unreachable = Arc::new(Unreachable);
    let source_publication = catalog.source_publication.clone();
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
        source_publication,
        Arc::new(super::support::RecordingSearchCorpusAuthority {
            exact: true,
            ..Default::default()
        }),
    )
}

struct RefusingRuntime {
    error: CoreError,
}

impl RuntimeMetadataIngestPort for RefusingRuntime {
    fn publish_batch(&self, _batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        Err(self.error.clone())
    }

    fn publish_catalog_batch(
        &self,
        _batch: &RuntimeCatalogIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        Err(unreachable_route("runtime catalog"))
    }
}

/// A dispatcher whose only reachable route is a real search-corpus
/// materializer over recording fakes, sharing `catalog` with the test.
fn search_corpus_dispatcher(
    materializer: DirectSearchCorpusMaterializer,
    catalog: Arc<MemoryIdempotencyCatalog>,
) -> SearchPlaneIngestDispatcher {
    search_corpus_dispatcher_with_port(Arc::new(materializer), catalog)
}

/// The same dispatcher with an explicit search-corpus port: counting
/// wrappers and unreachable routes share this constructor.
fn search_corpus_dispatcher_with_port(
    lexical: Arc<dyn SearchCorpusIngestPort + Send + Sync>,
    catalog: Arc<MemoryIdempotencyCatalog>,
) -> SearchPlaneIngestDispatcher {
    search_corpus_dispatcher_with_authority(
        lexical,
        catalog,
        Arc::new(super::support::RecordingSearchCorpusAuthority {
            exact: true,
            ..Default::default()
        }),
    )
}

fn search_corpus_dispatcher_with_authority(
    lexical: Arc<dyn SearchCorpusIngestPort + Send + Sync>,
    catalog: Arc<MemoryIdempotencyCatalog>,
    authority: Arc<dyn crate::ingest_dispatcher::SearchCorpusAuthorityInspectPort>,
) -> SearchPlaneIngestDispatcher {
    let unreachable = Arc::new(Unreachable);
    let source_publication = catalog.source_publication.clone();
    SearchPlaneIngestDispatcher::new(
        lexical,
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
        source_publication,
        authority,
    )
}

/// A search-corpus port that counts preflight and apply entries around
/// the real materializer: the oracle for "a replay runs no preflight".
struct CountingSearchCorpus {
    inner: DirectSearchCorpusMaterializer,
    preflights: AtomicUsize,
    applies: AtomicUsize,
}

impl CountingSearchCorpus {
    fn new(inner: DirectSearchCorpusMaterializer) -> Self {
        Self {
            inner,
            preflights: AtomicUsize::new(0),
            applies: AtomicUsize::new(0),
        }
    }

    fn preflights(&self) -> usize {
        self.preflights.load(Ordering::SeqCst)
    }

    fn applies(&self) -> usize {
        self.applies.load(Ordering::SeqCst)
    }
}

impl SearchCorpusIngestPort for CountingSearchCorpus {
    fn preflight_batch(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        let _prior = self.preflights.fetch_add(1, Ordering::SeqCst);
        self.inner.preflight_batch(batch)
    }

    fn publish_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_contract::SearchCorpusPublishOutcome, CoreError> {
        let _prior = self.applies.fetch_add(1, Ordering::SeqCst);
        self.inner.publish_batch(batch, budget)
    }
}

/// The recording fakes behind one search-corpus materializer.
struct SearchCorpusFakes {
    lexical_builder: Arc<FakeSearchCorpusBuilder>,
    semantic_builder: Arc<FakeSemanticBuilder>,
    embedder: Arc<CountingEmbedder>,
    authority: Arc<RecordingSearchCorpusAuthority>,
    ledger: Arc<RwLock<Ledger>>,
}

fn search_corpus_materializer(
    catalog: Arc<MemoryIdempotencyCatalog>,
    authority_exact: bool,
    build_from_unsealed: bool,
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
    let ledger = Arc::new(RwLock::new(Ledger::new()));
    let materializer = DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
        SearchCorpusMaterializerParts {
            builder: lexical_builder.clone(),
            ledger: ledger.clone(),
            semantic_ingest: semantic_materializer,
            semantic_embedder: embedder_port,
            authority: authority.clone(),
            lexical_generation_validator: if build_from_unsealed {
                crate::ingest_dispatcher::tests::support::build_then_valid_generation()
            } else {
                always_valid_generation()
            },
            semantic_generation_validator: if build_from_unsealed {
                crate::ingest_dispatcher::tests::support::build_then_valid_generation()
            } else {
                always_valid_generation()
            },
            semantic_content_roots:
                crate::content_roots_test_support::generation_keyed_content_roots(),
            lexical_incomplete_discard: test_incomplete_generation_discard(),
            semantic_incomplete_discard: test_incomplete_generation_discard(),
            lexical_reclaim: no_storage_sealed_reclaim(),
            semantic_reclaim: no_storage_sealed_reclaim(),
            snapshots: SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
            source_publication: catalog.source_publication.clone(),
            idempotency: catalog,
            resource_policy: IngestResourcePolicy::DEFAULT,
            semantic_stream_policy: SemanticStreamWindowPolicy::DEFAULT,
            source_egress_policy: None,
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
            ledger,
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

#[test]
fn ingest_claims_use_absolute_deadlines_on_batch_and_repomap_paths() -> TestRes {
    let catalog = memory_catalog();
    let runtime = Arc::new(CountingRuntime {
        applies: AtomicUsize::new(0),
    });
    let batch = dirty_batch("src/lease.rs")?;
    let batch_key = IdempotencyKeyV1 {
        kind: IngestOperationKindV1::Dirty,
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id.clone(),
        generation: batch.generation,
        batch_digest: batch.batch_digest.clone(),
    };
    let before = quanta_index_core::now_unix_ms();
    let _receipt = receipt_of(dispatcher(runtime, Arc::clone(&catalog)).dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch),
        &RequestBudgetV1::unbounded(),
    ))?;
    let after = quanta_index_core::now_unix_ms();
    assert_claim_deadline(&catalog, &batch_key, before, after)?;

    let repomap = Arc::new(CountingRepoMap {
        calls: AtomicUsize::new(0),
    });
    let request = RepoMapPublishBundleRequestV2::new(repomap_bundle_fixture())?;
    let repomap_key = IdempotencyKeyV1 {
        kind: IngestOperationKindV1::RepoMapBundle,
        repo_id: request.bundle.repo_id.clone(),
        revision_id: request.bundle.revision_id.clone(),
        generation: request.bundle.manifest_generation,
        batch_digest: request.source_bundle_digest.clone(),
    };
    let before = quanta_index_core::now_unix_ms();
    let _receipt = repomap_receipt_of(repomap_dispatcher(repomap, Arc::clone(&catalog)).dispatch(
        SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(request),
        &RequestBudgetV1::unbounded(),
    ))?;
    let after = quanta_index_core::now_unix_ms();
    assert_claim_deadline(&catalog, &repomap_key, before, after)
}

fn assert_claim_deadline(
    catalog: &MemoryIdempotencyCatalog,
    key: &IdempotencyKeyV1,
    before: u64,
    after: u64,
) -> TestRes {
    let observed = catalog.lease_deadline_ms(key)?;
    if observed < before.saturating_add(30_000) || observed > after.saturating_add(30_000) {
        return Err(format!(
            "claim deadline must be 30 seconds after the publish instant, got {observed} for wall-clock range {before}..={after}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn failed_terminal_refusal_write_is_reported() -> TestRes {
    use quanta_index_core::{IdempotencyCatalogPort as _, OperationInspectV1};

    let catalog = memory_catalog();
    catalog.fail_next_refusal();
    let dispatcher = dispatcher(
        Arc::new(RefusingRuntime {
            error: CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest,
                message: "injected frozen policy refusal".to_string(),
            },
        }),
        Arc::clone(&catalog),
    );
    let batch = dirty_batch("src/refusal.rs")?;
    let key = IdempotencyKeyV1 {
        kind: IngestOperationKindV1::Dirty,
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id.clone(),
        generation: batch.generation,
        batch_digest: batch.batch_digest.clone(),
    };
    let first = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch),
        &RequestBudgetV1::unbounded(),
    );
    if typed_code_of(&first) != Some(quanta_index_contract::SearchPlaneErrorCodeV2::Internal)
        || !matches!(catalog.inspect(&key)?, OperationInspectV1::InFlight { .. })
    {
        return Err(format!("failed refusal write was hidden: {first:?}").into());
    }
    Ok(())
}

#[test]
fn failed_uncertain_state_write_is_reported() -> TestRes {
    use quanta_index_core::{IdempotencyCatalogPort as _, OperationInspectV1};

    let catalog = memory_catalog();
    catalog.fail_next_uncertain();
    let dispatcher = dispatcher(
        Arc::new(RefusingRuntime {
            error: CoreError::InvalidContract("injected apply error".to_string()),
        }),
        Arc::clone(&catalog),
    );
    let batch = dirty_batch("src/uncertain.rs")?;
    let key = IdempotencyKeyV1 {
        kind: IngestOperationKindV1::Dirty,
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id.clone(),
        generation: batch.generation,
        batch_digest: batch.batch_digest.clone(),
    };
    let response = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch),
        &RequestBudgetV1::unbounded(),
    );
    if !matches!(&response, SearchPlaneIngestIpcResponse::Error(error)
        if error.code == quanta_index_contract::SearchPlaneErrorCodeV2::Internal
            && error.message.contains("injected uncertain-state write failure"))
        || !matches!(catalog.inspect(&key)?, OperationInspectV1::InFlight { .. })
    {
        return Err(format!("failed uncertain-state write was hidden: {response:?}").into());
    }
    Ok(())
}

#[test]
fn internal_contract_error_does_not_become_a_frozen_refusal() -> TestRes {
    use quanta_index_core::{IdempotencyCatalogPort as _, OperationInspectV1};

    let catalog = memory_catalog();
    let dispatcher = dispatcher(
        Arc::new(RefusingRuntime {
            error: CoreError::InvalidContract("injected route contract error".to_string()),
        }),
        Arc::clone(&catalog),
    );
    let batch = dirty_batch("src/contract.rs")?;
    let key = IdempotencyKeyV1 {
        kind: IngestOperationKindV1::Dirty,
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id.clone(),
        generation: batch.generation,
        batch_digest: batch.batch_digest.clone(),
    };
    let response = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch),
        &RequestBudgetV1::unbounded(),
    );
    if typed_code_of(&response)
        != Some(quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest)
        || !matches!(catalog.inspect(&key)?, OperationInspectV1::Uncertain { .. })
    {
        return Err(format!("internal contract error was frozen: {response:?}").into());
    }
    Ok(())
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
        | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => None,
    }
}

fn receipt_of(response: SearchPlaneIngestIpcResponse) -> Result<BatchPublishReceipt, String> {
    match response {
        SearchPlaneIngestIpcResponse::DirtyReceipt(receipt) => Ok(receipt),
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(outcome) => Ok(outcome.receipt),
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
        | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
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
    let first = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch.clone()),
        &budget,
    ))?;
    if !first.applied || first.durable_sequence != 1 || first.batch_digest != batch.batch_digest {
        return Err(format!("first publish must apply at sequence 1: {first:?}").into());
    }
    let replay = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch.clone()),
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

    // A different body wearing the first body's digest.
    let mut forged = dirty_batch("src/b.rs")?;
    forged.batch_digest.clone_from(&batch.batch_digest);
    let refused = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(forged),
        &budget,
    );
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

/// Reject a malformed source batch before mutable work.
///
/// Canonical source publication validates its complete intrinsic contract
/// before journaling, reservation, preflight or building. A corrected body
/// permits a fresh original publication.
#[test]
fn malformed_source_batch_refuses_before_journal_or_preflight() -> TestRes {
    let catalog = memory_catalog();
    let (materializer, fakes) = search_corpus_materializer(Arc::clone(&catalog), false, false);
    let counting = Arc::new(CountingSearchCorpus::new(materializer));
    let dispatcher = search_corpus_dispatcher_with_port(counting.clone(), Arc::clone(&catalog));
    let budget = RequestBudgetV1::unbounded();

    // Mode/base mismatch, re-stamped so the digest is the body's and the
    // shape rule is what refuses it.
    let mut malformed = fixture_search_corpus_batch()?;
    malformed.base_generation = Some(ManifestGeneration::new(3));
    stamp_batch_digest_v1(&mut malformed)?;
    let refused = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(malformed.clone()),
        &budget,
    );
    if typed_code_of(&refused)
        != Some(quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest)
    {
        return Err(format!("expected a shape refusal, got {refused:?}").into());
    }
    if counting.preflights() != 0 {
        return Err("intrinsic refusal must precede mutable preflight".into());
    }
    // No durable intent exists for an intrinsically invalid source event.
    if catalog.records() != 0 {
        return Err(format!(
            "intrinsic refusal must create no journal record, found {}",
            catalog.records()
        )
        .into());
    }
    if counting.applies() != 0 {
        return Err("a refused batch must never reach apply".into());
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

    // Revalidation is pure on every retry.
    let replayed = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(malformed),
        &budget,
    );
    if typed_code_of(&replayed)
        != Some(quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest)
    {
        return Err(format!("the retry must replay the refusal, got {replayed:?}").into());
    }
    if counting.preflights() != 0 || counting.applies() != 0 || catalog.records() != 0 {
        return Err(format!(
            "a refused replay runs nothing: preflights={} applies={} records={}",
            counting.preflights(),
            counting.applies(),
            catalog.records()
        )
        .into());
    }

    // The corrected batch under its own digest applies and is recorded.
    let corrected = fixture_search_corpus_batch()?;
    let receipt = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(corrected),
        &budget,
    ))?;
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
    let (materializer, fakes) = search_corpus_materializer(Arc::clone(&catalog), true, false);
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

    let resumed = receipt_of(dispatcher.dispatch(
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
    let replay = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
        &budget,
    ))?;
    if replay.applied || replay.durable_sequence != 1 {
        return Err(
            format!("after the resume, a replay is answered from the record: {replay:?}").into(),
        );
    }
    Ok(())
}

/// A committed replay runs no preflight, no apply and no storage: the
/// stored receipt answers verbatim.
///
/// The replay goes through a second dispatcher whose routes are all
/// unreachable and whose config differs — any preflight, provider or
/// storage contact would explode there instead of answering.
#[test]
fn a_committed_replay_runs_no_preflight_apply_or_storage() -> TestRes {
    let catalog = memory_catalog();
    let (materializer, fakes) = search_corpus_materializer(Arc::clone(&catalog), true, false);
    let counting = Arc::new(CountingSearchCorpus::new(materializer));
    let dispatcher = search_corpus_dispatcher_with_port(counting.clone(), Arc::clone(&catalog));
    let stages = Arc::new(RecordingProviderStages::default());
    let budget = RequestBudgetV1::unbounded().with_diagnostics(stages.clone());

    let batch = fixture_search_corpus_batch()?;
    let first_response = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
        &budget,
    );
    let SearchPlaneIngestIpcResponse::SearchCorpusReceipt(first) = first_response else {
        return Err("expected observed first corpus receipt".into());
    };
    let first_observation = first
        .observation
        .as_ref()
        .ok_or("first observation missing")?;
    if first_observation.status != quanta_index_contract::IngestObservationStatus::FinalizeOnly
        || first_observation.semantic.is_some()
        || first_observation.lexical_build_ns.is_some()
        || first_observation.finalize_ns.is_none()
    {
        return Err(format!("unexpected finalize-only observation: {first_observation:?}").into());
    }
    if !first.applied || first.durable_sequence != 1 {
        return Err(format!("the first publish must apply at sequence 1: {first:?}").into());
    }
    if counting.preflights() != 1 || counting.applies() != 1 {
        return Err("the first publish runs preflight and apply exactly once".into());
    }
    let first_stages = stages
        .0
        .lock()
        .map_err(|error| format!("provider stage recorder: {error}"))?
        .clone();
    if !first_stages.is_empty() {
        return Err(format!(
            "finalize-only publish must not emit provider stages: {first_stages:?}"
        )
        .into());
    }
    let builds_after_apply = fakes
        .lexical_builder
        .batches
        .lock()
        .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
        .len();

    let replay_response = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
        &budget,
    );
    let SearchPlaneIngestIpcResponse::SearchCorpusReceipt(replay) = replay_response else {
        return Err("expected observed replay corpus receipt".into());
    };
    let replay_observation = replay
        .observation
        .as_ref()
        .ok_or("replay observation missing")?;
    replay_observation.validate_for(
        budget.response_request_id(),
        &batch,
        &replay.publication,
        &replay.receipt,
    )?;
    if replay_observation.status != quanta_index_contract::IngestObservationStatus::Replayed
        || replay_observation.semantic.is_some()
        || replay_observation.lexical_build_ns.is_some()
        || replay_observation.finalize_ns.is_some()
    {
        return Err(
            format!("cached/zero-filled replay measurement: {replay_observation:?}").into(),
        );
    }
    if replay.applied
        || replay.durable_sequence != 1
        || replay.accepted_replace_scopes != first.accepted_replace_scopes
        || replay.generation != first.generation
    {
        return Err(format!("the replay must be the stored receipt: {replay:?}").into());
    }
    if counting.preflights() != 1 || counting.applies() != 1 {
        return Err(format!(
            "a committed replay runs no preflight and no apply: preflights={} applies={}",
            counting.preflights(),
            counting.applies()
        )
        .into());
    }
    let builds = fakes
        .lexical_builder
        .batches
        .lock()
        .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
        .len();
    if builds != builds_after_apply {
        return Err(format!(
            "the replay must not rebuild: builds={builds} after-apply={builds_after_apply}"
        )
        .into());
    }
    if *stages
        .0
        .lock()
        .map_err(|error| format!("provider stage recorder: {error}"))?
        != first_stages
    {
        return Err("journal replay must not issue provider stages".into());
    }

    // A dispatcher whose routes cannot serve anything still answers the
    // replay from the journal alone.
    let unreachable = Arc::new(Unreachable);
    let cold = search_corpus_dispatcher_with_port(unreachable, Arc::clone(&catalog));
    let cold_replay = receipt_of(cold.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
        &budget,
    ))?;
    if cold_replay != replay.receipt {
        return Err(format!(
            "a route-less dispatcher must answer the identical stored receipt: {cold_replay:?}"
        )
        .into());
    }
    if *stages
        .0
        .lock()
        .map_err(|error| format!("provider stage recorder: {error}"))?
        != first_stages
    {
        return Err("cold journal replay must not issue provider stages".into());
    }
    Ok(())
}

/// Pause after actual source materialization has returned but before the
/// dispatcher commits the journal.
///
/// Cancelling the transport budget here must not turn a completed, admitted
/// mutation into a rollback claim.
#[test]
fn cancelled_peer_after_admitted_apply_still_commits_and_replays_exactly() -> TestRes {
    use quanta_index_core::{IdempotencyCatalogPort as _, OperationInspectV1};

    struct PauseAfterApply {
        inner: DirectSearchCorpusMaterializer,
        entered: mpsc::Sender<bool>,
        release: Mutex<mpsc::Receiver<()>>,
        applies: AtomicUsize,
    }

    impl SearchCorpusIngestPort for PauseAfterApply {
        fn preflight_batch(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
            self.inner.preflight_batch(batch)
        }

        fn publish_batch(
            &self,
            batch: &SearchCorpusIngestBatch,
            budget: &RequestBudgetV1,
        ) -> Result<quanta_index_contract::SearchCorpusPublishOutcome, CoreError> {
            let _prior = self.applies.fetch_add(1, Ordering::SeqCst);
            let outcome = self.inner.publish_batch(batch, budget);
            self.entered.send(outcome.is_ok()).map_err(|error| {
                CoreError::Storage(format!("post-apply fixture lost observer: {error}"))
            })?;
            self.release
                .lock()
                .map_err(|error| CoreError::Storage(format!("fixture gate poisoned: {error}")))?
                .recv_timeout(Duration::from_secs(5))
                .map_err(|error| {
                    CoreError::Storage(format!("post-apply gate not released: {error}"))
                })?;
            outcome
        }
    }

    let catalog = memory_catalog();
    let (inner, _fakes) = search_corpus_materializer(Arc::clone(&catalog), true, false);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let port = Arc::new(PauseAfterApply {
        inner,
        entered: entered_tx,
        release: Mutex::new(release_rx),
        applies: AtomicUsize::new(0),
    });
    let dispatcher = search_corpus_dispatcher_with_port(port.clone(), Arc::clone(&catalog));
    let batch = fixture_search_corpus_batch()?;
    let key = IdempotencyKeyV1 {
        kind: IngestOperationKindV1::SearchCorpus,
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id.clone(),
        generation: batch.generation,
        batch_digest: batch.batch_digest.clone(),
    };
    let budget = RequestBudgetV1::unbounded();
    let cancel = budget.cancel_handle();
    let first_batch = batch.clone();
    let first = std::thread::spawn(move || {
        dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(first_batch),
            &budget,
        )
    });
    let materialized = entered_rx.recv_timeout(Duration::from_secs(5))?;
    let in_flight = catalog.inspect(&key)?;
    cancel.cancel();
    release_tx.send(())?;
    let first_response = first.join().map_err(|panic| {
        let detail = panic
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic payload");
        format!("publish thread panicked: {detail}")
    })?;
    if !materialized || !matches!(in_flight, OperationInspectV1::InFlight { .. }) {
        return Err(
            format!("fixture did not reach admitted post-apply state: {in_flight:?}").into(),
        );
    }
    let receipt = receipt_of(first_response)?;
    if !receipt.applied || receipt.durable_sequence == 0 {
        return Err(
            format!("cancelled peer lost the admitted terminal receipt: {receipt:?}").into(),
        );
    }
    if !matches!(catalog.inspect(&key)?, OperationInspectV1::Committed { .. }) {
        return Err("admitted publish did not settle committed".into());
    }
    let replay = receipt_of(
        search_corpus_dispatcher_with_port(port.clone(), Arc::clone(&catalog)).dispatch(
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
            &RequestBudgetV1::unbounded(),
        ),
    )?;
    if replay != receipt.replayed() || port.applies.load(Ordering::SeqCst) != 1 {
        return Err(format!("exact replay re-applied or changed receipt: {replay:?}").into());
    }
    Ok(())
}

#[test]
fn ingest_dispatch_carries_provider_window_stages_and_replay_does_not() -> TestRes {
    let catalog = memory_catalog();
    let (materializer, fakes) = search_corpus_materializer(Arc::clone(&catalog), false, true);
    let dispatcher = search_corpus_dispatcher(materializer, Arc::clone(&catalog));
    let stages = Arc::new(RecordingProviderStages::default());
    let budget = RequestBudgetV1::unbounded().with_diagnostics(stages.clone());
    let batch = fixture_search_corpus_batch()?;

    let first = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
        &budget,
    ))?;
    if !first.applied {
        return Err(format!("first ingest publish must apply: {first:?}").into());
    }
    let first_stages = stages
        .0
        .lock()
        .map_err(|error| format!("provider stage recorder: {error}"))?
        .clone();
    if first_stages
        != [
            RequestProviderStageV1::IngestWindowStarted { window_ordinal: 1 },
            RequestProviderStageV1::IngestWindowReturned { window_ordinal: 1 },
        ]
    {
        return Err(format!("provider window stages differ: {first_stages:?}").into());
    }
    let calls = *fakes
        .embedder
        .calls
        .lock()
        .map_err(|error| format!("provider call recorder: {error}"))?;
    if calls != 1 {
        return Err(format!("one ingest provider call expected, got {calls}").into());
    }

    let replay = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
        &budget,
    ))?;
    if replay.applied || replay.durable_sequence != first.durable_sequence {
        return Err(format!("journal replay must not apply again: {replay:?}").into());
    }
    if *stages
        .0
        .lock()
        .map_err(|error| format!("provider stage recorder: {error}"))?
        != first_stages
        || *fakes
            .embedder
            .calls
            .lock()
            .map_err(|error| format!("provider call recorder: {error}"))?
            != calls
    {
        return Err("journal replay must not re-enter the provider boundary".into());
    }
    Ok(())
}

/// A repo-map store double behind the journal: counts V2 applies and
/// answers a canned terminal receipt built from the request.
struct CountingRepoMap {
    calls: AtomicUsize,
}

impl RepoMapBundleIngestPort for CountingRepoMap {
    fn ingest_bundle(
        &self,
        request: &RepoMapPublishBundleRequestV2,
    ) -> Result<RepoMapTerminalReceiptV2, CoreError> {
        let _prior = self.calls.fetch_add(1, Ordering::SeqCst);
        let bundle = &request.bundle;
        Ok(RepoMapTerminalReceiptV2 {
            phase: RepoMapMutationPhaseV2::Publish,
            mutation: RepoMapMutationAck {
                repo_id: bundle.repo_id.clone(),
                revision_id: bundle.revision_id.clone(),
                manifest_generation: bundle.manifest_generation,
                prior_candidate_commitment: None,
                new_candidate_commitment: "c".repeat(64),
                activation_epoch: 0,
                terminal_sequence: 41,
                replayed: false,
            },
            manifest_digest: bundle.manifest_digest.clone(),
            snapshot_id: bundle.snapshot_id.clone(),
            projection_version: bundle.projection_version,
            authority_digest: bundle.authority_digest.clone(),
            source_bundle_digest: request.source_bundle_digest.clone(),
        })
    }
}

fn repomap_dispatcher(
    repomap: Arc<CountingRepoMap>,
    catalog: Arc<MemoryIdempotencyCatalog>,
) -> SearchPlaneIngestDispatcher {
    let unreachable = Arc::new(Unreachable);
    let source_publication = catalog.source_publication.clone();
    SearchPlaneIngestDispatcher::new(
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
        repomap,
        catalog,
        source_publication,
        Arc::new(super::support::RecordingSearchCorpusAuthority {
            exact: true,
            ..Default::default()
        }),
    )
}

fn repomap_bundle_fixture() -> RepoMapSourceBundle {
    RepoMapSourceBundle::new(
        RepoId::new("repo-rm2").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("rev-rm2").expect("static fixture ID satisfies canonical policy"),
        ManifestGeneration::new(9),
        "m".repeat(64),
        "snap-rm2",
        3,
        "a".repeat(64),
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
}

fn repomap_receipt_of(
    response: SearchPlaneIngestIpcResponse,
) -> Result<RepoMapTerminalReceiptV2, String> {
    match response {
        SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(receipt) => Ok(receipt),
        SearchPlaneIngestIpcResponse::Error(error) => {
            Err(format!("{}: {}", error.code, error.message))
        }
        other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
        | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)) => {
            Err(format!("unexpected response {other:?}"))
        }
    }
}

/// SEP-21 P02B: a `RepoMap` V2 publish travels the operation journal —
/// exactly one store apply, then replays carry the stored terminal
/// receipt with zero store contact.
///
/// A forged source-bundle digest is refused before the journal or the
/// store is touched, and the journal-free V1 bundle arm records
/// nothing.
#[test]
fn a_repomap_v2_publish_journals_once_and_replays_without_the_store() -> TestRes {
    let catalog = memory_catalog();
    let repomap = Arc::new(CountingRepoMap {
        calls: AtomicUsize::new(0),
    });
    let dispatcher = repomap_dispatcher(Arc::clone(&repomap), Arc::clone(&catalog));
    let budget = RequestBudgetV1::unbounded();

    let request = RepoMapPublishBundleRequestV2::new(repomap_bundle_fixture())?;
    let records_before = catalog.records();
    let first = repomap_receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(request.clone()),
        &budget,
    ))?;
    if first.mutation.replayed || first.source_bundle_digest != request.source_bundle_digest {
        return Err(format!("the first V2 publish must apply: {first:?}").into());
    }
    if repomap.calls.load(Ordering::SeqCst) != 1 {
        return Err("the first V2 publish applies exactly once".into());
    }

    let replay = repomap_receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(request.clone()),
        &budget,
    ))?;
    if !replay.mutation.replayed
        || replay.mutation.new_candidate_commitment != first.mutation.new_candidate_commitment
    {
        return Err(format!("the V2 replay must be the stored receipt: {replay:?}").into());
    }
    if replay.mutation.terminal_sequence != first.mutation.terminal_sequence
        || replay.source_bundle_digest != first.source_bundle_digest
    {
        return Err("the V2 replay must carry the original terminal identity".into());
    }
    if repomap.calls.load(Ordering::SeqCst) != 1 {
        return Err(format!(
            "a V2 replay never reaches the store: calls={}",
            repomap.calls.load(Ordering::SeqCst)
        )
        .into());
    }

    // A forged digest is refused before the journal or the store.
    let mut forged = request.clone();
    forged.source_bundle_digest = format!("sha256:{}", "0".repeat(64));
    if forged.source_bundle_digest == request.source_bundle_digest {
        return Err("the forged digest fixture must differ".into());
    }
    let refused = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(forged),
        &budget,
    );
    if !matches!(
        &refused,
        SearchPlaneIngestIpcResponse::Error(error)
            if error.code == quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest
                && error.message.contains("source bundle digest mismatch")
    ) {
        return Err(format!(
            "a forged bundle digest must be refused as a mismatch, got {refused:?}"
        )
        .into());
    }
    if repomap.calls.load(Ordering::SeqCst) != 1 {
        return Err("a forged digest must not reach the store".into());
    }

    if catalog.records() != records_before + 1 {
        return Err(format!(
            "only the current publish journals: records={} before={records_before}",
            catalog.records()
        )
        .into());
    }
    Ok(())
}

#[test]
fn source_event_replay_with_new_target_returns_original_receipt_and_sequence() -> TestRes {
    let catalog = memory_catalog();
    let (materializer, _fakes) = search_corpus_materializer(Arc::clone(&catalog), true, false);
    let counting = Arc::new(CountingSearchCorpus::new(materializer));
    let dispatcher = search_corpus_dispatcher_with_port(counting.clone(), Arc::clone(&catalog));
    let budget = RequestBudgetV1::unbounded();
    let original = fixture_search_corpus_batch()?;
    let first = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(original.clone()),
        &budget,
    ))?;
    let mut replay = original;
    replay.revision_id =
        RevisionId::new("new-containing-revision").map_err(|error| error.to_string())?;
    replay.generation = ManifestGeneration::new(99);
    replay.manifest_digest = "new-target-manifest".into();
    stamp_batch_digest_v1(&mut replay)?;
    let response = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(replay.clone()),
        &budget,
    );
    let SearchPlaneIngestIpcResponse::SearchCorpusReceipt(outcome) = response else {
        return Err(format!("expected original source publication, got {response:?}").into());
    };
    let returned = &outcome.receipt;
    if *returned != first.clone().replayed()
        || counting.applies() != 1
        || counting.preflights() != 1
        || catalog.records() != 1
    {
        return Err(format!("source replay fabricated a receipt or repeated work: first={first:?}, replay={returned:?}").into());
    }
    // Returning the original journal fields alone is insufficient: the public
    // response must also satisfy the observation contract consumed by the SDK.
    // Keep this assertion active while the shared replay binding is repaired.
    outcome
        .observation
        .as_ref()
        .ok_or("source replay observation missing")?
        .validate_for(
            budget.response_request_id(),
            &replay,
            &outcome.publication,
            returned,
        )?;
    Ok(())
}

#[test]
fn source_event_payload_reuse_conflicts_before_any_second_apply() -> TestRes {
    let catalog = memory_catalog();
    let (materializer, _fakes) = search_corpus_materializer(Arc::clone(&catalog), true, false);
    let counting = Arc::new(CountingSearchCorpus::new(materializer));
    let dispatcher = search_corpus_dispatcher_with_port(counting.clone(), Arc::clone(&catalog));
    let budget = RequestBudgetV1::unbounded();
    let original = fixture_search_corpus_batch()?;
    let _first = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(original.clone()),
        &budget,
    ))?;
    let mut conflict = original;
    conflict.bundle_payload = Some(vec![1, 2, 3]);
    conflict.source_event.payload_sha256 =
        quanta_index_contract::source_event_payload_sha256(&conflict)?;
    stamp_batch_digest_v1(&mut conflict)?;
    let refused = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(conflict),
        &budget,
    );
    if typed_code_of(&refused)
        != Some(quanta_index_contract::SearchPlaneErrorCodeV2::BatchDigestConflict)
        || counting.applies() != 1
        || catalog.records() != 1
    {
        return Err(
            format!("different payload reused a source event or caused work: {refused:?}").into(),
        );
    }
    Ok(())
}

/// Neither dispatcher nor materializer may reserve before repair admission.
#[test]
fn source_event_repair_refusal_leaves_the_stream_available_for_a_new_publication() -> TestRes {
    use quanta_index_contract::{
        BatchIngestMode, GenerationSnapshot, SearchPlaneErrorCodeV2, SearchPlaneTrackKind,
    };
    use quanta_index_core::{GenerationIdentityValidatePort, SourcePublicationCatalogPort as _};

    struct CorruptTarget {
        manifest_generation: ManifestGeneration,
        track: SearchPlaneTrackKind,
    }
    impl GenerationIdentityValidatePort for CorruptTarget {
        fn validate_generation_identity(
            &self,
            candidate: &GenerationSnapshot,
        ) -> Result<(), CoreError> {
            if candidate.manifest_generation == self.manifest_generation
                && candidate.track == self.track
            {
                return Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    message: "injected target sidecar corruption".into(),
                });
            }
            Ok(())
        }
    }

    for track in [
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ] {
        let mut delta = fixture_search_corpus_batch()?;
        delta.mode = BatchIngestMode::Delta;
        delta.base_generation = Some(ManifestGeneration::new(6));
        super::support::restamp_search_corpus_fixture(&mut delta)?;
        delta.validate_v1()?;
        let catalog = memory_catalog();
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let validator = Arc::new(CorruptTarget {
            manifest_generation: delta.generation,
            track,
        });
        let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let embedder = Arc::new(CountingEmbedder {
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
            calls: Mutex::new(0),
        });
        let authority = Arc::new(RecordingSearchCorpusAuthority::default());
        let materializer = DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
            SearchCorpusMaterializerParts {
                builder: lexical_builder.clone(),
                ledger,
                semantic_ingest: Arc::new(DirectSemanticMaterializer::new(
                    semantic_builder.clone(),
                )),
                semantic_embedder: embedder.clone(),
                authority: authority.clone(),
                lexical_generation_validator: validator.clone(),
                semantic_generation_validator: validator,
                semantic_content_roots:
                    crate::content_roots_test_support::generation_keyed_content_roots(),
                lexical_incomplete_discard: test_incomplete_generation_discard(),
                semantic_incomplete_discard: test_incomplete_generation_discard(),
                lexical_reclaim: no_storage_sealed_reclaim(),
                semantic_reclaim: no_storage_sealed_reclaim(),
                snapshots: SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
                source_publication: catalog.source_publication.clone(),
                idempotency: catalog.clone(),
                resource_policy: IngestResourcePolicy::DEFAULT,
                semantic_stream_policy: SemanticStreamWindowPolicy::DEFAULT,
                source_egress_policy: None,
                auxiliary_catalog: memory_aux_catalog(),
                auxiliary_coordinator: AuxiliaryMutationCoordinator::shared(),
            },
        );
        let mut base = fixture_search_corpus_batch()?;
        base.generation = ManifestGeneration::new(6);
        base.manifest_digest = "manifest:base".into();
        super::support::restamp_search_corpus_fixture(&mut base)?;
        materializer.finalize_generation_v1(
            &base,
            Some(
                &crate::readiness::SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
                    &base.repo_id,
                    &base.revision_id,
                    [base.generation],
                ),
            ),
        )?;
        let counting = Arc::new(CountingSearchCorpus::new(materializer));
        let dispatcher = search_corpus_dispatcher_with_port(counting.clone(), catalog.clone());
        let budget = RequestBudgetV1::unbounded();
        let refused = dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(delta.clone()),
            &budget,
        );
        if typed_code_of(&refused)
            != Some(SearchPlaneErrorCodeV2::SearchCorpusGenerationRepairRequired)
            || catalog
                .source_publication
                .inspect_source_event(&delta.repo_id, &delta.source_event)?
                .is_some()
            || counting.applies() != 1
            || !lexical_builder
                .batches
                .lock()
                .map_err(|error| error.to_string())?
                .is_empty()
            || !semantic_builder.take()?.is_empty()
            || *embedder.calls.lock().map_err(|error| error.to_string())? != 0
            || !authority
                .identities
                .lock()
                .map_err(|error| error.to_string())?
                .is_empty()
        {
            return Err(format!(
                "{track:?} repair refusal consumed source authority or mutated a track: {refused:?}"
            )
            .into());
        }

        // Keep the same stream and expected base; a different full event can
        // use a healthy target because the refused Delta never owned a slot.
        let mut next = fixture_search_corpus_batch()?;
        next.generation = ManifestGeneration::new(8);
        next.source_event.event_id = "after-refused-repair".into();
        super::support::restamp_search_corpus_fixture(&mut next)?;
        let receipt = receipt_of(dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(next.clone()),
            &budget,
        ))?;
        let record = catalog
            .source_publication
            .inspect_source_event(&next.repo_id, &next.source_event)?
            .ok_or("new publication did not reserve its source event")?;
        if !receipt.applied
            || receipt.generation != next.generation
            || record.phase != quanta_index_core::SourceEventPhaseV1::Staged
            || record.binding.target.manifest_generation != next.generation
        {
            return Err("new publication did not stage after a repair refusal".into());
        }
    }
    Ok(())
}

#[test]
fn source_event_semantic_admission_refusal_leaves_no_reservation_or_track_work() -> TestRes {
    use quanta_index_core::SourcePublicationCatalogPort as _;

    let catalog = memory_catalog();
    let (materializer, fakes) = search_corpus_materializer(catalog.clone(), false, true);
    let counting = Arc::new(CountingSearchCorpus::new(materializer));
    let dispatcher = search_corpus_dispatcher_with_port(counting.clone(), catalog.clone());
    let mut batch = fixture_search_corpus_batch()?;
    batch
        .semantic_replace_scopes
        .first_mut()
        .ok_or("fixture semantic scope")?
        .scope_digest
        .clear();
    super::support::restamp_search_corpus_fixture(&mut batch)?;
    batch.validate_v1()?;
    batch
        .validate_surface_mutations_v1()
        .map_err(|error| error.to_string())?;

    let response = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
        &RequestBudgetV1::unbounded(),
    );
    if !matches!(&response, SearchPlaneIngestIpcResponse::Error(error)
        if error.message.contains("semantic source scope_digest must not be empty"))
        || catalog
            .source_publication
            .inspect_source_event(&batch.repo_id, &batch.source_event)?
            .is_some()
        || counting.applies() != 1
        || !fakes
            .lexical_builder
            .batches
            .lock()
            .map_err(|error| error.to_string())?
            .is_empty()
        || !fakes.semantic_builder.take()?.is_empty()
        || *fakes
            .embedder
            .calls
            .lock()
            .map_err(|error| error.to_string())?
            != 0
        || !fakes
            .authority
            .identities
            .lock()
            .map_err(|error| error.to_string())?
            .is_empty()
    {
        return Err(format!(
            "semantic admission failure reserved or mutated before refusal: {response:?}"
        )
        .into());
    }
    Ok(())
}

/// Failure after materialization is not proof that a source event never ran.
#[test]
fn completed_delta_recovers_after_retention_retires_base_before_journal_ack() -> TestRes {
    use quanta_index_core::{
        IdempotencyCatalogPort as _, OperationInspectV1, SourceEventPhaseV1,
        SourcePublicationCatalogPort as _,
    };
    struct FailAfterRetirement {
        inner: DirectSearchCorpusMaterializer,
        failures: AtomicUsize,
    }
    impl SearchCorpusIngestPort for FailAfterRetirement {
        fn preflight_batch(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
            self.inner.preflight_batch(batch)
        }
        fn publish_batch(
            &self,
            batch: &SearchCorpusIngestBatch,
            budget: &RequestBudgetV1,
        ) -> Result<quanta_index_contract::SearchCorpusPublishOutcome, CoreError> {
            let outcome = self.inner.publish_batch(batch, budget)?;
            if batch.base_generation.is_some() && self.failures.fetch_add(1, Ordering::SeqCst) == 0
            {
                let retention = crate::readiness::SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
                    &batch.repo_id, &batch.revision_id, [batch.generation],
                );
                self.inner.finalize_generation_v1(batch, Some(&retention))?;
                return Err(CoreError::Storage(
                    "injected failure after delta base retirement".into(),
                ));
            }
            Ok(outcome)
        }
    }
    let catalog = memory_catalog();
    let (inner, fakes) = search_corpus_materializer(catalog.clone(), true, false);
    let port = Arc::new(FailAfterRetirement {
        inner,
        failures: AtomicUsize::new(0),
    });
    let dispatcher = search_corpus_dispatcher_with_port(port.clone(), catalog.clone());
    let base = super::support::multi_scope_corpus_batch()?;
    let budget = RequestBudgetV1::unbounded();
    let receipt = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(base.clone()),
        &budget,
    ))?;
    let staged = catalog
        .source_publication
        .inspect_source_event(&base.repo_id, &base.source_event)?
        .ok_or("base source event absent")?;
    catalog.source_publication.activate_staged(
        &staged.binding,
        receipt
            .semantic_content
            .ok_or("sealed semantic content absent")?,
    )?;

    let mut delta = base.clone();
    delta.generation = ManifestGeneration::new(base.generation.get() + 1);
    delta.manifest_digest = "manifest:retained-delta".into();
    delta.base_generation = Some(base.generation);
    delta.mode = quanta_index_contract::BatchIngestMode::Delta;
    delta.source_event.expected_base_event_id = Some(base.source_event.event_id.clone());
    delta.source_event.event_id = "event-after-base".into();
    delta.replace_scopes.truncate(1);
    super::support::restamp_search_corpus_fixture(&mut delta)?;
    let request = SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(delta.clone());
    let failed = dispatcher.dispatch(request.clone(), &budget);
    if !matches!(&failed, SearchPlaneIngestIpcResponse::Error(error)
        if error.message.contains("injected failure after delta base retirement"))
    {
        return Err(format!("failure injection did not execute: {failed:?}").into());
    }
    let pending = catalog
        .source_publication
        .inspect_source_event(&delta.repo_id, &delta.source_event)?
        .ok_or("delta source event absent")?;
    if pending.phase != SourceEventPhaseV1::Pending
        || !matches!(
            catalog.inspect(&pending.binding.journal_key)?,
            OperationInspectV1::Uncertain { .. }
        )
    {
        return Err("delta must remain retryable under its original journal".into());
    }
    {
        let ledger = fakes.ledger.read().map_err(|e| e.to_string())?;
        if ledger
            .structural_state(&base.repo_id, &base.revision_id, base.generation)
            .is_some()
            || ledger
                .sealed_track_identity_digest(
                    &base.repo_id,
                    &base.revision_id,
                    quanta_index_contract::SearchPlaneTrackKind::Lexical,
                    base.generation,
                )
                .is_some()
        {
            return Err("the test did not retire the base".into());
        }
        let ids: Vec<_> = ledger
            .structural_state(&delta.repo_id, &delta.revision_id, delta.generation)
            .ok_or("target chunk universe absent")?
            .chunks()
            .keys()
            .map(quanta_index_contract::ChunkId::as_str)
            .collect();
        if ids != ["a-1", "a-2", "b-1", "c-1", "c-2"] {
            return Err(format!("target lost inherited chunks: {ids:?}").into());
        }
        drop(ledger);
    }
    // Neither an exact sealed pair without chunk completion, nor a completion
    // marker without the original source reservation may waive the base check.
    let set_marker = |digest: Option<String>| -> TestRes {
        fakes
            .ledger
            .write()
            .map_err(|e| e.to_string())?
            .aux_restore_mut::<crate::readiness::StructuralAuthorityState>(
                &delta.repo_id,
                &delta.revision_id,
                delta.generation,
            )
            .restore_meta(crate::readiness::StructuralStateMeta {
                seal_requested: false,
                source_batch_digest: digest,
            });
        Ok(())
    };
    set_marker(None)?;
    if !matches!(
        port.preflight_batch(&delta),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusDeltaBaseNotSealed,
            ..
        })
    ) {
        return Err("sealed tracks waived an absent chunk completion marker".into());
    }
    let mut unreserved = delta.clone();
    unreserved.source_event.event_id = "unreserved-target".into();
    super::support::restamp_search_corpus_fixture(&mut unreserved)?;
    set_marker(Some(unreserved.batch_digest.clone()))?;
    if !matches!(
        port.preflight_batch(&unreserved),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusDeltaBaseNotSealed,
            ..
        })
    ) {
        return Err("completion marker waived an absent original source reservation".into());
    }
    set_marker(Some(delta.batch_digest.clone()))?;
    let recovered = receipt_of(dispatcher.dispatch(request.clone(), &budget))?;
    let replay = receipt_of(dispatcher.dispatch(request, &budget))?;
    if !recovered.applied
        || replay.applied
        || recovered.durable_sequence != replay.durable_sequence
        || port.failures.load(Ordering::SeqCst) != 2
    {
        return Err("original completed delta did not recover and replay once".into());
    }
    if !fakes
        .lexical_builder
        .batches
        .lock()
        .map_err(|e| e.to_string())?
        .is_empty()
        || !fakes.semantic_builder.take()?.is_empty()
    {
        return Err("completed delta recovery rebuilt sealed tracks".into());
    }
    Ok(())
}

/// Failure after materialization is not proof that a source event never ran.
#[test]
fn source_event_apply_errors_retry_the_original_journal_without_retargeting() -> TestRes {
    use quanta_index_core::{
        IdempotencyCatalogPort as _, OperationInspectV1, SourceEventPhaseV1,
        SourcePublicationCatalogPort as _,
    };

    struct FailAfterPublishOnce {
        inner: DirectSearchCorpusMaterializer,
        error: Mutex<Option<CoreError>>,
        applies: AtomicUsize,
        preflights: AtomicUsize,
    }
    impl SearchCorpusIngestPort for FailAfterPublishOnce {
        fn preflight_batch(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
            if self.preflights.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::DeltaBaseConflict,
                    message: "transient source preflight refusal after an uncertain apply".into(),
                });
            }
            self.inner.preflight_batch(batch)
        }

        fn publish_batch(
            &self,
            batch: &SearchCorpusIngestBatch,
            budget: &RequestBudgetV1,
        ) -> Result<quanta_index_contract::SearchCorpusPublishOutcome, CoreError> {
            let _prior = self.applies.fetch_add(1, Ordering::SeqCst);
            let outcome = self.inner.publish_batch(batch, budget)?;
            let injected_error = self
                .error
                .lock()
                .map_err(|error| CoreError::Storage(error.to_string()))?
                .take();
            if let Some(error) = injected_error {
                return Err(error);
            }
            Ok(outcome)
        }
    }

    let message = "injected source apply failure after finalization";
    for failure in [
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationConflict,
            message: message.into(),
        },
        CoreError::InvalidContract(message.into()),
        CoreError::Storage(message.into()),
    ] {
        let catalog = memory_catalog();
        let (inner, _fakes) = search_corpus_materializer(catalog.clone(), true, false);
        let port = Arc::new(FailAfterPublishOnce {
            inner,
            error: Mutex::new(Some(failure)),
            applies: AtomicUsize::new(0),
            preflights: AtomicUsize::new(0),
        });
        let dispatcher = search_corpus_dispatcher_with_port(port.clone(), catalog.clone());
        let batch = fixture_search_corpus_batch()?;
        let key = IdempotencyKeyV1 {
            kind: IngestOperationKindV1::SearchCorpus,
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            batch_digest: batch.batch_digest.clone(),
        };
        let budget = RequestBudgetV1::unbounded();
        let failed = dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
            &budget,
        );
        let pending = catalog
            .source_publication
            .inspect_source_event(&batch.repo_id, &batch.source_event)?
            .ok_or("materialized event lost its reservation")?;
        if !matches!(&failed, SearchPlaneIngestIpcResponse::Error(error)
            if error.message.contains(message))
            || !matches!(catalog.inspect(&key)?, OperationInspectV1::Uncertain { .. })
            || pending.phase != SourceEventPhaseV1::Pending
            || pending.binding.journal_key != key
        {
            return Err(format!("apply error froze or released a source event: {failed:?}").into());
        }

        let mut retargeted = batch.clone();
        retargeted.generation = ManifestGeneration::new(99);
        stamp_batch_digest_v1(&mut retargeted)?;
        let refused = dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(retargeted),
            &budget,
        );
        if typed_code_of(&refused)
            != Some(quanta_index_contract::SearchPlaneErrorCodeV2::CatalogBusy)
            || port.applies.load(Ordering::SeqCst) != 1
        {
            return Err("an uncertain source event must retain its original target".into());
        }
        let mut changed = batch.clone();
        changed
            .replace_scopes
            .first_mut()
            .ok_or("fixture replacement")?
            .coverage
            .producer_policy_sha256 = [9; 32];
        super::support::restamp_search_corpus_fixture(&mut changed)?;
        let refused = dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(changed),
            &budget,
        );
        if typed_code_of(&refused)
            != Some(quanta_index_contract::SearchPlaneErrorCodeV2::BatchDigestConflict)
            || port.applies.load(Ordering::SeqCst) != 1
        {
            return Err("an uncertain source event must reject payload replacement".into());
        }

        let preflight_refusal = dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
            &budget,
        );
        if typed_code_of(&preflight_refusal)
            != Some(quanta_index_contract::SearchPlaneErrorCodeV2::DeltaBaseConflict)
            || matches!(catalog.inspect(&key)?, OperationInspectV1::Refused { .. })
            || catalog
                .source_publication
                .inspect_source_event(&batch.repo_id, &batch.source_event)?
                != Some(pending.clone())
            || port.applies.load(Ordering::SeqCst) != 1
        {
            return Err("retry preflight froze or released the original source event".into());
        }

        let receipt = receipt_of(dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch.clone()),
            &budget,
        ))?;
        let staged = catalog
            .source_publication
            .inspect_source_event(&batch.repo_id, &batch.source_event)?
            .ok_or("retried source event disappeared")?;
        if !receipt.applied
            || receipt.durable_sequence == 0
            || staged.phase != SourceEventPhaseV1::Staged
            || staged.binding != pending.binding
            || port.applies.load(Ordering::SeqCst) != 2
        {
            return Err(
                "the original source event did not recover under its retained binding".into(),
            );
        }
        let replay = receipt_of(dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
            &budget,
        ))?;
        if replay.applied
            || replay.durable_sequence != receipt.durable_sequence
            || port.applies.load(Ordering::SeqCst) != 2
        {
            return Err("recovered publication did not retain terminal replay semantics".into());
        }
    }
    Ok(())
}

/// Journal evidence survives in this test, so `UNKNOWN_GENERATION` must come
/// from the retained generation authority rather than a missing journal row.
#[test]
fn source_event_replay_refuses_reclaimed_original_even_when_journal_survives() -> TestRes {
    struct Retention(std::sync::atomic::AtomicBool);
    impl crate::ingest_dispatcher::SearchCorpusAuthorityInspectPort for Retention {
        fn inspect_sealed_search_corpus(
            &self,
            _: &RepoId,
            _: &RevisionId,
            _: ManifestGeneration,
            _: &str,
        ) -> Result<crate::SealedSearchCorpusAuthorityStateV1, CoreError> {
            Ok(if self.0.load(Ordering::SeqCst) {
                crate::SealedSearchCorpusAuthorityStateV1::Exact
            } else {
                crate::SealedSearchCorpusAuthorityStateV1::Absent
            })
        }
    }
    let catalog = memory_catalog();
    let (materializer, _fakes) = search_corpus_materializer(Arc::clone(&catalog), true, false);
    let counting = Arc::new(CountingSearchCorpus::new(materializer));
    let retention = Arc::new(Retention(std::sync::atomic::AtomicBool::new(true)));
    let dispatcher = search_corpus_dispatcher_with_authority(
        counting.clone(),
        Arc::clone(&catalog),
        retention.clone(),
    );
    let budget = RequestBudgetV1::unbounded();
    let original = fixture_search_corpus_batch()?;
    let _first = receipt_of(dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(original.clone()),
        &budget,
    ))?;
    retention.0.store(false, Ordering::SeqCst);
    let mut replay = original;
    replay.generation = ManifestGeneration::new(99);
    replay.manifest_digest = "repackaged-manifest".into();
    stamp_batch_digest_v1(&mut replay)?;
    let refused = dispatcher.dispatch(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(replay),
        &budget,
    );
    if typed_code_of(&refused)
        != Some(quanta_index_contract::SearchPlaneErrorCodeV2::UnknownGeneration)
        || catalog.records() != 1
        || counting.applies() != 1
        || counting.preflights() != 1
    {
        return Err(
            format!("reclaimed source event was replayed or re-materialized: {refused:?}").into(),
        );
    }
    Ok(())
}
