//! Search-plane ingest orchestration (QI-RT-01).
//!
//! Producer sends a typed [`SearchPlaneIngestIpcRequest`] over UDS
//! `ingest.sock`. This dispatcher routes the typed batch to owner materializer
//! ports. The concrete runtime may choose to mirror accepted batches into
//! legacy channel persistence, but channel row-op fanout is no longer the
//! public ingest truth.
//!
//! Composition root in `quanta-index-searchd` is the only place that names
//! concrete adapter types (materializers, channel mirrors, repo-map ingest);
//! this module holds only [`Arc<dyn ...Port>`] (CLAUDE.md DIP rule).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    BatchPublishReceipt, DirtyIngestBatch, DirtyMutation, HistoryIngestBatch, HistoryRefMutation,
    LexicalIngestBatch, RepoMapMutationAck, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneIpcError, SearchPlaneTrackKind, SemanticIngestBatch,
    StructuralIngestBatch,
};
use quanta_index_core::{
    CoreError, LexicalBatchBuildPort, LexicalIngestPort, RepoMapBundleIngestPort,
    SemanticBatchBuildPort, SemanticIngestPort,
};
use serde::{Deserialize, Serialize};

use crate::{AuxiliaryAuthorityStore, Ledger};

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";

pub trait HistoryIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError>;
}

pub trait RuntimeMetadataIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError>;
}

pub trait StructuralIngestPort: Send + Sync {
    fn publish_batch(
        &self,
        batch: &StructuralIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SemanticAuthorityJournal {
    batches: Vec<SemanticIngestBatch>,
}

#[derive(Debug)]
pub struct SemanticAuthorityStore {
    journal_path: PathBuf,
    journal: RwLock<SemanticAuthorityJournal>,
}

impl SemanticAuthorityStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|err| {
            CoreError::Storage(format!(
                "semantic authority store: create root {}: {err}",
                root.display()
            ))
        })?;
        let journal_path = root.join("journal.cbor");
        let journal = read_cbor::<SemanticAuthorityJournal>(&journal_path, "semantic journal")?
            .unwrap_or_default();
        Ok(Self {
            journal_path,
            journal: RwLock::new(journal),
        })
    }

    pub fn append_batch(&self, batch: &SemanticIngestBatch) -> Result<(), CoreError> {
        let mut guard = self.journal.write().map_err(|err| {
            CoreError::Storage(format!("semantic authority store poisoned: {err}"))
        })?;
        guard.batches.push(batch.clone());
        write_cbor(&self.journal_path, &*guard, "semantic journal")
    }

    pub fn rollback_last_batch(&self, batch: &SemanticIngestBatch) -> Result<(), CoreError> {
        let mut guard = self.journal.write().map_err(|err| {
            CoreError::Storage(format!("semantic authority store poisoned: {err}"))
        })?;
        match guard.batches.last() {
            Some(last) if last == batch => {
                drop(guard.batches.pop());
                write_cbor(&self.journal_path, &*guard, "semantic journal")
            }
            Some(_) => Err(CoreError::Storage(
                "semantic authority rollback: latest batch mismatch".to_string(),
            )),
            None => Err(CoreError::Storage(
                "semantic authority rollback: journal empty".to_string(),
            )),
        }
    }

    pub fn replay_into(
        &self,
        ledger: &Arc<RwLock<Ledger>>,
        builder: &(dyn SemanticBatchBuildPort + Send + Sync),
    ) -> Result<(), CoreError> {
        let batches = {
            let guard = self.journal.read().map_err(|err| {
                CoreError::Storage(format!("semantic authority store poisoned: {err}"))
            })?;
            guard.batches.clone()
        };
        for batch in batches {
            builder.build_batch(&batch)?;
            let mut guard = ledger.write().map_err(|err| {
                CoreError::Storage(format!("semantic authority replay: ledger poisoned: {err}"))
            })?;
            guard.semantic_materialize(batch.generation, Some(batch.manifest_digest.as_str()));
            guard.record_track_materialized(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Semantic,
                batch.generation,
                Some(batch.manifest_digest.as_str()),
            );
            if batch.seal {
                guard.semantic_seal_with_digest(batch.generation, batch.manifest_digest.as_str());
                guard.record_track_seal_with_digest(
                    &batch.repo_id,
                    &batch.revision_id,
                    SearchPlaneTrackKind::Semantic,
                    batch.generation,
                    batch.manifest_digest.as_str(),
                );
            }
        }
        Ok(())
    }
}

/// Direct lexical batch materializer that updates the builder + readiness
/// ledger immediately and keeps the supplied ingest port only as a durability
/// mirror.
pub struct DirectLexicalMaterializer {
    builder: Arc<dyn LexicalBatchBuildPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectLexicalMaterializer {
    #[must_use]
    pub fn new(
        builder: Arc<dyn LexicalBatchBuildPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self { builder, ledger }
    }
}

impl LexicalIngestPort for DirectLexicalMaterializer {
    fn publish_batch(&self, batch: &LexicalIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        self.builder.build_batch(batch)?;
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct lexical materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.apply_lexical_batch(batch)?;
        guard.lexical_materialize(batch.generation, Some(batch.manifest_digest.as_str()));
        guard.record_track_materialized(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Lexical,
            batch.generation,
            Some(batch.manifest_digest.as_str()),
        );
        if batch.seal {
            guard.lexical_seal_with_digest(batch.generation, batch.manifest_digest.as_str());
            guard.record_track_seal_with_digest(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Lexical,
                batch.generation,
                batch.manifest_digest.as_str(),
            );
        }
        drop(guard);
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.manifest_digest.clone());
        for _scope in &batch.replace_scopes {
            receipt.accept_replace_scope();
        }
        for _scope in &batch.tombstone_scopes {
            receipt.accept_tombstone_scope();
        }
        if batch.seal {
            receipt.mark_sealed();
        }
        Ok(receipt)
    }
}

/// Direct semantic batch materializer that updates the semantic builder + the
/// readiness ledger immediately while persisting accepted batches into the
/// semantic authority journal for restart recovery.
pub struct DirectSemanticMaterializer {
    authority_store: Arc<SemanticAuthorityStore>,
    builder: Arc<dyn SemanticBatchBuildPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectSemanticMaterializer {
    #[must_use]
    pub fn new(
        authority_store: Arc<SemanticAuthorityStore>,
        builder: Arc<dyn SemanticBatchBuildPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self {
            authority_store,
            builder,
            ledger,
        }
    }
}

impl SemanticIngestPort for DirectSemanticMaterializer {
    fn publish_batch(&self, batch: &SemanticIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        self.authority_store.append_batch(batch)?;
        if let Err(err) = self.builder.build_batch(batch) {
            self.authority_store.rollback_last_batch(batch)?;
            return Err(err);
        }
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct semantic materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.semantic_materialize(batch.generation, Some(batch.manifest_digest.as_str()));
        guard.record_track_materialized(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Semantic,
            batch.generation,
            Some(batch.manifest_digest.as_str()),
        );
        if batch.seal {
            guard.semantic_seal_with_digest(batch.generation, batch.manifest_digest.as_str());
            guard.record_track_seal_with_digest(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Semantic,
                batch.generation,
                batch.manifest_digest.as_str(),
            );
        }
        drop(guard);
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.manifest_digest.clone());
        for _scope in &batch.replace_scopes {
            receipt.accept_replace_scope();
        }
        for _scope in &batch.tombstone_scopes {
            receipt.accept_tombstone_scope();
        }
        if batch.seal {
            receipt.mark_sealed();
        }
        Ok(receipt)
    }
}

/// Direct history materializer. History is auxiliary and non-activation
/// blocking, but direct ledger updates keep query truth aligned with accepted
/// ingest batches.
pub struct DirectHistoryMaterializer {
    authority_store: Arc<AuxiliaryAuthorityStore>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectHistoryMaterializer {
    #[must_use]
    pub fn new(authority_store: Arc<AuxiliaryAuthorityStore>, ledger: Arc<RwLock<Ledger>>) -> Self {
        Self {
            authority_store,
            ledger,
        }
    }
}

impl HistoryIngestPort for DirectHistoryMaterializer {
    fn publish_batch(&self, batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let mut receipt = BatchPublishReceipt::empty_for(batch.generation, String::new());
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct history materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.apply_history_batch(batch)?;
        for _record in &batch.commits {
            receipt.accept_replace_scope();
        }
        for mutation in &batch.refs {
            match mutation {
                HistoryRefMutation::Upsert(_) => receipt.accept_replace_scope(),
                HistoryRefMutation::Delete(_) => receipt.accept_tombstone_scope(),
            }
        }
        for mutation in &batch.tags {
            match mutation {
                HistoryRefMutation::Upsert(_) => receipt.accept_replace_scope(),
                HistoryRefMutation::Delete(_) => receipt.accept_tombstone_scope(),
            }
        }
        for _record in &batch.diff_hunks {
            receipt.accept_replace_scope();
        }
        self.authority_store.persist_from_ledger(&guard)?;
        drop(guard);
        Ok(receipt)
    }
}

/// Direct dirty-overlay materializer. Dirty state remains auxiliary and
/// non-activation-blocking.
pub struct DirectRuntimeMetadataMaterializer {
    authority_store: Arc<AuxiliaryAuthorityStore>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectRuntimeMetadataMaterializer {
    #[must_use]
    pub fn new(authority_store: Arc<AuxiliaryAuthorityStore>, ledger: Arc<RwLock<Ledger>>) -> Self {
        Self {
            authority_store,
            ledger,
        }
    }
}

impl RuntimeMetadataIngestPort for DirectRuntimeMetadataMaterializer {
    fn publish_batch(&self, batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let mut receipt = BatchPublishReceipt::empty_for(batch.generation, String::new());
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!("direct dirty materialize: ledger poisoned: {err}"))
        })?;
        guard.apply_runtime_batch(batch);
        for entry in &batch.entries {
            match entry {
                DirtyMutation::Upsert(_) => receipt.accept_replace_scope(),
                DirtyMutation::Delete(_) => receipt.accept_tombstone_scope(),
            }
        }
        self.authority_store.persist_from_ledger(&guard)?;
        drop(guard);
        Ok(receipt)
    }
}

/// Direct structural materializer. Structural readiness is first-class and no
/// longer inferred from lexical seal replay; the mirrored lexical channel path
/// is kept only for restart-time authority rebuild.
pub struct DirectStructuralMaterializer {
    authority_store: Arc<AuxiliaryAuthorityStore>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectStructuralMaterializer {
    #[must_use]
    pub fn new(authority_store: Arc<AuxiliaryAuthorityStore>, ledger: Arc<RwLock<Ledger>>) -> Self {
        Self {
            authority_store,
            ledger,
        }
    }
}

impl StructuralIngestPort for DirectStructuralMaterializer {
    fn publish_batch(
        &self,
        batch: &StructuralIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct structural materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.apply_structural_batch(batch)?;
        guard.record_track_materialized(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Structural,
            batch.generation,
            Some(batch.manifest_digest.as_str()),
        );
        let has_parse_trees = guard
            .structural_state(&batch.repo_id, &batch.revision_id, batch.generation)
            .is_some_and(|state| !state.parse_trees().is_empty());
        if batch.seal && has_parse_trees {
            guard.request_structural_seal(&batch.repo_id, &batch.revision_id, batch.generation);
            guard.record_track_seal_with_digest(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Structural,
                batch.generation,
                batch.manifest_digest.as_str(),
            );
        }
        self.authority_store.persist_from_ledger(&guard)?;
        drop(guard);
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.manifest_digest.clone());
        for _scope in &batch.replace_scopes {
            receipt.accept_replace_scope();
        }
        for _scope in &batch.tombstone_scopes {
            receipt.accept_tombstone_scope();
        }
        if batch.seal {
            receipt.mark_sealed();
        }
        Ok(receipt)
    }
}

// =============================================================================
// Top-level dispatcher
// =============================================================================

/// Routes typed ingest requests to the appropriate domain port. Mirrors the
/// shape of [`crate::SearchPlaneControlDispatcher`] / [`crate::SearchPlaneDispatcher`]
/// for the new ingest surface (QI-RT-01).
pub struct SearchPlaneIngestDispatcher {
    lexical: Arc<dyn LexicalIngestPort + Send + Sync>,
    semantic: Arc<dyn SemanticIngestPort + Send + Sync>,
    history: Arc<dyn HistoryIngestPort + Send + Sync>,
    runtime: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
    structural: Arc<dyn StructuralIngestPort + Send + Sync>,
    repomap: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
}

impl SearchPlaneIngestDispatcher {
    #[must_use]
    pub fn new(
        lexical: Arc<dyn LexicalIngestPort + Send + Sync>,
        semantic: Arc<dyn SemanticIngestPort + Send + Sync>,
        history: Arc<dyn HistoryIngestPort + Send + Sync>,
        runtime: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
        structural: Arc<dyn StructuralIngestPort + Send + Sync>,
        repomap: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    ) -> Self {
        Self {
            lexical,
            semantic,
            history,
            runtime,
            structural,
            repomap,
        }
    }

    #[must_use]
    pub fn dispatch(&self, request: SearchPlaneIngestIpcRequest) -> SearchPlaneIngestIpcResponse {
        match request {
            SearchPlaneIngestIpcRequest::PublishLexicalBatch(batch) => {
                match self.lexical.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::LexicalReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishSemanticBatch(batch) => {
                match self.semantic.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::SemanticReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) => {
                match self.history.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::HistoryReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch) => {
                match self.runtime.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::DirtyReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(batch) => {
                match self.structural.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::StructuralReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoMapBundle(bundle) => {
                match self.repomap.ingest_bundle(&bundle) {
                    Ok(()) => SearchPlaneIngestIpcResponse::RepoMapReceipt(RepoMapMutationAck {
                        repo_id: bundle.repo_id,
                        revision_id: bundle.revision_id,
                        manifest_generation: bundle.manifest_generation,
                    }),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            } // QI-LXB-01 / QI-HIST-01 / QI-RT-02 / QI-STR-02: history /
              // dirty / structural batches now have first-class arms above.
              // No fallback arm needed.
        }
    }
}

fn write_cbor<T: Serialize>(path: &Path, value: &T, label: &str) -> Result<(), CoreError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).map_err(|err| {
        CoreError::Storage(format!(
            "search-plane ingest: encode {label} {}: {err}",
            path.display()
        ))
    })?;
    fs::write(path, bytes).map_err(|err| {
        CoreError::Storage(format!(
            "search-plane ingest: write {label} {}: {err}",
            path.display()
        ))
    })
}

fn read_cbor<T: for<'de> Deserialize<'de>>(
    path: &Path,
    label: &str,
) -> Result<Option<T>, CoreError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(CoreError::Storage(format!(
                "search-plane ingest: read {label} {}: {err}",
                path.display()
            )));
        }
    };
    ciborium::from_reader(bytes.as_slice())
        .map(Some)
        .map_err(|err| {
            CoreError::Storage(format!(
                "search-plane ingest: decode {label} {}: {err}",
                path.display()
            ))
        })
}

fn core_error_to_ipc(err: CoreError) -> SearchPlaneIpcError {
    let (code, message) = match err {
        CoreError::InvalidContract(msg) => (ERR_INVALID.to_string(), msg),
        CoreError::Typed { code, message } => (code, message),
        CoreError::NotReady(msg) => (ERR_NOT_READY.to_string(), msg),
        CoreError::NotImplemented(msg) => (ERR_NOT_IMPLEMENTED.to_string(), msg),
        CoreError::NotFound(msg) => (ERR_NOT_FOUND.to_string(), msg),
        CoreError::Storage(msg) => (ERR_INTERNAL.to_string(), msg),
    };
    SearchPlaneIpcError { code, message }
}

// =============================================================================
// Tests — direct authority persistence
// =============================================================================

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, RwLock};

    use super::*;
    use quanta_index_contract::{
        BatchIngestMode, EmbeddingDistanceMetric, EmbeddingId, EmbeddingModelContract,
        EmbeddingNormalization, EmbeddingRecord, ManifestGeneration, OwnerDocKind, RepoId,
        RepoRelativePath, RevisionId, SearchPlaneTrackKind, SearchScopeKey, SearchScopeSurface,
        SemanticIngestBatch, SemanticReplaceScope,
    };

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    #[derive(Default)]
    struct FakeSemanticBuilder {
        batches: Mutex<Vec<SemanticIngestBatch>>,
    }

    impl FakeSemanticBuilder {
        fn take(&self) -> Result<Vec<SemanticIngestBatch>, Box<dyn std::error::Error>> {
            let mut guard = self
                .batches
                .lock()
                .map_err(|err| format!("fake semantic builder poisoned: {err}"))?;
            Ok(std::mem::take(&mut *guard))
        }
    }

    impl SemanticBatchBuildPort for FakeSemanticBuilder {
        fn build_batch(&self, batch: &SemanticIngestBatch) -> Result<(), CoreError> {
            let mut guard = self.batches.lock().map_err(|err| {
                CoreError::Storage(format!("fake semantic builder poisoned: {err}"))
            })?;
            guard.push(batch.clone());
            Ok(())
        }
    }

    fn fixture_scope() -> SearchScopeKey {
        SearchScopeKey {
            doc_surface: SearchScopeSurface::Chunk,
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
        }
    }

    fn fixture_model_contract() -> EmbeddingModelContract {
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

    fn fixture_embedding_record() -> EmbeddingRecord {
        EmbeddingRecord {
            embedding_id: EmbeddingId::new("emb-1"),
            owner_kind: OwnerDocKind::Chunk,
            owner_id: "main".to_string().into_boxed_str(),
            source_doc_id: "chunk-1".to_string().into_boxed_str(),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: quanta_index_contract::lex::LanguageCode::new("rust")
                .expect("hard-coded rust language must remain valid"),
            symbol_kind: None,
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 1,
            snippet: "fn main() {}".to_string().into_boxed_str(),
            embedding_input_digest: "input:abc".to_string().into_boxed_str(),
            vector_digest: "vec:def".to_string().into_boxed_str(),
            view_kind: "raw_chunk".to_string().into_boxed_str(),
            vector: vec![0.1, 0.2, 0.3],
        }
    }

    fn fixture_semantic_batch() -> SemanticIngestBatch {
        SemanticIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
            base_generation: None,
            manifest_digest: "manifest:sem".to_string(),
            batch_digest: "batch:sem".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: fixture_model_contract(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: fixture_scope(),
                scope_digest: "scope:sem".to_string(),
                embeddings: vec![fixture_embedding_record()],
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        }
    }

    #[test]
    fn semantic_authority_store_replays_batches_into_builder_and_ledger() -> TestRes {
        let dir = tempfile::tempdir()?;
        let store = SemanticAuthorityStore::open(dir.path())?;
        let batch = fixture_semantic_batch();
        store.append_batch(&batch)?;

        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let builder = FakeSemanticBuilder::default();
        store.replay_into(&ledger, &builder)?;

        let batches = builder.take()?;
        if batches.as_slice() != [batch.clone()] {
            return Err(format!("unexpected semantic replay batch sequence: {batches:?}").into());
        }
        let guard = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        if guard.track_materialized(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Semantic,
        ) != Some(batch.generation)
        {
            return Err("semantic replay did not record materialized generation".into());
        }
        if guard.track_sealed(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Semantic,
        ) != Some(batch.generation)
        {
            return Err("semantic replay did not record sealed generation".into());
        }
        if guard.track_manifest_digest(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Semantic,
        ) != Some(batch.manifest_digest.as_str())
        {
            return Err("semantic replay did not preserve manifest digest".into());
        }
        Ok(())
    }

    #[test]
    fn direct_semantic_materializer_persists_batches_for_restart_replay() -> TestRes {
        let dir = tempfile::tempdir()?;
        let store = Arc::new(SemanticAuthorityStore::open(dir.path())?);
        let live_builder = Arc::new(FakeSemanticBuilder::default());
        let live_ledger = Arc::new(RwLock::new(Ledger::new()));
        let materializer = DirectSemanticMaterializer::new(
            Arc::clone(&store),
            live_builder,
            Arc::clone(&live_ledger),
        );
        let batch = fixture_semantic_batch();
        let receipt = materializer.publish_batch(&batch)?;
        if !receipt.sealed || receipt.manifest_digest != batch.manifest_digest {
            return Err("unexpected semantic materialize receipt".into());
        }

        let restart_builder = FakeSemanticBuilder::default();
        let restart_ledger = Arc::new(RwLock::new(Ledger::new()));
        store.replay_into(&restart_ledger, &restart_builder)?;
        let guard = restart_ledger
            .read()
            .map_err(|err| format!("restart ledger poisoned: {err}"))?;
        if guard.semantic_sealed() != Some(batch.generation) {
            return Err("restart replay did not restore semantic seal".into());
        }
        Ok(())
    }
}
