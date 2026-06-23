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

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    BatchPublishReceipt, DirtyIngestBatch, DirtyMutation, EmbeddingDistanceMetric, EmbeddingId,
    EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord, HistoryIngestBatch,
    HistoryRefMutation, OwnerDocKind, RepoMapMutationAck, RuntimeCatalogIngestBatch,
    SearchCorpusIngestBatch, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse,
    SearchPlaneIpcError, SearchPlaneTrackKind, SemanticIngestBatch, SemanticReplaceScope,
    StructuralIngestBatch,
};
use quanta_index_core::{
    CoreError, FileContributorIngestPort, FileOwnershipIngestPort, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMapBundleIngestPort, RepoMetaIngestPort, RepoTopicIngestPort,
    SearchCorpusBatchBuildPort, SearchCorpusIngestPort, SemanticBatchBuildPort, SemanticIngestPort,
    TextEmbeddingProvider,
};
use quanta_index_ipc::{decode_cbor_payload, encode_cbor_payload};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::{AuxiliaryAuthorityStore, Ledger};

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";

macro_rules! impl_struct_serde {
    ($ty:ident { $($field:ident : $field_ty:ty),+ $(,)? }) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                let mut state = serializer.serialize_struct(stringify!($ty), FIELDS.len())?;
                $(state.serialize_field(stringify!($field), &self.$field)?;)+
                state.end()
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct StructVisitor;

                impl<'de> Visitor<'de> for StructVisitor {
                    type Value = $ty;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!("struct ", stringify!($ty)))
                    }

                    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
                    where
                        A: MapAccess<'de>,
                    {
                        const FIELDS: &[&str] = &[$(stringify!($field)),+];
                        $(let mut $field: Option<$field_ty> = None;)+
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $(
                                    stringify!($field) => {
                                        if $field.is_some() {
                                            return Err(de::Error::duplicate_field(stringify!($field)));
                                        }
                                        $field = Some(map.next_value()?);
                                    }
                                )+
                                _ => return Err(de::Error::unknown_field(key.as_str(), FIELDS)),
                            }
                        }
                        Ok($ty {
                            $(
                                $field: $field.ok_or_else(|| de::Error::missing_field(stringify!($field)))?,
                            )+
                        })
                    }
                }

                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                deserializer.deserialize_struct(stringify!($ty), FIELDS, StructVisitor)
            }
        }
    };
}

pub trait HistoryIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError>;
}

pub trait RuntimeMetadataIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError>;

    fn publish_catalog_batch(
        &self,
        batch: &RuntimeCatalogIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

pub trait StructuralIngestPort: Send + Sync {
    fn publish_batch(
        &self,
        batch: &StructuralIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

#[derive(Clone, Debug, Default)]
struct SemanticAuthorityJournal {
    batches: Vec<SemanticIngestBatch>,
}

impl_struct_serde!(SemanticAuthorityJournal {
    batches: Vec<SemanticIngestBatch>,
});

/// Legacy semantic journal authority — **migration input only** (LDB-04).
///
/// Before the Lance cutover this was the live semantic durability path
/// (`journal.cbor` plus boot replay). It is now read-only: the durable
/// generation directories under `state_root/indexes/semantic` are the sole
/// serve-time authority. This type exists solely to let a one-shot migration
/// read legacy batches and record an idempotent completion marker. It is never
/// a second live authority.
#[derive(Debug)]
pub struct LegacySemanticJournalStore {
    journal_path: PathBuf,
    migrated_marker_path: PathBuf,
    batches: Vec<SemanticIngestBatch>,
}

impl LegacySemanticJournalStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|err| {
            CoreError::Storage(format!(
                "semantic authority store: create root {}: {err}",
                root.display()
            ))
        })?;
        let journal_path = root.join("journal.cbor");
        let migrated_marker_path = root.join("MIGRATED");
        let journal = read_cbor::<SemanticAuthorityJournal>(&journal_path, "semantic journal")?
            .unwrap_or_default();
        Ok(Self {
            journal_path,
            migrated_marker_path,
            batches: journal.batches,
        })
    }

    /// True when a legacy `journal.cbor` is present on disk.
    #[must_use]
    pub fn has_legacy_journal(&self) -> bool {
        self.journal_path.exists()
    }

    /// True when the one-shot migration completion marker is present.
    #[must_use]
    pub fn migration_complete(&self) -> bool {
        self.migrated_marker_path.exists()
    }

    /// Legacy batches in journal order (empty when no journal exists).
    #[must_use]
    pub fn legacy_batches(&self) -> &[SemanticIngestBatch] {
        &self.batches
    }

    /// Write a legacy-format semantic journal — the inverse of [`Self::open`]'s
    /// read. The steady-state ingest path no longer writes a journal; this is
    /// retained for migration round-trip proofs and offline journal staging.
    pub fn write_legacy_journal(
        root: impl AsRef<Path>,
        batches: &[SemanticIngestBatch],
    ) -> Result<(), CoreError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|err| {
            CoreError::Storage(format!(
                "semantic authority store: create root {}: {err}",
                root.display()
            ))
        })?;
        let journal = SemanticAuthorityJournal {
            batches: batches.to_vec(),
        };
        let bytes = encode_cbor_payload(&journal)
            .map_err(|err| CoreError::Storage(format!("semantic journal: encode: {err}")))?;
        let journal_path = root.join("journal.cbor");
        fs::write(&journal_path, bytes).map_err(|err| {
            CoreError::Storage(format!(
                "semantic journal: write {}: {err}",
                journal_path.display()
            ))
        })
    }

    /// Record idempotent migration completion. The legacy journal is retained
    /// (not deleted) so the only recoverable legacy state survives validation.
    pub fn mark_migration_complete(&self) -> Result<(), CoreError> {
        fs::write(&self.migrated_marker_path, b"migrated").map_err(|err| {
            CoreError::Storage(format!(
                "semantic migration marker {}: {err}",
                self.migrated_marker_path.display()
            ))
        })
    }
}

/// Direct search-corpus batch materializer that updates the builder + readiness
/// ledger immediately and keeps the supplied ingest port only as a durability
/// mirror.
pub struct DirectSearchCorpusMaterializer {
    builder: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
    semantic_ingest: Option<Arc<dyn SemanticIngestPort + Send + Sync>>,
    semantic_embedder: Option<Arc<dyn TextEmbeddingProvider + Send + Sync>>,
}

impl DirectSearchCorpusMaterializer {
    #[must_use]
    pub fn new(
        builder: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self {
            builder,
            ledger,
            semantic_ingest: None,
            semantic_embedder: None,
        }
    }

    #[must_use]
    pub fn new_with_search_owned_semantics(
        builder: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
        semantic_ingest: Arc<dyn SemanticIngestPort + Send + Sync>,
        semantic_embedder: Arc<dyn TextEmbeddingProvider + Send + Sync>,
    ) -> Self {
        Self {
            builder,
            ledger,
            semantic_ingest: Some(semantic_ingest),
            semantic_embedder: Some(semantic_embedder),
        }
    }
}

impl SearchCorpusIngestPort for DirectSearchCorpusMaterializer {
    fn publish_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        let derived_semantic_batch = match (&self.semantic_ingest, &self.semantic_embedder) {
            (Some(_), Some(embedder)) => Some(derive_semantic_batch_from_search_corpus_batch(
                batch,
                embedder.as_ref(),
            )?),
            // Semantics are wired as a pair (ingest + embedder) or not at all.
            _ => None,
        };
        self.builder.build_batch(batch)?;
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct search-corpus materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.apply_search_corpus_batch(batch);
        guard.materialize_track(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Lexical,
            batch.generation,
            Some(batch.manifest_digest.as_str()),
        );
        if batch.seal {
            guard.seal_track_with_digest(
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
        if let (Some(semantic_ingest), Some(semantic_batch)) =
            (&self.semantic_ingest, derived_semantic_batch.as_ref())
        {
            // Search-owned semantic derivation is explicit follow-on work from
            // the accepted search-corpus batch. Failure is surfaced to the
            // caller; no silent downgrade to search-corpus-only indexing occurs.
            drop(semantic_ingest.publish_batch(semantic_batch)?);
        }
        Ok(receipt)
    }
}

pub const SEARCH_OWNED_SEMANTIC_DIMENSION: usize = 64;

/// SSOT for the search-owned hash embedder's model identity.
///
/// Both the corpus derivation contract (here) and the query-time
/// `HashingQueryTextEmbedder` (`query_embedder.rs`) read this so the two sides
/// cannot silently disagree on model identity; query-time enforcement rejects a
/// mismatch (`SEM_MODEL_MISMATCH`).
pub const SEARCH_OWNED_SEMANTIC_MODEL_ID: &str = "search-owned-hash-text-v1";

fn embedding_model_contract_for(
    embedder: &dyn TextEmbeddingProvider,
) -> Result<EmbeddingModelContract, CoreError> {
    let dimension = u32::try_from(embedder.dimension()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "semantic derivation: embedding dimension overflow: {err}"
        ))
    })?;
    let model_id = embedder.model_id();
    Ok(EmbeddingModelContract {
        model_id: model_id.to_string().into_boxed_str(),
        model_version: embedder
            .model_version()
            .map(|version| version.to_string().into_boxed_str()),
        dimension,
        normalization: EmbeddingNormalization::L2Unit,
        distance_metric: EmbeddingDistanceMetric::Cosine,
        policy_digest: format!("{model_id}:chunk.text").into_boxed_str(),
        view_policy_digest: None,
    })
}

fn derive_semantic_batch_from_search_corpus_batch(
    batch: &SearchCorpusIngestBatch,
    embedder: &dyn TextEmbeddingProvider,
) -> Result<SemanticIngestBatch, CoreError> {
    let dimension = embedder.dimension();
    if dimension == 0 {
        return Err(CoreError::InvalidContract(
            "semantic derivation: embedding dimension must be non-zero".to_string(),
        ));
    }
    let model_contract = embedding_model_contract_for(embedder)?;
    // Embed the WHOLE batch in one call: gather every chunk text across ALL
    // scopes and hand them to the embedder together, so a real provider packs
    // them into the fewest token-budget-bounded requests (one network round trip
    // can carry many scopes/files). Embedding per scope instead forces at least
    // one request per scope — in practice one per file — which provider telemetry
    // confirmed dominates ingest cost. The deterministic hash embedder is
    // unaffected: its per-text vectors are identical regardless of batching.
    let all_texts: Vec<&str> = batch
        .replace_scopes
        .iter()
        .flat_map(|scope| scope.chunks.iter().map(|chunk| chunk.text.as_ref()))
        .collect();
    let all_vectors = embedder.embed_batch(&all_texts)?;
    if all_vectors.len() != all_texts.len() {
        return Err(CoreError::InvalidContract(format!(
            "semantic derivation: embedder returned {} vectors for {} batched chunk texts",
            all_vectors.len(),
            all_texts.len()
        )));
    }

    // Redistribute the flat vectors back to their scopes IN ORDER. A draining
    // iterator preserves chunk<->vector alignment without index arithmetic; an
    // underflow (fewer vectors than chunks) and a leftover (more than chunks)
    // both fail closed rather than silently misalign a vector with a chunk.
    let mut vectors = all_vectors.into_iter();
    let replace_scopes = batch
        .replace_scopes
        .iter()
        .map(|scope| {
            let embeddings = scope
                .chunks
                .iter()
                .map(|chunk| {
                    let vector = vectors.next().ok_or_else(|| {
                        CoreError::InvalidContract(
                            "semantic derivation: ran out of embedding vectors while \
                             redistributing the batched embed result across scopes"
                                .to_string(),
                        )
                    })?;
                    embedding_record_for(
                        chunk,
                        semantic_embedding_input_text(chunk),
                        vector,
                        &model_contract,
                    )
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            Ok(SemanticReplaceScope {
                scope: scope.scope.clone(),
                scope_digest: scope.scope_digest.clone(),
                embeddings,
            })
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    if vectors.next().is_some() {
        return Err(CoreError::InvalidContract(
            "semantic derivation: batched embed produced more vectors than the batch had chunks"
                .to_string(),
        ));
    }
    Ok(SemanticIngestBatch {
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id.clone(),
        generation: batch.generation,
        base_generation: batch.base_generation,
        manifest_digest: batch.manifest_digest.clone(),
        batch_digest: format!("{}:semantic-derive", batch.batch_digest),
        mode: batch.mode,
        model_contract,
        replace_scopes,
        tombstone_scopes: batch
            .tombstone_scopes
            .iter()
            .map(|scope| quanta_index_contract::SemanticTombstoneScope {
                scope: scope.scope.clone(),
            })
            .collect(),
        seal: batch.seal,
    })
}

fn embedding_record_for(
    chunk: &quanta_index_contract::ChunkRecord,
    embedding_input_text: &str,
    vector: Vec<f32>,
    model_contract: &EmbeddingModelContract,
) -> Result<EmbeddingRecord, CoreError> {
    let dimension = usize::try_from(model_contract.dimension).map_err(|_err| {
        CoreError::InvalidContract(format!(
            "semantic derivation: model contract dimension {} does not fit usize",
            model_contract.dimension
        ))
    })?;
    if vector.len() != dimension {
        return Err(CoreError::InvalidContract(format!(
            "semantic derivation: embedder returned dim {} for chunk {}, expected {}",
            vector.len(),
            chunk.chunk_id.as_str(),
            model_contract.dimension
        )));
    }
    Ok(EmbeddingRecord {
        embedding_id: EmbeddingId::new(chunk.chunk_id.as_str()),
        owner_kind: OwnerDocKind::Chunk,
        owner_id: chunk.chunk_id.as_str().to_string().into_boxed_str(),
        source_doc_id: chunk.chunk_id.as_str().to_string().into_boxed_str(),
        repo_relative_path: chunk.repo_relative_path.clone(),
        language: chunk.language.clone(),
        symbol_kind: None,
        start_byte: chunk.start_byte,
        end_byte: chunk.end_byte,
        start_line: chunk.start_line,
        end_line: chunk.end_line,
        snippet: chunk.derived_snippet().to_string().into_boxed_str(),
        embedding_input_digest: semantic_embedding_input_digest(
            model_contract,
            "chunk.text",
            embedding_input_text,
        )
        .into_boxed_str(),
        vector_digest: semantic_vector_digest(model_contract, &vector).into_boxed_str(),
        view_kind: "chunk.text".to_string().into_boxed_str(),
        vector,
    })
}

fn semantic_embedding_input_text(chunk: &quanta_index_contract::ChunkRecord) -> &str {
    chunk.text.as_ref()
}

fn semantic_embedding_input_digest(
    model_contract: &EmbeddingModelContract,
    view_kind: &str,
    embedding_input_text: &str,
) -> String {
    let digest = sha256_hex(&[
        model_contract.model_id.as_bytes(),
        model_contract
            .model_version
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
        &model_contract.dimension.to_le_bytes(),
        view_kind.as_bytes(),
        embedding_input_text.as_bytes(),
    ]);
    format!("search-owned-in:sha256:{digest}")
}

fn semantic_vector_digest(model_contract: &EmbeddingModelContract, vector: &[f32]) -> String {
    let mut vector_bytes = Vec::with_capacity(vector.len().saturating_mul(4));
    for value in vector {
        vector_bytes.extend_from_slice(&value.to_le_bytes());
    }
    let digest = sha256_hex(&[
        model_contract.model_id.as_bytes(),
        model_contract
            .model_version
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
        &model_contract.dimension.to_le_bytes(),
        &vector_bytes,
    ]);
    format!("search-owned-vec:sha256:{digest}")
}

fn sha256_hex(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0x1f]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len().saturating_mul(2));
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Direct semantic batch materializer.
///
/// Writes the durable, generation-scoped semantic adapter first (rows on every
/// batch; graph + manifest + seal on `seal`), then updates the readiness
/// ledger. Durability lives entirely in the adapter's generation directories;
/// there is no journal write here. A failed durable write leaves no SEALED
/// marker and does not touch the ledger, so readiness cannot go falsely ready.
pub struct DirectSemanticMaterializer {
    builder: Arc<dyn SemanticBatchBuildPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectSemanticMaterializer {
    #[must_use]
    pub fn new(
        builder: Arc<dyn SemanticBatchBuildPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self { builder, ledger }
    }
}

impl SemanticIngestPort for DirectSemanticMaterializer {
    fn publish_batch(&self, batch: &SemanticIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        self.builder.build_batch(batch)?;
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct semantic materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.materialize_track(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Semantic,
            batch.generation,
            Some(batch.manifest_digest.as_str()),
        );
        if batch.seal {
            guard.seal_track_with_digest(
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

    fn publish_catalog_batch(
        &self,
        batch: &RuntimeCatalogIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.batch_digest.clone());
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct runtime catalog materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.apply_runtime_catalog_batch(batch)?;
        for _record in &batch.changed_entries {
            receipt.accept_replace_scope();
        }
        for _record in &batch.facet_entries {
            receipt.accept_replace_scope();
        }
        for _record in &batch.snapshot_entries {
            receipt.accept_replace_scope();
        }
        for _record in &batch.affected_entries {
            receipt.accept_replace_scope();
        }
        for _record in &batch.invalidated_by_entries {
            receipt.accept_replace_scope();
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
    lexical: Arc<dyn SearchCorpusIngestPort + Send + Sync>,
    history: Arc<dyn HistoryIngestPort + Send + Sync>,
    repo_commit_recency: Arc<dyn RepoCommitRecencyIngestPort + Send + Sync>,
    repo_topic: Arc<dyn RepoTopicIngestPort + Send + Sync>,
    repo_description: Arc<dyn RepoDescriptionIngestPort + Send + Sync>,
    file_ownership: Arc<dyn FileOwnershipIngestPort + Send + Sync>,
    file_contributor: Arc<dyn FileContributorIngestPort + Send + Sync>,
    repo_meta: Arc<dyn RepoMetaIngestPort + Send + Sync>,
    runtime: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
    structural: Arc<dyn StructuralIngestPort + Send + Sync>,
    repomap: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
}

impl SearchPlaneIngestDispatcher {
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "composition-root wiring of one Arc<dyn ...Port> per ingest authority; bundling into a struct is a separate refactor"
    )]
    pub fn new(
        lexical: Arc<dyn SearchCorpusIngestPort + Send + Sync>,
        history: Arc<dyn HistoryIngestPort + Send + Sync>,
        repo_commit_recency: Arc<dyn RepoCommitRecencyIngestPort + Send + Sync>,
        repo_topic: Arc<dyn RepoTopicIngestPort + Send + Sync>,
        repo_description: Arc<dyn RepoDescriptionIngestPort + Send + Sync>,
        file_ownership: Arc<dyn FileOwnershipIngestPort + Send + Sync>,
        file_contributor: Arc<dyn FileContributorIngestPort + Send + Sync>,
        repo_meta: Arc<dyn RepoMetaIngestPort + Send + Sync>,
        runtime: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
        structural: Arc<dyn StructuralIngestPort + Send + Sync>,
        repomap: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    ) -> Self {
        Self {
            lexical,
            history,
            repo_commit_recency,
            repo_topic,
            repo_description,
            file_ownership,
            file_contributor,
            repo_meta,
            runtime,
            structural,
            repomap,
        }
    }

    #[must_use]
    pub fn dispatch(&self, request: SearchPlaneIngestIpcRequest) -> SearchPlaneIngestIpcResponse {
        match request {
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch) => {
                match self.lexical.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) => {
                match self.history.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::HistoryReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(batch) => {
                match self.repo_commit_recency.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(batch) => {
                match self.repo_topic.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoTopicReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(batch) => {
                match self.repo_description.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(batch) => {
                match self.file_ownership.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::FileOwnershipReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(batch) => {
                match self.file_contributor.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(batch) => {
                match self.repo_meta.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoMetaReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch) => {
                match self.runtime.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::DirtyReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(batch) => {
                match self.runtime.publish_catalog_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(receipt),
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
    decode_cbor_payload(bytes.as_slice())
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
    // Ingest failures carry no query-intent repair metadata (J7Q-06 repair is
    // query-route specific); the wire field stays None.
    SearchPlaneIpcError {
        code,
        message,
        repair: None,
    }
}

// =============================================================================
// Tests — direct authority persistence
// =============================================================================

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, RwLock};

    use super::*;
    use quanta_index_contract::{
        BatchIngestMode, ChunkId, ChunkRecord, EmbeddingDistanceMetric, EmbeddingId,
        EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord, ManifestGeneration,
        OwnerDocKind, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
        SearchCorpusReplaceScope, SearchPlaneTrackKind, SearchScopeKey, SearchScopeSurface,
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
            self.batches
                .lock()
                .map_err(|err| {
                    CoreError::Storage(format!("fake semantic builder poisoned: {err}"))
                })?
                .push(batch.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeSearchCorpusBuilder {
        batches: Mutex<Vec<SearchCorpusIngestBatch>>,
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

    fn fixture_embedding_record() -> Result<EmbeddingRecord, Box<dyn std::error::Error>> {
        Ok(EmbeddingRecord {
            embedding_id: EmbeddingId::new("emb-1"),
            owner_kind: OwnerDocKind::Chunk,
            owner_id: "main".to_string().into_boxed_str(),
            source_doc_id: "chunk-1".to_string().into_boxed_str(),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: quanta_index_contract::lex::LanguageCode::new("rust")
                .map_err(str::to_string)?,
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
        })
    }

    fn fixture_semantic_batch() -> Result<SemanticIngestBatch, Box<dyn std::error::Error>> {
        Ok(SemanticIngestBatch {
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
                embeddings: vec![fixture_embedding_record()?],
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        })
    }

    fn fixture_chunk_record() -> Result<ChunkRecord, Box<dyn std::error::Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new("chunk-1"),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: quanta_index_contract::lex::LanguageCode::new("rust")
                .map_err(str::to_string)?,
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

    fn fixture_search_corpus_batch() -> Result<SearchCorpusIngestBatch, Box<dyn std::error::Error>>
    {
        Ok(SearchCorpusIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(7),
            base_generation: None,
            manifest_digest: "manifest:lex".to_string(),
            batch_digest: "batch:lex".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            replace_scopes: vec![SearchCorpusReplaceScope {
                scope: fixture_scope(),
                scope_digest: "scope:lex".to_string(),
                chunks: vec![fixture_chunk_record()?],
                symbols: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        })
    }

    #[test]
    fn legacy_semantic_journal_store_exposes_migration_surface() -> TestRes {
        let dir = tempfile::tempdir()?;
        let store = LegacySemanticJournalStore::open(dir.path())?;
        if store.has_legacy_journal() {
            return Err("fresh store must report no legacy journal".into());
        }
        if !store.legacy_batches().is_empty() {
            return Err("fresh store must expose no legacy batches".into());
        }
        if store.migration_complete() {
            return Err("fresh store must not be marked migrated".into());
        }
        store.mark_migration_complete()?;
        if !store.migration_complete() {
            return Err("migration completion marker must persist".into());
        }
        Ok(())
    }

    #[test]
    fn direct_semantic_materializer_builds_durably_and_marks_ledger() -> TestRes {
        let builder = Arc::new(FakeSemanticBuilder::default());
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let materializer = DirectSemanticMaterializer::new(builder.clone(), Arc::clone(&ledger));
        let batch = fixture_semantic_batch()?;
        let receipt = materializer.publish_batch(&batch)?;
        if !receipt.sealed || receipt.manifest_digest != batch.manifest_digest {
            return Err("unexpected semantic materialize receipt".into());
        }

        // The durable builder received the batch (durability lives in the adapter).
        let built = builder.take()?;
        if built.as_slice() != [batch.clone()] {
            return Err(format!("durable builder did not receive batch: {built:?}").into());
        }

        // Readiness reflects the durable seal, not a journal write.
        let guard = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        if guard.track_sealed(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Semantic,
        ) != Some(batch.generation)
        {
            return Err("publish did not record sealed generation".into());
        }
        if guard.track_manifest_digest(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Semantic,
        ) != Some(batch.manifest_digest.as_str())
        {
            return Err("publish did not preserve manifest digest".into());
        }
        drop(guard);
        Ok(())
    }

    #[test]
    fn search_corpus_materializer_derives_search_owned_semantic_batch() -> TestRes {
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let semantic_ledger = Arc::new(RwLock::new(Ledger::new()));
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
            DirectSemanticMaterializer::new(semantic_builder.clone(), Arc::clone(&semantic_ledger)),
        );
        let search_corpus_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let lexical_ledger = Arc::new(RwLock::new(Ledger::new()));
        let materializer = DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
            search_corpus_builder,
            Arc::clone(&lexical_ledger),
            semantic_materializer,
            Arc::new(crate::HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
        );
        let batch = fixture_search_corpus_batch()?;
        let receipt = materializer.publish_batch(&batch)?;
        if !receipt.sealed {
            return Err("derived semantic search-corpus receipt must preserve seal".into());
        }
        let semantic_batches = semantic_builder.take()?;
        let derived = semantic_batches
            .first()
            .ok_or_else(|| "expected one derived semantic batch".to_string())?;
        if derived.generation != batch.generation || !derived.seal {
            return Err("derived semantic batch lost generation/seal truth".into());
        }
        if derived.replace_scopes.len() != 1 {
            return Err(
                "derived semantic batch did not mirror search-corpus scope/chunk count".into(),
            );
        }
        let scope = derived
            .replace_scopes
            .first()
            .ok_or_else(|| "derived semantic batch missing replace scope".to_string())?;
        if scope.embeddings.len() != 1 {
            return Err("derived semantic batch did not mirror lexical scope/chunk count".into());
        }
        let embedding = scope
            .embeddings
            .first()
            .ok_or_else(|| "derived semantic batch missing embedding".to_string())?;
        if embedding.embedding_id.as_str() != "chunk-1" {
            return Err("derived semantic embedding_id must equal chunk_id".into());
        }
        if !embedding
            .embedding_input_digest
            .starts_with("search-owned-in:sha256:")
        {
            return Err(format!(
                "input digest must be content-hash based, got {}",
                embedding.embedding_input_digest
            )
            .into());
        }
        if !embedding
            .vector_digest
            .starts_with("search-owned-vec:sha256:")
        {
            return Err(format!(
                "vector digest must be vector-hash based, got {}",
                embedding.vector_digest
            )
            .into());
        }
        Ok(())
    }

    struct FixedFakeEmbedder {
        dimension: usize,
        vectors_per_call: usize,
        vector_len: usize,
    }

    impl TextEmbeddingProvider for FixedFakeEmbedder {
        fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
            Ok((0..self.vectors_per_call)
                .map(|_| vec![0.0_f32; self.vector_len])
                .collect())
        }
        fn model_id(&self) -> &str {
            "fake-embedder"
        }
        fn model_version(&self) -> Option<&str> {
            None
        }
        fn dimension(&self) -> usize {
            self.dimension
        }
    }

    fn materializer_with_embedder(
        embedder: Arc<dyn TextEmbeddingProvider + Send + Sync>,
    ) -> DirectSearchCorpusMaterializer {
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let semantic_ledger = Arc::new(RwLock::new(Ledger::new()));
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
            DirectSemanticMaterializer::new(semantic_builder, Arc::clone(&semantic_ledger)),
        );
        let search_corpus_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let lexical_ledger = Arc::new(RwLock::new(Ledger::new()));
        DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
            search_corpus_builder,
            lexical_ledger,
            semantic_materializer,
            embedder,
        )
    }

    // CASE-COVERS: corpus derivation fails closed when the embedder returns the
    // wrong number of vectors — a misaligned batch must never reach the index.
    #[test]
    fn search_corpus_derivation_fails_closed_on_embedder_count_mismatch() -> TestRes {
        let materializer = materializer_with_embedder(Arc::new(FixedFakeEmbedder {
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
            vectors_per_call: 0,
            vector_len: SEARCH_OWNED_SEMANTIC_DIMENSION,
        }));
        let batch = fixture_search_corpus_batch()?;
        match materializer.publish_batch(&batch) {
            Err(CoreError::InvalidContract(message)) => {
                if !message.contains("vectors for") {
                    return Err(format!("unexpected count-mismatch message: {message}").into());
                }
            }
            other => return Err(format!("count mismatch must fail closed, got {other:?}").into()),
        }
        Ok(())
    }

    // CASE-COVERS: corpus derivation fails closed when a returned vector has the
    // wrong dimension — would corrupt the lancedb fixed-size-list schema.
    #[test]
    fn search_corpus_derivation_fails_closed_on_embedder_dim_mismatch() -> TestRes {
        let materializer = materializer_with_embedder(Arc::new(FixedFakeEmbedder {
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
            vectors_per_call: 1,
            vector_len: SEARCH_OWNED_SEMANTIC_DIMENSION + 1,
        }));
        let batch = fixture_search_corpus_batch()?;
        match materializer.publish_batch(&batch) {
            Err(CoreError::InvalidContract(message)) => {
                if !message.contains("returned dim") {
                    return Err(format!("unexpected dim-mismatch message: {message}").into());
                }
            }
            other => return Err(format!("dim mismatch must fail closed, got {other:?}").into()),
        }
        Ok(())
    }

    fn chunk_record_v(
        id: &str,
        path: &str,
        text: &str,
    ) -> Result<ChunkRecord, Box<dyn std::error::Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new(id),
            repo_relative_path: RepoRelativePath::new(path),
            language: quanta_index_contract::lex::LanguageCode::new("rust")
                .map_err(str::to_string)?,
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

    fn scope_with_chunks(
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
    fn multi_scope_corpus_batch() -> Result<SearchCorpusIngestBatch, Box<dyn std::error::Error>> {
        Ok(SearchCorpusIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(7),
            base_generation: None,
            manifest_digest: "manifest:lex".to_string(),
            batch_digest: "batch:lex".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
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
            seal: true,
        })
    }

    // An embedder that counts embed_batch calls and returns one zero vector per
    // input text, so a test can assert how many provider round trips a batch costs.
    struct CountingEmbedder {
        dimension: usize,
        calls: Mutex<usize>,
    }

    impl TextEmbeddingProvider for CountingEmbedder {
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
            {
                let mut calls = self.calls.lock().map_err(|err| {
                    CoreError::InvalidContract(format!("counting embedder lock poisoned: {err}"))
                })?;
                *calls += 1;
            }
            Ok(texts.iter().map(|_| vec![0.0_f32; self.dimension]).collect())
        }
        fn model_id(&self) -> &str {
            "counting-embedder"
        }
        fn model_version(&self) -> Option<&str> {
            None
        }
        fn dimension(&self) -> usize {
            self.dimension
        }
    }

    // CASE-COVERS: a multi-scope ingest batch is embedded in ONE provider call
    // (not one per scope/file), and the flat vectors are redistributed back to
    // each scope's chunks IN ORDER. Reverting the derivation to a per-scope embed
    // makes the call-count assertion fail; misaligning the redistribution makes
    // the chunk-id-order assertion fail.
    #[test]
    fn corpus_derivation_batches_all_scopes_into_one_embed_call() -> TestRes {
        let embedder = Arc::new(CountingEmbedder {
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
            calls: Mutex::new(0),
        });
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let semantic_ledger = Arc::new(RwLock::new(Ledger::new()));
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
            DirectSemanticMaterializer::new(semantic_builder.clone(), Arc::clone(&semantic_ledger)),
        );
        let search_corpus_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let lexical_ledger = Arc::new(RwLock::new(Ledger::new()));
        let materializer = DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
            search_corpus_builder,
            lexical_ledger,
            semantic_materializer,
            embedder.clone(),
        );

        let batch = multi_scope_corpus_batch()?;
        let _receipt = materializer.publish_batch(&batch)?;

        // (1) The whole 3-scope / 5-chunk batch costs exactly ONE embed call.
        let calls = *embedder
            .calls
            .lock()
            .map_err(|err| format!("counting embedder lock poisoned: {err}"))?;
        if calls != 1 {
            return Err(format!(
                "expected ONE batched embed call for the whole batch, got {calls} (per-scope regression)"
            )
            .into());
        }

        // (2) Vectors redistributed back to scopes with chunk counts + order intact.
        let derived = semantic_builder.take()?;
        let derived_batch = derived
            .first()
            .ok_or_else(|| "expected one derived semantic batch".to_string())?;
        let per_scope_counts: Vec<usize> = derived_batch
            .replace_scopes
            .iter()
            .map(|scope| scope.embeddings.len())
            .collect();
        if per_scope_counts != vec![2, 1, 2] {
            return Err(format!(
                "scope->chunk redistribution wrong: {per_scope_counts:?}, expected [2, 1, 2]"
            )
            .into());
        }
        let ids: Vec<&str> = derived_batch
            .replace_scopes
            .iter()
            .flat_map(|scope| scope.embeddings.iter().map(|record| record.embedding_id.as_str()))
            .collect();
        if ids != vec!["a-1", "a-2", "b-1", "c-1", "c-2"] {
            return Err(format!("chunk<->vector alignment lost across scopes: {ids:?}").into());
        }
        Ok(())
    }

    #[test]
    fn semantic_digests_change_when_input_or_vector_changes() -> TestRes {
        let model = fixture_model_contract();
        let chunk_a = fixture_chunk_record()?;
        let mut chunk_b = fixture_chunk_record()?;
        chunk_b.text = "typed semantic parser with different body"
            .to_string()
            .into_boxed_str();
        let input_a = semantic_embedding_input_digest(
            &model,
            "chunk.text",
            semantic_embedding_input_text(&chunk_a),
        );
        let input_b = semantic_embedding_input_digest(
            &model,
            "chunk.text",
            semantic_embedding_input_text(&chunk_b),
        );
        if input_a == input_b {
            return Err("input digest must change when embedding input text changes".into());
        }
        let vec_a = semantic_vector_digest(&model, &[0.1, 0.2, 0.3]);
        let vec_b = semantic_vector_digest(&model, &[0.1, 0.2, 0.4]);
        if vec_a == vec_b {
            return Err("vector digest must change when vector contents change".into());
        }
        if !input_a.starts_with("search-owned-in:sha256:") {
            return Err(format!("unexpected input digest format: {input_a}").into());
        }
        if !vec_a.starts_with("search-owned-vec:sha256:") {
            return Err(format!("unexpected vector digest format: {vec_a}").into());
        }
        Ok(())
    }
}
