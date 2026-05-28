//! Durable open + search path.
//!
//! `open_generation` loads exactly one sealed generation directly from disk —
//! manifest, columnar rows, and the persisted HNSW graph — validating scope,
//! shape, and content checksum before it can serve. There is no cross-
//! generation replay and no rebuild of the graph: the open cost is bounded by
//! the single generation being served.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    LexicalCandidate, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
};
use quanta_index_core::CoreError;
use quanta_index_core::domains::semantic::{SemanticPolicy, SemanticSearcher};

use crate::dataset::DatasetShard;
use crate::hnsw::HnswIndex;
use crate::manifest::SemanticManifest;
use crate::{codec, graph, layout};

struct RowMeta {
    repo_relative_path: RepoRelativePath,
    start_line: u32,
    end_line: u32,
    snippet: String,
}

/// One sealed generation loaded into memory from durable state.
pub(crate) struct LoadedGeneration {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    /// `None` for an explicit sealed-empty generation.
    graph: Option<HnswIndex>,
    metadata: BTreeMap<String, RowMeta>,
}

/// Open a sealed generation directly from durable state, failing closed on any
/// absent marker, scope mismatch, shape mismatch, or content corruption.
pub(crate) fn open_generation(
    semantic_root: &Path,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
) -> Result<LoadedGeneration, CoreError> {
    let generation_dir = layout::generation_dir(semantic_root, repo, revision, generation);
    if !layout::sealed_marker_path(&generation_dir).exists() {
        return Err(CoreError::NotReady(format!(
            "semantic: generation {} for repo={} revision={} is not sealed (or absent)",
            generation.get(),
            repo.as_str(),
            revision.as_str()
        )));
    }

    let manifest_path = layout::manifest_path(&generation_dir);
    let manifest_bytes = fs::read(&manifest_path).map_err(|err| {
        CoreError::Storage(format!(
            "semantic: read manifest {}: {err}",
            manifest_path.display()
        ))
    })?;
    let manifest = SemanticManifest::decode(&manifest_bytes)?;
    manifest.validate_scope(repo, revision, generation)?;

    let rows_path = layout::rows_path(&generation_dir);
    let rows_bytes = fs::read(&rows_path).map_err(|err| {
        CoreError::Storage(format!(
            "semantic: read dataset {}: {err}",
            rows_path.display()
        ))
    })?;
    let shard = DatasetShard::decode(&rows_bytes)?;
    if shard.dimension != manifest.dimension {
        return Err(CoreError::Storage(format!(
            "semantic: dataset dimension {} != manifest dimension {}",
            shard.dimension, manifest.dimension
        )));
    }
    let shard_row_count = u64::try_from(shard.rows.len()).map_err(|err| {
        CoreError::Storage(format!("semantic: dataset row count overflow: {err}"))
    })?;
    if shard_row_count != manifest.row_count {
        return Err(CoreError::Storage(format!(
            "semantic: dataset row count {shard_row_count} != manifest row count {}",
            manifest.row_count
        )));
    }

    let (graph, graph_bytes) = if manifest.row_count == 0 {
        (None, Vec::new())
    } else {
        let graph_path = layout::graph_path(&generation_dir);
        let graph_bytes = fs::read(&graph_path).map_err(|err| {
            CoreError::Storage(format!(
                "semantic: read graph {}: {err}",
                graph_path.display()
            ))
        })?;
        let index = graph::decode_graph(&graph_bytes)?;
        let manifest_dim = usize::try_from(manifest.dimension).map_err(|err| {
            CoreError::Storage(format!("semantic: manifest dimension overflow: {err}"))
        })?;
        if index.dim() != manifest_dim {
            return Err(CoreError::Storage(format!(
                "semantic: graph dimension {} != manifest dimension {manifest_dim}",
                index.dim()
            )));
        }
        (Some(index), graph_bytes)
    };

    let recomputed = codec::content_checksum(&[&rows_bytes, &graph_bytes]);
    if recomputed != manifest.content_checksum {
        return Err(CoreError::Storage(format!(
            "semantic: content checksum mismatch for generation {} (manifest {}, recomputed {recomputed})",
            generation.get(),
            manifest.content_checksum
        )));
    }

    let mut metadata: BTreeMap<String, RowMeta> = BTreeMap::new();
    for row in shard.rows {
        let meta = RowMeta {
            repo_relative_path: RepoRelativePath::new(row.repo_relative_path),
            start_line: row.start_line,
            end_line: row.end_line,
            snippet: row.snippet,
        };
        let _prior = metadata.insert(row.embedding_id, meta);
    }

    Ok(LoadedGeneration {
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        generation,
        graph,
        metadata,
    })
}

impl LoadedGeneration {
    fn collect_hits(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<(String, f32)>, CoreError> {
        let Some(index) = self.graph.as_ref() else {
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
        index.search(query_vector, limit)
    }

    fn collect_hits_scoped(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        limit: usize,
    ) -> Result<Vec<(String, f32)>, CoreError> {
        let Some(index) = self.graph.as_ref() else {
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
        index.search_scoped(query_vector, allowed_ids, limit)
    }

    fn to_candidates(&self, hits: Vec<(String, f32)>) -> Result<Vec<LexicalCandidate>, CoreError> {
        let mut out: Vec<LexicalCandidate> = Vec::with_capacity(hits.len());
        for (id, score) in hits {
            let meta = self.metadata.get(&id).ok_or_else(|| {
                CoreError::Storage(format!(
                    "semantic: metadata missing candidate `{id}` at generation {}",
                    self.generation.get()
                ))
            })?;
            out.push(LexicalCandidate {
                candidate_id: id,
                repo_id: self.repo_id.clone(),
                revision_id: self.revision_id.clone(),
                manifest_generation: self.generation,
                repo_relative_path: meta.repo_relative_path.clone(),
                start_line: meta.start_line,
                end_line: meta.end_line,
                score,
                snippet: meta.snippet.clone(),
            });
        }
        Ok(out)
    }
}

/// Searcher over a single loaded sealed generation.
pub(crate) struct PersistedSemanticSearcher {
    generation: Arc<LoadedGeneration>,
}

impl PersistedSemanticSearcher {
    pub(crate) fn new(generation: Arc<LoadedGeneration>) -> Self {
        Self { generation }
    }
}

fn top_k_limit(top_k: u32) -> Result<usize, CoreError> {
    usize::try_from(top_k)
        .map_err(|err| CoreError::InvalidContract(format!("semantic: top_k overflow: {err}")))
}

impl SemanticSearcher for PersistedSemanticSearcher {
    fn search(&self, query_vector: &[f32], top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_top_k(top_k)?;
        SemanticPolicy::validate_query_vector(query_vector)?;
        let limit = top_k_limit(top_k)?;
        let hits = self.generation.collect_hits(query_vector, limit)?;
        self.generation.to_candidates(hits)
    }

    fn search_scoped(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        top_k: u32,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_top_k(top_k)?;
        SemanticPolicy::validate_query_vector(query_vector)?;
        let limit = top_k_limit(top_k)?;
        let hits = self
            .generation
            .collect_hits_scoped(query_vector, allowed_ids, limit)?;
        self.generation.to_candidates(hits)
    }
}
