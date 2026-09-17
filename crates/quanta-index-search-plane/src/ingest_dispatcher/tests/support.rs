//! Fixtures shared by the ingest dispatcher test modules: in-memory
//! catalogs, port doubles, and batch builders.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use quanta_index_contract::{
    BatchIngestMode, BatchPublishReceipt, CapabilityStatusV1, ChunkId, ChunkRecord,
    DirtyIngestBatch, DirtyMutation, EmbeddingDistanceMetric, EmbeddingId, EmbeddingModelContract,
    EmbeddingNormalization, EmbeddingRecord, GenerationSnapshot, HistoryIngestBatch,
    ManifestGeneration, OwnerDocKind, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchPlaneTrackKind, SearchScopeKey,
    SearchScopeSurface, SemanticCorpusKindV1, SemanticIngestBatch, SemanticReplaceScope,
    SourceRoleV1,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, IdempotencyBeginV1, IdempotencyCatalogPort,
    IdempotencyKeyV1, IncompleteGenerationDiscardOutcomeV1, IncompleteGenerationDiscardPort,
    IngestResourcePolicy, RequestBudgetV1, SealedGenerationReclaimOutcomeV1,
    SealedGenerationReclaimPort, SemanticBatchBuildPort, SemanticIngestPort, TextEmbeddingProvider,
};

use crate::auxiliary_authority::testing::MemoryAuxiliaryCatalog;
use crate::ingest_dispatcher::auxiliary::{
    AuxiliaryMaterializerParts, AuxiliaryMutationCoordinator,
};
use crate::ingest_dispatcher::ports::SearchCorpusAuthorityWritePort;
use crate::ingest_dispatcher::search_corpus::{
    DirectSearchCorpusMaterializer, SearchCorpusMaterializerParts,
};
use crate::ingest_dispatcher::semantic::DirectSemanticMaterializer;
use crate::readiness::SearchCorpusHistoryRetentionReceiptV1;
use crate::{
    Ledger, SEARCH_OWNED_SEMANTIC_DIMENSION, SealedSearchCorpusAuthorityStateV1, SnapshotRegistries,
};

pub(super) type TestRes = Result<(), Box<dyn std::error::Error>>;

/// One in-memory record: the body hash and, once finalized, the receipt
/// and its sequence.
pub(super) type MemoryRecord = ([u8; 32], Option<(BatchPublishReceipt, u64)>);

/// An in-memory idempotency catalog with the port's exact semantics, for
/// tests that need the protocol without the storage engine.
#[derive(Default)]
pub(crate) struct MemoryIdempotencyCatalog {
    records: Mutex<BTreeMap<IdempotencyKeyV1, MemoryRecord>>,
    next_sequence: AtomicUsize,
}

impl MemoryIdempotencyCatalog {
    pub(crate) fn records(&self) -> usize {
        self.records.lock().map_or(0, |records| records.len())
    }
}

impl IdempotencyCatalogPort for MemoryIdempotencyCatalog {
    fn begin(
        &self,
        key: &IdempotencyKeyV1,
        body_sha256: &[u8; 32],
    ) -> Result<IdempotencyBeginV1, CoreError> {
        let mut records = self
            .records
            .lock()
            .map_err(|err| CoreError::Storage(format!("memory catalog poisoned: {err}")))?;
        let outcome = match records.get(key) {
            None => {
                let _new = records.insert(key.clone(), (*body_sha256, None));
                Ok(IdempotencyBeginV1::Fresh)
            }
            Some((stored, _)) if stored != body_sha256 => Err(CoreError::Typed {
                code: quanta_index_core::BATCH_DIGEST_CONFLICT_CODE.to_string(),
                message: format!(
                    "{} batch_digest={} body differs",
                    key.kind, key.batch_digest
                ),
            }),
            Some((_, Some((receipt, durable_sequence)))) => Ok(IdempotencyBeginV1::Replay {
                receipt: receipt.clone(),
                durable_sequence: *durable_sequence,
            }),
            Some((_, None)) => Ok(IdempotencyBeginV1::Resume),
        };
        drop(records);
        outcome
    }

    fn finalize(
        &self,
        key: &IdempotencyKeyV1,
        body_sha256: &[u8; 32],
        receipt: &BatchPublishReceipt,
    ) -> Result<u64, CoreError> {
        let mut records = self
            .records
            .lock()
            .map_err(|err| CoreError::Storage(format!("memory catalog poisoned: {err}")))?;
        let Some(record) = records.get_mut(key) else {
            return Err(CoreError::InvalidContract(
                "finalize before begin".to_string(),
            ));
        };
        if record.0 != *body_sha256 {
            return Err(CoreError::InvalidContract(
                "finalize under another body".to_string(),
            ));
        }
        if record.1.is_some() {
            return Err(CoreError::InvalidContract("finalize twice".to_string()));
        }
        let sequence = u64::try_from(self.next_sequence.fetch_add(1, Ordering::SeqCst))
            .map_err(|err| CoreError::Storage(err.to_string()))?
            .saturating_add(1);
        record.1 = Some((receipt.clone(), sequence));
        drop(records);
        Ok(sequence)
    }

    fn forget_generation(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<u64, CoreError> {
        let mut records = self
            .records
            .lock()
            .map_err(|err| CoreError::Storage(format!("memory catalog poisoned: {err}")))?;
        let before = records.len();
        records.retain(|key, _| {
            !(key.repo_id == *repo_id
                && key.revision_id == *revision_id
                && key.generation == generation)
        });
        u64::try_from(before.saturating_sub(records.len()))
            .map_err(|err| CoreError::Storage(err.to_string()))
    }
}

pub(super) fn memory_catalog() -> Arc<MemoryIdempotencyCatalog> {
    Arc::new(MemoryIdempotencyCatalog::default())
}

pub(super) fn memory_aux_catalog() -> Arc<MemoryAuxiliaryCatalog> {
    Arc::new(MemoryAuxiliaryCatalog::default())
}

pub(super) fn aux_parts(
    ledger: Arc<RwLock<Ledger>>,
) -> (AuxiliaryMaterializerParts, Arc<MemoryAuxiliaryCatalog>) {
    let catalog = memory_aux_catalog();
    (
        AuxiliaryMaterializerParts {
            catalog: catalog.clone(),
            coordinator: AuxiliaryMutationCoordinator::shared(),
            ledger,
        },
        catalog,
    )
}

macro_rules! search_corpus_materializer {
    (
        $builder:expr,
        $ledger:expr,
        $semantic_ingest:expr,
        $semantic_embedder:expr,
        $authority:expr,
        $lexical_generation_validator:expr,
        $semantic_generation_validator:expr,
        $lexical_incomplete_discard:expr,
        $semantic_incomplete_discard:expr $(,)?
    ) => {
        DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
            SearchCorpusMaterializerParts {
                builder: $builder,
                ledger: $ledger,
                semantic_ingest: $semantic_ingest,
                semantic_embedder: $semantic_embedder,
                authority: $authority,
                lexical_generation_validator: $lexical_generation_validator,
                semantic_generation_validator: $semantic_generation_validator,
                lexical_incomplete_discard: $lexical_incomplete_discard,
                semantic_incomplete_discard: $semantic_incomplete_discard,
                lexical_reclaim: no_storage_sealed_reclaim(),
                semantic_reclaim: no_storage_sealed_reclaim(),
                snapshots: SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT),
                idempotency: memory_catalog(),
                resource_policy: IngestResourcePolicy::DEFAULT,
                auxiliary_catalog: memory_aux_catalog(),
                auxiliary_coordinator: AuxiliaryMutationCoordinator::shared(),
            },
        )
    };
}

#[derive(Default)]
pub(super) struct RecordingSearchCorpusAuthority {
    pub(super) identities: Mutex<Vec<(RepoId, RevisionId, ManifestGeneration, String)>>,
    pub(super) exact: bool,
}

impl SearchCorpusAuthorityWritePort for RecordingSearchCorpusAuthority {
    fn inspect_sealed_search_corpus(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        _generation: ManifestGeneration,
        _manifest_digest: &str,
    ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError> {
        Ok(if self.exact {
            SealedSearchCorpusAuthorityStateV1::Exact
        } else {
            SealedSearchCorpusAuthorityStateV1::Absent
        })
    }

    fn record_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError> {
        let retained_generations = {
            let mut identities = self.identities.lock().map_err(|err| {
                CoreError::Storage(format!("recording search-corpus authority poisoned: {err}"))
            })?;
            identities.push((
                repo_id.clone(),
                revision_id.clone(),
                generation,
                manifest_digest.to_string(),
            ));
            identities
                .iter()
                .filter(|(observed_repo, observed_revision, _, _)| {
                    observed_repo == repo_id && observed_revision == revision_id
                })
                .map(|(_, _, observed_generation, _)| *observed_generation)
                .collect::<Vec<_>>()
        };
        Ok(
            SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
                repo_id,
                revision_id,
                retained_generations,
            ),
        )
    }
}

pub(super) fn recording_search_corpus_authority()
-> Arc<dyn SearchCorpusAuthorityWritePort + Send + Sync> {
    Arc::new(RecordingSearchCorpusAuthority::default())
}

pub(super) struct FailingRetentionAuthority;

impl SearchCorpusAuthorityWritePort for FailingRetentionAuthority {
    fn inspect_sealed_search_corpus(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        _generation: ManifestGeneration,
        _manifest_digest: &str,
    ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError> {
        Ok(SealedSearchCorpusAuthorityStateV1::Exact)
    }

    fn record_sealed_search_corpus(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        _generation: ManifestGeneration,
        _manifest_digest: &str,
    ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError> {
        Err(CoreError::Storage(
            "injected post-delete retention durability failure".to_string(),
        ))
    }
}

#[derive(Default)]
pub(super) struct BuildThenValidGeneration {
    validations: AtomicUsize,
}

impl GenerationIdentityValidatePort for BuildThenValidGeneration {
    fn validate_generation_identity(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<(), CoreError> {
        if self.validations.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(CoreError::NotFound(format!(
                "test generation not built yet: {:?}",
                candidate.track
            )));
        }
        Ok(())
    }
}

pub(super) fn build_then_valid_generation() -> Arc<dyn GenerationIdentityValidatePort + Send + Sync>
{
    Arc::new(BuildThenValidGeneration::default())
}

#[derive(Default)]
pub(super) struct IncompleteThenValidGeneration {
    validations: AtomicUsize,
}

impl GenerationIdentityValidatePort for IncompleteThenValidGeneration {
    fn validate_generation_identity(
        &self,
        _candidate: &GenerationSnapshot,
    ) -> Result<(), CoreError> {
        if self.validations.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(CoreError::Typed {
                code: "GENERATION_IDENTITY_INCOMPLETE".to_string(),
                message: "injected incomplete generation".to_string(),
            });
        }
        Ok(())
    }
}

pub(super) fn incomplete_then_valid_generation()
-> Arc<dyn GenerationIdentityValidatePort + Send + Sync> {
    Arc::new(IncompleteThenValidGeneration::default())
}

pub(super) struct AlwaysValidGeneration;

impl GenerationIdentityValidatePort for AlwaysValidGeneration {
    fn validate_generation_identity(
        &self,
        _candidate: &GenerationSnapshot,
    ) -> Result<(), CoreError> {
        Ok(())
    }
}

pub(super) fn always_valid_generation() -> Arc<dyn GenerationIdentityValidatePort + Send + Sync> {
    Arc::new(AlwaysValidGeneration)
}

pub(super) struct TestIncompleteGenerationDiscard;

impl IncompleteGenerationDiscardPort for TestIncompleteGenerationDiscard {
    fn discard_incomplete_generation(
        &self,
        _candidate: &GenerationSnapshot,
    ) -> Result<IncompleteGenerationDiscardOutcomeV1, CoreError> {
        Ok(IncompleteGenerationDiscardOutcomeV1::Discarded)
    }
}

pub(super) fn test_incomplete_generation_discard()
-> Arc<dyn IncompleteGenerationDiscardPort + Send + Sync> {
    Arc::new(TestIncompleteGenerationDiscard)
}

/// A reclaim port over no storage.
///
/// Nothing is ever on disk, so the sweep finds nothing. The materializer
/// tests here exercise the authority and ledger protocol; physical
/// reclaim is proven against the real adapters in the daemon-level
/// `e2e_physical_gc` test.
pub(super) struct NoStorageSealedReclaim;

impl SealedGenerationReclaimPort for NoStorageSealedReclaim {
    fn reclaim_sealed_generation(
        &self,
        _retired: &GenerationSnapshot,
    ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError> {
        Ok(SealedGenerationReclaimOutcomeV1::Absent)
    }

    fn sealed_generations_for_pair(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
    ) -> Result<Vec<GenerationSnapshot>, CoreError> {
        Ok(Vec::new())
    }
}

pub(super) fn no_storage_sealed_reclaim() -> Arc<dyn SealedGenerationReclaimPort + Send + Sync> {
    Arc::new(NoStorageSealedReclaim)
}

/// A reclaim port over a scripted set of on-disk sealed generations; it
/// records every reclaim so the protocol's decisions are observable.
pub(super) struct ScriptedSealedReclaim {
    pub(super) track: SearchPlaneTrackKind,
    pub(super) on_disk: Mutex<Vec<ManifestGeneration>>,
    pub(super) reclaimed: Mutex<Vec<ManifestGeneration>>,
}

impl ScriptedSealedReclaim {
    pub(super) fn new(track: SearchPlaneTrackKind, on_disk: &[u64]) -> Arc<Self> {
        Arc::new(Self {
            track,
            on_disk: Mutex::new(
                on_disk
                    .iter()
                    .copied()
                    .map(ManifestGeneration::new)
                    .collect(),
            ),
            reclaimed: Mutex::new(Vec::new()),
        })
    }

    pub(super) fn reclaimed(&self) -> Vec<u64> {
        match self.reclaimed.lock() {
            Ok(guard) => guard.iter().map(|generation| generation.get()).collect(),
            Err(poisoned) => poisoned
                .into_inner()
                .iter()
                .map(|generation| generation.get())
                .collect(),
        }
    }

    pub(super) fn remaining(&self) -> Vec<u64> {
        match self.on_disk.lock() {
            Ok(guard) => guard.iter().map(|generation| generation.get()).collect(),
            Err(poisoned) => poisoned
                .into_inner()
                .iter()
                .map(|generation| generation.get())
                .collect(),
        }
    }
}

impl SealedGenerationReclaimPort for ScriptedSealedReclaim {
    fn reclaim_sealed_generation(
        &self,
        retired: &GenerationSnapshot,
    ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError> {
        let mut on_disk = self
            .on_disk
            .lock()
            .map_err(|err| CoreError::Storage(format!("scripted reclaim poisoned: {err}")))?;
        let Some(index) = on_disk
            .iter()
            .position(|generation| *generation == retired.manifest_generation)
        else {
            return Ok(SealedGenerationReclaimOutcomeV1::Absent);
        };
        let _removed = on_disk.remove(index);
        drop(on_disk);
        self.reclaimed
            .lock()
            .map_err(|err| CoreError::Storage(format!("scripted reclaim poisoned: {err}")))?
            .push(retired.manifest_generation);
        Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { bytes: 1 })
    }

    fn sealed_generations_for_pair(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<GenerationSnapshot>, CoreError> {
        Ok(self
            .on_disk
            .lock()
            .map_err(|err| CoreError::Storage(format!("scripted reclaim poisoned: {err}")))?
            .iter()
            .map(|generation| GenerationSnapshot {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                track: self.track,
                manifest_generation: *generation,
                manifest_digest: format!("digest:{}", generation.get()),
            })
            .collect())
    }
}

/// The smallest `LexicalSearcher` that can occupy a registry slot: it
/// exists only so a test can hold a pin on a generation.
pub(super) struct PinnedLexicalHandle;

impl quanta_index_core::LexicalSearcher for PinnedLexicalHandle {
    fn resident_bytes_estimate(&self) -> u64 {
        1
    }

    fn search_constrained(
        &self,
        _query: &quanta_index_contract::LqQuery,
        _constraints: &quanta_index_contract::QueryConstraintSetV1,
        _top_k: u32,
        _budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::LexicalSearchPageV1, CoreError> {
        Err(CoreError::NotImplemented("pin-only handle".to_string()))
    }

    fn project_file_owners(
        &self,
        _candidates: &[quanta_index_contract::LexicalCandidate],
    ) -> Result<Vec<quanta_index_contract::FileOwnerProjectionRow>, CoreError> {
        Err(CoreError::NotImplemented("pin-only handle".to_string()))
    }

    fn search_symbols(
        &self,
        _query: &quanta_index_contract::LqQuery,
        _top_k: u32,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<quanta_index_contract::SymbolCandidate>, CoreError> {
        Err(CoreError::NotImplemented("pin-only handle".to_string()))
    }

    fn search_all(
        &self,
        _query: &quanta_index_contract::LqQuery,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<quanta_index_contract::LexicalCandidate>, CoreError> {
        Err(CoreError::NotImplemented("pin-only handle".to_string()))
    }

    fn candidate_presence(
        &self,
        _candidate_id: &str,
    ) -> Result<quanta_index_contract::CandidatePresenceV1, CoreError> {
        Err(CoreError::NotImplemented("pin-only handle".to_string()))
    }

    fn explain_candidate(
        &self,
        _query: &quanta_index_contract::LqQuery,
        _constraints: &quanta_index_contract::QueryConstraintSetV1,
        _candidate_id: &str,
        _budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::LexicalCandidateExplanationV1, CoreError> {
        Err(CoreError::NotImplemented("pin-only handle".to_string()))
    }
}

/// One materializer over recording fakes, plus the fakes, so a test can
/// prove that a refused batch touched nothing.
pub(super) struct ZeroMutationProbe {
    pub(super) materializer: DirectSearchCorpusMaterializer,
    pub(super) lexical_builder: Arc<FakeSearchCorpusBuilder>,
    pub(super) semantic_builder: Arc<FakeSemanticBuilder>,
    pub(super) authority: Arc<RecordingSearchCorpusAuthority>,
    pub(super) ledger: Arc<RwLock<Ledger>>,
}

impl ZeroMutationProbe {
    pub(super) fn new(validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>) -> Self {
        Self::with_resource_policy(validator, IngestResourcePolicy::DEFAULT)
    }

    pub(super) fn with_resource_policy(
        validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
        resource_policy: IngestResourcePolicy,
    ) -> Self {
        let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let authority = Arc::new(RecordingSearchCorpusAuthority::default());
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
            Arc::new(DirectSemanticMaterializer::new(
                semantic_builder.clone(),
                Arc::new(RwLock::new(Ledger::new())),
            ));
        let materializer = DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
            SearchCorpusMaterializerParts {
                builder: lexical_builder.clone(),
                ledger: Arc::clone(&ledger),
                semantic_ingest: semantic_materializer,
                semantic_embedder: Arc::new(crate::HashingQueryTextEmbedder::new(
                    SEARCH_OWNED_SEMANTIC_DIMENSION,
                )),
                authority: authority.clone(),
                lexical_generation_validator: Arc::clone(&validator),
                semantic_generation_validator: validator,
                lexical_incomplete_discard: test_incomplete_generation_discard(),
                semantic_incomplete_discard: test_incomplete_generation_discard(),
                lexical_reclaim: no_storage_sealed_reclaim(),
                semantic_reclaim: no_storage_sealed_reclaim(),
                snapshots: SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT),
                idempotency: memory_catalog(),
                resource_policy,
                auxiliary_catalog: memory_aux_catalog(),
                auxiliary_coordinator: AuxiliaryMutationCoordinator::shared(),
            },
        );
        Self {
            materializer,
            lexical_builder,
            semantic_builder,
            authority,
            ledger,
        }
    }

    pub(super) fn assert_nothing_touched(&self, what: &str) -> TestRes {
        let lexical = self
            .lexical_builder
            .batches
            .lock()
            .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
            .len();
        let semantic = self.semantic_builder.take()?.len();
        let recorded = self
            .authority
            .identities
            .lock()
            .map_err(|err| format!("recording authority poisoned: {err}"))?
            .len();
        if lexical != 0 || semantic != 0 || recorded != 0 {
            return Err(format!(
                "{what}: refused batch still mutated: lexical_builds={lexical} semantic_builds={semantic} authority_records={recorded}"
            )
            .into());
        }
        Ok(())
    }
}

/// A validator that reports the base as sealed under a different digest.
pub(super) struct MismatchedGeneration;

impl GenerationIdentityValidatePort for MismatchedGeneration {
    fn validate_generation_identity(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<(), CoreError> {
        Err(CoreError::Typed {
            code: "GENERATION_IDENTITY_DIGEST_MISMATCH".to_string(),
            message: format!(
                "injected digest mismatch for {:?} generation {}",
                candidate.track,
                candidate.manifest_generation.get()
            ),
        })
    }
}

#[derive(Default)]
pub(super) struct RecordingIncompleteGenerationDiscard {
    pub(super) calls: AtomicUsize,
}

impl IncompleteGenerationDiscardPort for RecordingIncompleteGenerationDiscard {
    fn discard_incomplete_generation(
        &self,
        _candidate: &GenerationSnapshot,
    ) -> Result<IncompleteGenerationDiscardOutcomeV1, CoreError> {
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(IncompleteGenerationDiscardOutcomeV1::Discarded)
    }
}

pub(super) struct MismatchedSemanticIngest;

impl SemanticIngestPort for MismatchedSemanticIngest {
    fn publish_batch(&self, batch: &SemanticIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let mut receipt = BatchPublishReceipt::empty_for(
            batch.generation,
            Some("mismatched-semantic-digest".to_string()),
            batch.batch_digest.clone(),
        );
        if batch.seal {
            receipt.mark_sealed();
        }
        Ok(receipt)
    }
}

#[derive(Default)]
pub(super) struct FakeSemanticBuilder {
    pub(super) batches: Mutex<Vec<SemanticIngestBatch>>,
}

impl FakeSemanticBuilder {
    pub(super) fn take(&self) -> Result<Vec<SemanticIngestBatch>, Box<dyn std::error::Error>> {
        let mut guard = self
            .batches
            .lock()
            .map_err(|err| format!("fake semantic builder poisoned: {err}"))?;
        Ok(std::mem::take(&mut *guard))
    }
}

impl SemanticBatchBuildPort for FakeSemanticBuilder {
    fn build_batch(&self, batch: &SemanticIngestBatch) -> Result<(), CoreError> {
        self.batches
            .lock()
            .map_err(|err| CoreError::Storage(format!("fake semantic builder poisoned: {err}")))?
            .push(batch.clone());
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct FakeSearchCorpusBuilder {
    pub(super) batches: Mutex<Vec<SearchCorpusIngestBatch>>,
}

impl quanta_index_core::SearchCorpusBatchBuildPort for FakeSearchCorpusBuilder {
    fn build_batch(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        self.batches
            .lock()
            .map_err(|err| {
                CoreError::Storage(format!("fake search-corpus builder poisoned: {err}"))
            })?
            .push(batch.clone());
        Ok(())
    }
}

pub(super) fn fixture_scope() -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new("src/main.rs"),
    }
}

pub(super) fn fixture_model_contract() -> EmbeddingModelContract {
    EmbeddingModelContract {
        model_id: "test-model".to_string().into_boxed_str(),
        model_version: None,
        dimension: 3,
        normalization: EmbeddingNormalization::None,
        distance_metric: EmbeddingDistanceMetric::Cosine,
        policy_digest: "policy:abc".to_string().into_boxed_str(),
        view_policy_digest: None,
    }
}

pub(super) fn fixture_embedding_record() -> Result<EmbeddingRecord, Box<dyn std::error::Error>> {
    Ok(EmbeddingRecord {
        embedding_id: EmbeddingId::new("emb-1"),
        record_id: "emb-1".to_string().into_boxed_str(),
        owner_kind: OwnerDocKind::Chunk,
        owner_id: "main".to_string().into_boxed_str(),
        corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
        parent_owner_id: None,
        source_doc_id: "chunk-1".to_string().into_boxed_str(),
        repo_relative_path: RepoRelativePath::new("src/main.rs"),
        language: quanta_index_contract::lex::LanguageCode::new("rust").map_err(str::to_string)?,
        package: None,
        symbol_kind: None,
        visibility: None,
        source_role: SourceRoleV1::RawFallbackText,
        generated: false,
        capability_status: CapabilityStatusV1::Degraded,
        authority_digest: "search-owned:legacy-chunk-text"
            .to_string()
            .into_boxed_str(),
        render_policy_digest: "search-owned:legacy-chunk-text"
            .to_string()
            .into_boxed_str(),
        card_schema_version: 0,
        start_byte: 0,
        end_byte: 12,
        start_line: 1,
        end_line: 1,
        snippet: "fn main() {}".to_string().into_boxed_str(),
        embedding_input_digest: "input:abc".to_string().into_boxed_str(),
        vector_digest: "vec:def".to_string().into_boxed_str(),
        view_kind: "raw_chunk".to_string().into_boxed_str(),
        vector: vec![0.1, 0.2, 0.3],
    })
}

pub(super) fn fixture_semantic_batch() -> Result<SemanticIngestBatch, Box<dyn std::error::Error>> {
    Ok(SemanticIngestBatch {
        repo_id: RepoId::new("r"),
        revision_id: RevisionId::new("rev"),
        generation: ManifestGeneration::new(1),
        base_generation: None,
        manifest_digest: "manifest:sem".to_string(),
        batch_digest: "batch:sem".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: fixture_model_contract(),
        required_corpora: vec![SemanticCorpusKindV1::RawCodeFallback],
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: fixture_scope(),
            scope_digest: "scope:sem".to_string(),
            embeddings: vec![fixture_embedding_record()?],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        seal: true,
    })
}

pub(super) fn fixture_chunk_record() -> Result<ChunkRecord, Box<dyn std::error::Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new("chunk-1"),
        repo_relative_path: RepoRelativePath::new("src/main.rs"),
        language: quanta_index_contract::lex::LanguageCode::new("rust").map_err(str::to_string)?,
        start_byte: 0,
        end_byte: 24,
        start_line: 1,
        end_line: 1,
        text: "typed semantic parser".to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

pub(super) fn fixture_search_corpus_batch()
-> Result<SearchCorpusIngestBatch, Box<dyn std::error::Error>> {
    Ok(SearchCorpusIngestBatch {
        repo_id: RepoId::new("r"),
        revision_id: RevisionId::new("rev"),
        generation: ManifestGeneration::new(7),
        base_generation: None,
        manifest_digest: "manifest:lex".to_string(),
        batch_digest: "batch:lex".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SearchCorpusReplaceScope {
            scope: fixture_scope(),
            scope_digest: "scope:lex".to_string(),
            chunks: vec![fixture_chunk_record()?],
            symbols: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    })
}

pub(super) fn fixture_dirty_batch() -> DirtyIngestBatch {
    DirtyIngestBatch {
        repo_id: RepoId::new("r"),
        revision_id: RevisionId::new("rev"),
        generation: ManifestGeneration::new(9),
        overlay_epoch_ms: 123,
        batch_digest: "batch:dirty".to_string(),
        entries: vec![DirtyMutation::Upsert(
            quanta_index_contract::lex::DirtyRecord {
                wire_version: 1,
                doc_id: ChunkId::new("chunk-1"),
                applied_at_ms: 123,
                payload_hash: [7; 32],
            },
        )],
    }
}

pub(super) fn fixture_commit(
    sha_byte: u8,
    parents: &[u8],
) -> quanta_index_contract::lex::CommitRecord {
    quanta_index_contract::lex::CommitRecord {
        wire_version: 1,
        sha: quanta_index_contract::lex::CommitSha::from_bytes([sha_byte; 20]),
        parents: parents
            .iter()
            .map(|parent| quanta_index_contract::lex::CommitSha::from_bytes([*parent; 20]))
            .collect(),
        author_time_ms: 1,
        committer_time_ms: 2,
        applied_at_ms: 3,
        author: "a".into(),
        author_name: None,
        author_email: None,
        committer: "c".into(),
        committer_name: None,
        committer_email: None,
        message: format!("commit {sha_byte}").into_boxed_str(),
        is_merge: false,
        tags: Vec::new(),
    }
}

pub(super) fn fixture_history_batch(
    generation: u64,
    commits: Vec<quanta_index_contract::lex::CommitRecord>,
) -> HistoryIngestBatch {
    HistoryIngestBatch {
        repo_id: RepoId::new("r"),
        revision_id: RevisionId::new("rev"),
        generation: ManifestGeneration::new(generation),
        manifest_digest: None,
        batch_digest: format!("batch:history:{generation}"),
        commits,
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    }
}

pub(super) struct FixedFakeEmbedder {
    pub(super) dimension: usize,
    pub(super) vectors_per_call: usize,
    pub(super) vector_len: usize,
}

impl TextEmbeddingProvider for FixedFakeEmbedder {
    fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        Ok((0..self.vectors_per_call)
            .map(|_| vec![0.0_f32; self.vector_len])
            .collect())
    }
    fn model_id(&self) -> &'static str {
        "fake-embedder"
    }
    fn model_revision(&self) -> &'static str {
        "r1"
    }
    fn dimension(&self) -> usize {
        self.dimension
    }
    fn normalization(&self) -> EmbeddingNormalization {
        EmbeddingNormalization::L2Unit
    }
}

pub(super) fn materializer_with_embedder(
    embedder: Arc<dyn TextEmbeddingProvider + Send + Sync>,
) -> DirectSearchCorpusMaterializer {
    let semantic_builder = Arc::new(FakeSemanticBuilder::default());
    let semantic_ledger = Arc::new(RwLock::new(Ledger::new()));
    let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
        DirectSemanticMaterializer::new(semantic_builder, Arc::clone(&semantic_ledger)),
    );
    let search_corpus_builder = Arc::new(FakeSearchCorpusBuilder::default());
    let lexical_ledger = Arc::new(RwLock::new(Ledger::new()));
    search_corpus_materializer!(
        search_corpus_builder,
        lexical_ledger,
        semantic_materializer,
        embedder,
        recording_search_corpus_authority(),
        build_then_valid_generation(),
        build_then_valid_generation(),
        test_incomplete_generation_discard(),
        test_incomplete_generation_discard(),
    )
}

pub(super) fn chunk_record_v(
    id: &str,
    path: &str,
    text: &str,
) -> Result<ChunkRecord, Box<dyn std::error::Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: RepoRelativePath::new(path),
        language: quanta_index_contract::lex::LanguageCode::new("rust").map_err(str::to_string)?,
        start_byte: 0,
        end_byte: 24,
        start_line: 1,
        end_line: 1,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

pub(super) fn scope_with_chunks(
    path: &str,
    digest: &str,
    chunks: Vec<ChunkRecord>,
) -> SearchCorpusReplaceScope {
    SearchCorpusReplaceScope {
        scope: SearchScopeKey {
            doc_surface: SearchScopeSurface::Chunk,
            repo_relative_path: RepoRelativePath::new(path),
        },
        scope_digest: digest.to_string(),
        chunks,
        symbols: Vec::new(),
    }
}

// A batch spanning 3 scopes with 2 / 1 / 2 chunks = 5 chunk texts in total.
pub(super) fn multi_scope_corpus_batch()
-> Result<SearchCorpusIngestBatch, Box<dyn std::error::Error>> {
    Ok(SearchCorpusIngestBatch {
        repo_id: RepoId::new("r"),
        revision_id: RevisionId::new("rev"),
        generation: ManifestGeneration::new(7),
        base_generation: None,
        manifest_digest: "manifest:lex".to_string(),
        batch_digest: "batch:lex".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![
            scope_with_chunks(
                "a.rs",
                "scope:a",
                vec![
                    chunk_record_v("a-1", "a.rs", "alpha one")?,
                    chunk_record_v("a-2", "a.rs", "alpha two")?,
                ],
            ),
            scope_with_chunks(
                "b.rs",
                "scope:b",
                vec![chunk_record_v("b-1", "b.rs", "beta one")?],
            ),
            scope_with_chunks(
                "c.rs",
                "scope:c",
                vec![
                    chunk_record_v("c-1", "c.rs", "gamma one")?,
                    chunk_record_v("c-2", "c.rs", "gamma two")?,
                ],
            ),
        ],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    })
}

// An embedder that counts embed_batch calls and returns one zero vector per
// input text, so a test can assert how many provider round trips a batch costs.
pub(super) struct CountingEmbedder {
    pub(super) dimension: usize,
    pub(super) calls: Mutex<usize>,
}

impl TextEmbeddingProvider for CountingEmbedder {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        {
            let mut calls = self.calls.lock().map_err(|err| {
                CoreError::InvalidContract(format!("counting embedder lock poisoned: {err}"))
            })?;
            *calls = calls.checked_add(1).ok_or_else(|| {
                CoreError::InvalidContract("counting embedder call counter overflow".to_string())
            })?;
        }
        Ok(texts
            .iter()
            .map(|_| vec![0.0_f32; self.dimension])
            .collect())
    }
    fn model_id(&self) -> &'static str {
        "counting-embedder"
    }
    fn model_revision(&self) -> &'static str {
        "r1"
    }
    fn dimension(&self) -> usize {
        self.dimension
    }
    fn normalization(&self) -> EmbeddingNormalization {
        EmbeddingNormalization::L2Unit
    }
}

pub(super) use search_corpus_materializer;
