#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Semantic adapter — direct-ingest in-memory HNSW vector index.
//!
//! Implements [`SemanticIndexBuildPort`] and [`SemanticIndexOpenPort`] from
//! `quanta-index-core::domains::semantic`. Embedding payloads on the channel
//! are decoded as CBOR `Vec<f32>` blobs; queries run approximate-nearest-
//! neighbor search over a hand-written HNSW graph (cosine similarity).
//! Replace with Lance / on-disk HNSW without changing the port surface.

mod hnsw;

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use quanta_index_contract::channel::SemanticChannelOp;
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    BatchIngestMode, EmbeddingModelContract, EmbeddingRecord, LexicalCandidate, ManifestGeneration,
    ReplaceSemanticScope, RepoId, RepoRelativePath, RevisionId, SemanticIngestBatch, SemanticSeal,
    TombstoneSemanticScope,
};
use quanta_index_core::{
    CoreError, SemanticBatchBuildPort, SemanticIndexBuildPort, SemanticIndexOpenPort,
    domains::semantic::{SemanticPolicy, SemanticSearcher},
};

use crate::hnsw::HnswIndex;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct GenKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
}

/// Per-generation HNSW index. Lazily constructed at the first
/// `UpsertEmbedding` so the embedding dimension is determined by data.
struct GenBucket {
    index: Option<HnswIndex>,
    metadata: BTreeMap<String, EmbeddingMetadata>,
}

impl GenBucket {
    fn new() -> Self {
        Self {
            index: None,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct EmbeddingMetadata {
    repo_relative_path: RepoRelativePath,
    start_line: u32,
    end_line: u32,
    snippet: String,
}

#[derive(Default)]
struct InMemoryEmbeddingStore {
    rows: BTreeMap<GenKey, GenBucket>,
}

impl InMemoryEmbeddingStore {
    fn apply(&mut self, op: &SemanticChannelOp) -> Result<(), CoreError> {
        let key = GenKey {
            repo_id: op.repo_id().clone(),
            revision_id: op.revision_id().clone(),
            generation: op.generation(),
        };
        match op {
            SemanticChannelOp::FullBundle(_) | SemanticChannelOp::Seal(_) => {
                // Ensure the bucket exists so an empty seal still surfaces a
                // generation entry.
                self.ensure_bucket(key);
            }
            SemanticChannelOp::UpsertEmbedding(upsert) => {
                let payload = decode_embedding_payload(&upsert.payload)?;
                let vector = payload.vector;
                SemanticPolicy::validate_query_vector(&vector)?;
                let bucket = self.rows.entry(key).or_insert_with(GenBucket::new);
                if bucket.index.is_none() {
                    bucket.index = Some(HnswIndex::new(vector.len()));
                }
                if let Some(index) = bucket.index.as_mut() {
                    index.insert(upsert.embedding_id.as_str().to_string(), &vector)?;
                }
                let _prior: Option<EmbeddingMetadata> = bucket.metadata.insert(
                    upsert.embedding_id.as_str().to_string(),
                    EmbeddingMetadata {
                        repo_relative_path: payload.repo_relative_path,
                        start_line: payload.start_line,
                        end_line: payload.end_line,
                        snippet: payload.snippet,
                    },
                );
            }
            SemanticChannelOp::DeleteEmbedding(delete) => {
                if let Some(bucket) = self.rows.get_mut(&key) {
                    if let Some(index) = bucket.index.as_mut() {
                        index.delete(delete.embedding_id.as_str());
                    }
                    let _prior: Option<EmbeddingMetadata> =
                        bucket.metadata.remove(delete.embedding_id.as_str());
                }
            }
            SemanticChannelOp::ReplaceSemanticScope(payload) => {
                let (_mode, _base_generation, model_contract, scope) =
                    decode_replace_scope_payload(&payload.payload)?;
                let bucket = self.rows.entry(key).or_insert_with(GenBucket::new);
                remove_scope_entries(bucket, &scope.scope.repo_relative_path);
                for embedding in &scope.embeddings {
                    let vector = embedding.vector.clone();
                    SemanticPolicy::validate_query_vector(&vector)?;
                    let expected_dim =
                        usize::try_from(model_contract.dimension).map_err(|err| {
                            CoreError::InvalidContract(format!(
                                "semantic: model contract dimension overflow: {err}"
                            ))
                        })?;
                    if vector.len() != expected_dim {
                        return Err(CoreError::InvalidContract(format!(
                            "semantic: embedding {} dim {} != contract dim {}",
                            embedding.embedding_id.as_str(),
                            vector.len(),
                            expected_dim
                        )));
                    }
                    if bucket.index.is_none() {
                        bucket.index = Some(HnswIndex::new(vector.len()));
                    }
                    if let Some(index) = bucket.index.as_mut() {
                        index.insert(embedding.embedding_id.as_str().to_string(), &vector)?;
                    }
                    let _prior: Option<EmbeddingMetadata> = bucket.metadata.insert(
                        embedding.embedding_id.as_str().to_string(),
                        EmbeddingMetadata {
                            repo_relative_path: embedding.repo_relative_path.clone(),
                            start_line: embedding.start_line,
                            end_line: embedding.end_line,
                            snippet: embedding.snippet.as_ref().to_string(),
                        },
                    );
                }
            }
            SemanticChannelOp::TombstoneSemanticScope(payload) => {
                let (_mode, _base_generation, _model_contract, scope) =
                    decode_tombstone_scope_payload(&payload.payload)?;
                if let Some(bucket) = self.rows.get_mut(&key) {
                    remove_scope_entries(bucket, &scope.scope.repo_relative_path);
                }
            }
        }
        Ok(())
    }

    fn ensure_bucket(&mut self, key: GenKey) {
        // entry API guarantees a single map traversal vs contains_key + insert;
        // we discard the &mut V handle because we only need the side-effect.
        let _bucket: &mut GenBucket = self.rows.entry(key).or_insert_with(GenBucket::new);
    }
}

struct DecodedEmbeddingPayload {
    repo_relative_path: RepoRelativePath,
    start_line: u32,
    end_line: u32,
    snippet: String,
    vector: Vec<f32>,
}

fn remove_scope_entries(bucket: &mut GenBucket, repo_relative_path: &RepoRelativePath) {
    let remove_ids: Vec<String> = bucket
        .metadata
        .iter()
        .filter(|(_candidate_id, metadata)| metadata.repo_relative_path == *repo_relative_path)
        .map(|(candidate_id, _metadata)| candidate_id.clone())
        .collect();
    if let Some(index) = bucket.index.as_mut() {
        for candidate_id in &remove_ids {
            index.delete(candidate_id);
        }
    }
    for candidate_id in remove_ids {
        let _prior = bucket.metadata.remove(candidate_id.as_str());
    }
}

fn encode_cbor<T>(value: &T, label: &str) -> Result<Vec<u8>, CoreError>
where
    T: serde::Serialize,
{
    let mut payload = Vec::new();
    ciborium::into_writer(value, &mut payload)
        .map_err(|err| CoreError::InvalidContract(format!("semantic: encode {label}: {err}")))?;
    Ok(payload)
}

fn legacy_ops_for_batch(
    batch: &SemanticIngestBatch,
    include_seal: bool,
) -> Result<Vec<SemanticChannelOp>, CoreError> {
    let op_capacity = batch
        .replace_scopes
        .len()
        .saturating_add(batch.tombstone_scopes.len())
        .saturating_add(usize::from(include_seal));
    let mut ops = Vec::with_capacity(op_capacity);
    for scope in &batch.replace_scopes {
        ops.push(SemanticChannelOp::ReplaceSemanticScope(
            ReplaceSemanticScope {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                payload: encode_cbor(
                    &(
                        batch.mode,
                        batch.base_generation,
                        batch.model_contract.clone(),
                        scope.clone(),
                    ),
                    "replace semantic scope payload",
                )?,
            },
        ));
    }
    for scope in &batch.tombstone_scopes {
        ops.push(SemanticChannelOp::TombstoneSemanticScope(
            TombstoneSemanticScope {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                payload: encode_cbor(
                    &(
                        batch.mode,
                        batch.base_generation,
                        batch.model_contract.clone(),
                        scope.clone(),
                    ),
                    "tombstone semantic scope payload",
                )?,
            },
        ));
    }
    if include_seal {
        ops.push(SemanticChannelOp::Seal(SemanticSeal {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        }));
    }
    Ok(ops)
}

fn decode_replace_scope_payload(
    bytes: &[u8],
) -> Result<
    (
        BatchIngestMode,
        Option<ManifestGeneration>,
        EmbeddingModelContract,
        quanta_index_contract::SemanticReplaceScope,
    ),
    CoreError,
> {
    ciborium::from_reader::<
        (
            BatchIngestMode,
            Option<ManifestGeneration>,
            EmbeddingModelContract,
            quanta_index_contract::SemanticReplaceScope,
        ),
        _,
    >(bytes)
    .map_err(|err| {
        CoreError::InvalidContract(format!("semantic: replace scope payload decode: {err}"))
    })
}

fn decode_tombstone_scope_payload(
    bytes: &[u8],
) -> Result<
    (
        BatchIngestMode,
        Option<ManifestGeneration>,
        EmbeddingModelContract,
        quanta_index_contract::SemanticTombstoneScope,
    ),
    CoreError,
> {
    ciborium::from_reader::<
        (
            BatchIngestMode,
            Option<ManifestGeneration>,
            EmbeddingModelContract,
            quanta_index_contract::SemanticTombstoneScope,
        ),
        _,
    >(bytes)
    .map_err(|err| {
        CoreError::InvalidContract(format!("semantic: tombstone scope payload decode: {err}"))
    })
}

fn decode_embedding_payload(bytes: &[u8]) -> Result<DecodedEmbeddingPayload, CoreError> {
    if let Ok(record) = ciborium::from_reader::<EmbeddingRecord, _>(bytes) {
        return Ok(DecodedEmbeddingPayload {
            repo_relative_path: record.repo_relative_path,
            start_line: record.start_line,
            end_line: record.end_line,
            snippet: record.snippet.into(),
            vector: record.vector,
        });
    }
    if bytes.is_empty() {
        return Ok(DecodedEmbeddingPayload {
            repo_relative_path: RepoRelativePath::new(""),
            start_line: 0,
            end_line: 0,
            snippet: String::new(),
            vector: Vec::new(),
        });
    }
    let vector = ciborium::from_reader::<Vec<f32>, _>(bytes)
        .map_err(|err| CoreError::InvalidContract(format!("semantic payload decode: {err}")))?;
    Ok(DecodedEmbeddingPayload {
        repo_relative_path: RepoRelativePath::new(""),
        start_line: 0,
        end_line: 0,
        snippet: String::new(),
        vector,
    })
}

pub struct SemanticAdapter {
    store: Arc<RwLock<InMemoryEmbeddingStore>>,
}

impl SemanticAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(InMemoryEmbeddingStore::default())),
        }
    }
}

impl Default for SemanticAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticBatchBuildPort for SemanticAdapter {
    fn build_batch(&self, batch: &SemanticIngestBatch) -> Result<(), CoreError> {
        let ops = legacy_ops_for_batch(batch, batch.seal)?;
        self.apply_all(&ops)
    }
}

impl SemanticIndexBuildPort for SemanticAdapter {
    fn build(
        &self,
        _repo: &RepoId,
        _revision: &RevisionId,
        _generation: ManifestGeneration,
        ops: &[SemanticChannelOp],
    ) -> Result<(), CoreError> {
        self.apply_all(ops)
    }
}

impl SemanticAdapter {
    fn apply_all(&self, ops: &[SemanticChannelOp]) -> Result<(), CoreError> {
        let mut store = self
            .store
            .write()
            .map_err(|err| CoreError::Storage(format!("semantic store poisoned: {err}")))?;
        for op in ops {
            store.apply(op)?;
        }
        drop(store);
        Ok(())
    }
}

impl SemanticIndexOpenPort for SemanticAdapter {
    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
        Ok(Box::new(SnapshotSemanticSearcher {
            store: Arc::clone(&self.store),
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
        }))
    }
}

struct SnapshotSemanticSearcher {
    store: Arc<RwLock<InMemoryEmbeddingStore>>,
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
}

impl SemanticSearcher for SnapshotSemanticSearcher {
    fn search(&self, query_vector: &[f32], top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_top_k(top_k)?;
        SemanticPolicy::validate_query_vector(query_vector)?;
        let limit = match usize::try_from(top_k) {
            Ok(v) => v,
            Err(err) => {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: top_k overflow: {err}"
                )));
            }
        };
        let hits = self.collect_hits(query_vector, limit)?;
        let mut out: Vec<LexicalCandidate> = Vec::with_capacity(hits.len());
        for (id, score) in hits {
            let metadata = self.lookup_metadata(&id)?;
            let candidate = LexicalCandidate {
                candidate_id: id,
                repo_id: self.repo_id.clone(),
                revision_id: self.revision_id.clone(),
                manifest_generation: self.generation,
                repo_relative_path: metadata.repo_relative_path,
                start_line: metadata.start_line,
                end_line: metadata.end_line,
                score,
                snippet: metadata.snippet,
            };
            out.push(candidate);
        }
        Ok(out)
    }

    fn search_scoped(
        &self,
        query_vector: &[f32],
        allowed_ids: &std::collections::BTreeSet<String>,
        top_k: u32,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_top_k(top_k)?;
        SemanticPolicy::validate_query_vector(query_vector)?;
        let limit = match usize::try_from(top_k) {
            Ok(v) => v,
            Err(err) => {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: top_k overflow: {err}"
                )));
            }
        };
        let hits = self.collect_hits_scoped(query_vector, allowed_ids, limit)?;
        let mut out: Vec<LexicalCandidate> = Vec::with_capacity(hits.len());
        for (id, score) in hits {
            let metadata = self.lookup_metadata(&id)?;
            out.push(LexicalCandidate {
                candidate_id: id,
                repo_id: self.repo_id.clone(),
                revision_id: self.revision_id.clone(),
                manifest_generation: self.generation,
                repo_relative_path: metadata.repo_relative_path,
                start_line: metadata.start_line,
                end_line: metadata.end_line,
                score,
                snippet: metadata.snippet,
            });
        }
        Ok(out)
    }

    fn resolve_handle(&self, handle: &str) -> Result<Vec<f32>, CoreError> {
        let key = GenKey {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
        };
        let vector: Option<Vec<f32>> = {
            let store = self
                .store
                .read()
                .map_err(|err| CoreError::Storage(format!("semantic store poisoned: {err}")))?;
            store
                .rows
                .get(&key)
                .and_then(|bucket| bucket.index.as_ref())
                .and_then(|index| index.resolve_handle(handle))
                .map(<[f32]>::to_vec)
        };
        vector.ok_or_else(|| CoreError::Typed {
            code: "SEM_HANDLE_NOT_FOUND".to_string(),
            message: format!(
                "semantic: handle `{handle}` not found for generation {}",
                self.generation.get()
            ),
        })
    }
}

impl SnapshotSemanticSearcher {
    fn lookup_metadata(&self, candidate_id: &str) -> Result<EmbeddingMetadata, CoreError> {
        let key = GenKey {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
        };
        let outcome: Option<Option<EmbeddingMetadata>> = {
            let store = self
                .store
                .read()
                .map_err(|err| CoreError::Storage(format!("semantic store poisoned: {err}")))?;
            store
                .rows
                .get(&key)
                .map(|bucket| bucket.metadata.get(candidate_id).cloned())
        };
        match outcome {
            Some(Some(meta)) => Ok(meta),
            None => Err(CoreError::Storage(format!(
                "semantic metadata missing bucket for generation {}",
                self.generation.get()
            ))),
            Some(None) => Err(CoreError::Storage(format!(
                "semantic metadata missing candidate `{candidate_id}` at generation {}",
                self.generation.get()
            ))),
        }
    }

    fn collect_hits(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<(String, f32)>, CoreError> {
        let store = self
            .store
            .read()
            .map_err(|err| CoreError::Storage(format!("semantic store poisoned: {err}")))?;
        let key = GenKey {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
        };
        // Fail-closed on dim mismatch: silently returning an empty result
        // would let a query with the wrong embedder model claim "no matches"
        // when the real issue is a shape disagreement between producer and
        // searcher.
        let Some(bucket) = store.rows.get(&key) else {
            return Ok(Vec::new());
        };
        let Some(index) = bucket.index.as_ref() else {
            return Ok(Vec::new());
        };
        if index.dim() != query_vector.len() {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::SemDimMismatch.as_code_str().to_string(),
                message: format!(
                    "semantic: query vector dim {} does not match index dim {} for generation {}",
                    query_vector.len(),
                    index.dim(),
                    self.generation.get()
                ),
            });
        }
        let hits = index.search(query_vector, limit);
        drop(store);
        Ok(hits)
    }

    fn collect_hits_scoped(
        &self,
        query_vector: &[f32],
        allowed_ids: &std::collections::BTreeSet<String>,
        limit: usize,
    ) -> Result<Vec<(String, f32)>, CoreError> {
        let store = self
            .store
            .read()
            .map_err(|err| CoreError::Storage(format!("semantic store poisoned: {err}")))?;
        let key = GenKey {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
        };
        let Some(bucket) = store.rows.get(&key) else {
            return Ok(Vec::new());
        };
        let Some(index) = bucket.index.as_ref() else {
            return Ok(Vec::new());
        };
        if index.dim() != query_vector.len() {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::SemDimMismatch.as_code_str().to_string(),
                message: format!(
                    "semantic: query vector dim {} does not match index dim {} for generation {}",
                    query_vector.len(),
                    index.dim(),
                    self.generation.get()
                ),
            });
        }
        let hits = index.search_scoped(query_vector, allowed_ids, limit);
        drop(store);
        Ok(hits)
    }
}
