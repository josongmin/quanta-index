#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Semantic adapter — channel-fed in-memory HNSW vector index.
//!
//! Implements [`SemanticIndexBuildPort`] and [`SemanticIndexOpenPort`] from
//! `quanta-index-core::domains::semantic`. Embedding payloads on the channel
//! are decoded as CBOR `Vec<f32>` blobs; queries run approximate-nearest-
//! neighbor search over a hand-written HNSW graph (cosine similarity).
//! Replace with Lance / on-disk HNSW without changing the port surface.

mod hnsw;

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    EmbeddingRecord, LexicalCandidate, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SemanticChannelOp,
};
use quanta_index_core::{
    CoreError, SemanticIndexBuildPort, SemanticIndexOpenPort,
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
