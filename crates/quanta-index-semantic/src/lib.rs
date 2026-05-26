#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Semantic adapter — direct-ingest in-memory HNSW vector index.
//!
//! Implements the batch-native semantic build/open ports from
//! `quanta-index-core::domains::semantic`. Queries run approximate-nearest-
//! neighbor search over a hand-written HNSW graph (cosine similarity).
//! Replace with Lance / on-disk HNSW without changing the port surface.

mod hnsw;

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    EmbeddingRecord, LexicalCandidate, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SemanticIngestBatch,
};
use quanta_index_core::{
    CoreError, SemanticBatchBuildPort, SemanticIndexOpenPort,
    domains::semantic::{SemanticPolicy, SemanticSearcher},
};

use crate::hnsw::HnswIndex;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct GenKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
}

/// Per-generation HNSW index. Lazily constructed at the first embedding
/// record in a batch so the embedding dimension is determined by data.
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
    fn apply_batch(&mut self, batch: &SemanticIngestBatch) -> Result<(), CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let expected_dim = usize::try_from(batch.model_contract.dimension).map_err(|err| {
            CoreError::InvalidContract(format!(
                "semantic: model contract dimension overflow: {err}"
            ))
        })?;
        for scope in &batch.replace_scopes {
            let bucket = self.rows.entry(key.clone()).or_insert_with(GenBucket::new);
            remove_scope_entries(bucket, &scope.scope.repo_relative_path);
            for embedding in &scope.embeddings {
                upsert_embedding(bucket, embedding, expected_dim)?;
            }
        }
        for scope in &batch.tombstone_scopes {
            if let Some(bucket) = self.rows.get_mut(&key) {
                remove_scope_entries(bucket, &scope.scope.repo_relative_path);
            }
        }
        if batch.seal {
            self.ensure_bucket(key);
        }
        Ok(())
    }

    fn ensure_bucket(&mut self, key: GenKey) {
        // entry API guarantees a single map traversal vs contains_key + insert;
        // we discard the &mut V handle because we only need the side-effect.
        let _bucket: &mut GenBucket = self.rows.entry(key).or_insert_with(GenBucket::new);
    }
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

fn upsert_embedding(
    bucket: &mut GenBucket,
    embedding: &EmbeddingRecord,
    expected_dim: usize,
) -> Result<(), CoreError> {
    let vector = embedding.vector.clone();
    SemanticPolicy::validate_query_vector(&vector)?;
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
    Ok(())
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
        let mut store = self
            .store
            .write()
            .map_err(|err| CoreError::Storage(format!("semantic store poisoned: {err}")))?;
        store.apply_batch(batch)?;
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
        let bucket = store.rows.get(&key).ok_or_else(|| {
            CoreError::Storage(format!(
                "semantic generation {} bucket missing",
                self.generation.get()
            ))
        })?;
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
        let bucket = store.rows.get(&key).ok_or_else(|| {
            CoreError::Storage(format!(
                "semantic generation {} bucket missing",
                self.generation.get()
            ))
        })?;
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

#[cfg(test)]
mod tests {
    use super::SemanticAdapter;
    use quanta_index_contract::{
        BatchIngestMode, EmbeddingDistanceMetric, EmbeddingId, EmbeddingModelContract,
        EmbeddingNormalization, EmbeddingRecord, ManifestGeneration, OwnerDocKind, RepoId,
        RepoRelativePath, RevisionId, SearchScopeKey, SearchScopeSurface, SemanticIngestBatch,
        SemanticReplaceScope, SemanticTombstoneScope, lex::LanguageCode,
    };
    use quanta_index_core::{CoreError, SemanticBatchBuildPort, SemanticIndexOpenPort};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn repo_id() -> RepoId {
        RepoId::new("repo-sem")
    }

    fn revision_id() -> RevisionId {
        RevisionId::new("rev-sem")
    }

    fn generation() -> ManifestGeneration {
        ManifestGeneration::new(7)
    }

    fn model_contract(dimension: u32) -> EmbeddingModelContract {
        EmbeddingModelContract {
            model_id: "text-embed".to_string().into_boxed_str(),
            model_version: Some("1".to_string().into_boxed_str()),
            dimension,
            normalization: EmbeddingNormalization::L2Unit,
            distance_metric: EmbeddingDistanceMetric::Cosine,
            policy_digest: "policy:feed".to_string().into_boxed_str(),
            view_policy_digest: Some("view:feed".to_string().into_boxed_str()),
        }
    }

    fn scope(path: &str) -> SearchScopeKey {
        SearchScopeKey {
            doc_surface: SearchScopeSurface::Chunk,
            repo_relative_path: RepoRelativePath::new(path),
        }
    }

    fn embedding_record(
        id: &str,
        path: &str,
        vector: Vec<f32>,
    ) -> Result<EmbeddingRecord, Box<dyn std::error::Error>> {
        let language = LanguageCode::new("rust").map_err(|err| {
            format!("fixture language `rust` must stay valid for semantic adapter tests: {err}")
        })?;
        Ok(EmbeddingRecord {
            embedding_id: EmbeddingId::new(id),
            owner_kind: OwnerDocKind::Chunk,
            owner_id: format!("owner-{id}").into_boxed_str(),
            source_doc_id: format!("doc-{id}").into_boxed_str(),
            repo_relative_path: RepoRelativePath::new(path),
            language,
            symbol_kind: None,
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 3,
            snippet: format!("fn {id}() {{}}").into_boxed_str(),
            embedding_input_digest: format!("input:{id}").into_boxed_str(),
            vector_digest: format!("vector:{id}").into_boxed_str(),
            view_kind: "raw_chunk".to_string().into_boxed_str(),
            vector,
        })
    }

    fn batch_for_embeddings(
        generation: ManifestGeneration,
        path: &str,
        embeddings: Vec<EmbeddingRecord>,
        contract_dimension: u32,
    ) -> SemanticIngestBatch {
        SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:feed".to_string(),
            batch_digest: format!("batch:{}:{path}", generation.get()),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(contract_dimension),
            replace_scopes: vec![SemanticReplaceScope {
                scope: scope(path),
                scope_digest: format!("scope:{path}"),
                embeddings,
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        }
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts semantic adapter search results via assert macros"
    )]
    fn build_batch_directly_indexes_embeddings() -> TestResult {
        let adapter = SemanticAdapter::new();
        let batch = batch_for_embeddings(
            generation(),
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        );

        adapter.build_batch(&batch)?;
        let searcher = adapter.open(&repo_id(), &revision_id(), generation())?;
        let hits = searcher.search(&[1.0, 0.0, 0.0], 1)?;
        let Some(hit) = hits.first() else {
            return Err("semantic search must return one hit for identical vector".into());
        };
        assert_eq!(hits.len(), 1);
        assert_eq!(hit.candidate_id.as_str(), "emb-1");
        assert_eq!(hit.repo_relative_path.as_str(), "src/main.rs");
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts replacement semantic adapter search result via assert macros"
    )]
    fn build_batch_replace_scope_overwrites_same_path_entries() -> TestResult {
        let adapter = SemanticAdapter::new();
        let first = batch_for_embeddings(
            generation(),
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        );
        let second = batch_for_embeddings(
            generation(),
            "src/main.rs",
            vec![embedding_record(
                "emb-2",
                "src/main.rs",
                vec![0.0, 1.0, 0.0],
            )?],
            3,
        );

        adapter.build_batch(&first)?;
        adapter.build_batch(&second)?;
        let searcher = adapter.open(&repo_id(), &revision_id(), generation())?;
        let hits = searcher.search(&[0.0, 1.0, 0.0], 1)?;
        let Some(hit) = hits.first() else {
            return Err("replacement semantic search must return one hit".into());
        };
        assert_eq!(hits.len(), 1);
        assert_eq!(hit.candidate_id.as_str(), "emb-2");
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts exact invalid-contract message via assert macros"
    )]
    fn build_batch_rejects_contract_dimension_mismatch() -> TestResult {
        let adapter = SemanticAdapter::new();
        let batch = batch_for_embeddings(
            generation(),
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            2,
        );

        let err = match adapter.build_batch(&batch) {
            Ok(()) => return Err("contract dimension mismatch must fail closed".into()),
            Err(err) => err,
        };
        match err {
            CoreError::InvalidContract(message) => {
                assert!(message.contains("embedding emb-1 dim 3 != contract dim 2"));
            }
            other @ (CoreError::Typed { .. }
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => {
                return Err(format!("expected invalid-contract error, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts tombstone search empties the semantic scope"
    )]
    fn build_batch_tombstone_scope_removes_existing_entries() -> TestResult {
        let adapter = SemanticAdapter::new();
        let initial = batch_for_embeddings(
            generation(),
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        );
        let tombstone = SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            base_generation: Some(generation()),
            manifest_digest: "manifest:feed".to_string(),
            batch_digest: "batch:tombstone".to_string(),
            mode: BatchIngestMode::Delta,
            model_contract: model_contract(3),
            replace_scopes: Vec::new(),
            tombstone_scopes: vec![SemanticTombstoneScope {
                scope: scope("src/main.rs"),
            }],
            seal: true,
        };

        adapter.build_batch(&initial)?;
        adapter.build_batch(&tombstone)?;
        let searcher = adapter.open(&repo_id(), &revision_id(), generation())?;
        let hits = searcher.search(&[1.0, 0.0, 0.0], 1)?;
        assert!(hits.is_empty());
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts exact storage error surface via assert macros"
    )]
    fn missing_generation_bucket_fails_closed_on_search() -> TestResult {
        let adapter = SemanticAdapter::new();
        let searcher = adapter.open(&repo_id(), &revision_id(), generation())?;

        let Err(err) = searcher.search(&[1.0, 0.0, 0.0], 1) else {
            return Err("missing generation bucket must fail closed".into());
        };
        match err {
            CoreError::Storage(message) => {
                assert!(message.contains("semantic generation 7 bucket missing"));
            }
            other @ (CoreError::InvalidContract(_)
            | CoreError::Typed { .. }
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)) => {
                return Err(format!("expected storage error, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts sealed empty generation returns no hits"
    )]
    fn sealed_empty_generation_returns_empty_hits() -> TestResult {
        let adapter = SemanticAdapter::new();
        let empty = SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            base_generation: None,
            manifest_digest: "manifest:feed".to_string(),
            batch_digest: "batch:empty".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(3),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            seal: true,
        };

        adapter.build_batch(&empty)?;
        let searcher = adapter.open(&repo_id(), &revision_id(), generation())?;
        let hits = searcher.search(&[1.0, 0.0, 0.0], 1)?;
        assert!(hits.is_empty());
        Ok(())
    }
}
