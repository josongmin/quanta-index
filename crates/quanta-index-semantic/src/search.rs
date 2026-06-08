//! Lancedb-backed durable open + search path.
//!
//! `open_generation` opens exactly one sealed generation's lancedb dataset
//! (manifest validated against requested scope, row count cross-checked
//! against the manifest) and returns a `LoadedGeneration` ready to serve
//! `vector_search`. There is no cross-generation replay; the open cost is
//! bounded by reopening a single lancedb dataset.
//!
//! The searcher returns scores as cosine similarity in `[-1, 1]` (lancedb
//! reports cosine *distance* in the `_distance` column; we convert
//! `similarity = 1 - distance` so the historical query-time contract is
//! preserved).

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use arrow_array::{Array, Float32Array, RecordBatch, StringArray, UInt32Array};
use futures::TryStreamExt as _;
use lancedb::DistanceType;
use lancedb::connect;
use lancedb::query::{ExecutableQuery as _, QueryBase as _};
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    LexicalCandidate, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
};
use quanta_index_core::CoreError;
use quanta_index_core::domains::semantic::{SemanticPolicy, SemanticSearcher};

use crate::build::{
    COLUMN_EMBEDDING_ID, COLUMN_END_LINE, COLUMN_REPO_RELATIVE_PATH, COLUMN_SNIPPET,
    COLUMN_START_LINE, TABLE_NAME, dataset_uri,
};
use crate::generation_contract::GenerationContract;
use crate::layout;
use crate::manifest::{FORMAT_VERSION, LEGACY_LANCEDB_FORMAT_VERSION, SemanticManifest};
use crate::sql::build_id_in_filter;

const COLUMN_DISTANCE: &str = "_distance";

fn lancedb_err(action: &str, err: impl core::fmt::Display) -> CoreError {
    CoreError::Storage(format!("semantic: lancedb {action}: {err}"))
}

fn load_generation_contract(generation_dir: &Path) -> Result<GenerationContract, CoreError> {
    let contract_path = layout::build_contract_path(generation_dir);
    let bytes = std::fs::read(&contract_path).map_err(|err| {
        CoreError::Storage(format!(
            "semantic: read generation contract {}: {err}",
            contract_path.display()
        ))
    })?;
    GenerationContract::decode(&bytes)
}

fn load_generation_contract_for_manifest(
    generation_dir: &Path,
    manifest: &SemanticManifest,
) -> Result<Option<GenerationContract>, CoreError> {
    let contract_path = layout::build_contract_path(generation_dir);
    if contract_path.exists() {
        return load_generation_contract(generation_dir).map(Some);
    }
    match manifest.format_version {
        FORMAT_VERSION => Err(CoreError::Storage(format!(
            "semantic: manifest format version {} requires generation contract {}",
            manifest.format_version,
            contract_path.display()
        ))),
        LEGACY_LANCEDB_FORMAT_VERSION => Ok(None),
        other => Err(CoreError::Storage(format!(
            "semantic: manifest format version {other} unsupported during contract load"
        ))),
    }
}

/// One sealed generation loaded against its lancedb dataset.
///
/// Note: lancedb 0.30's `Table` is `Arc<dyn BaseTable>` + an injected
/// `Arc<dyn Database>` (`connection.rs:337`, `table.rs:655-810`); it does
/// **not** borrow from its parent `Connection`. Dropping the connection after
/// `open_table` does not invalidate the table, so we hold only the table here
/// and let the connection drop at end of `open_generation`.
pub(crate) struct LoadedGeneration {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    dimension: usize,
    table: lancedb::Table,
}

/// Open a sealed generation directly from durable state, failing closed on any
/// absent marker, scope mismatch, or shape mismatch.
pub(crate) async fn open_generation(
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
    let manifest_bytes = std::fs::read(&manifest_path).map_err(|err| {
        CoreError::Storage(format!(
            "semantic: read manifest {}: {err}",
            manifest_path.display()
        ))
    })?;
    let manifest = SemanticManifest::decode(&manifest_bytes)?;
    manifest.validate_scope(repo, revision, generation)?;
    if let Some(generation_contract) =
        load_generation_contract_for_manifest(&generation_dir, &manifest)?
    {
        generation_contract.validate_manifest(&manifest)?;
    }

    let dimension = usize::try_from(manifest.dimension).map_err(|err| {
        CoreError::Storage(format!("semantic: manifest dimension overflow: {err}"))
    })?;

    let uri = dataset_uri(&generation_dir)?;
    let connection = connect(&uri)
        .execute()
        .await
        .map_err(|err| lancedb_err(&format!("connect {uri}"), err))?;
    let table = connection
        .open_table(TABLE_NAME)
        .execute()
        .await
        .map_err(|err| lancedb_err(&format!("open_table {TABLE_NAME}"), err))?;

    let live_row_count = table
        .count_rows(None)
        .await
        .map_err(|err| lancedb_err("count_rows", err))?;
    let live_row_count_u64 = u64::try_from(live_row_count)
        .map_err(|err| CoreError::Storage(format!("semantic: row count overflow: {err}")))?;
    if live_row_count_u64 != manifest.row_count {
        return Err(CoreError::Storage(format!(
            "semantic: lancedb row count {live_row_count_u64} != manifest row count {}",
            manifest.row_count
        )));
    }

    drop(connection);
    Ok(LoadedGeneration {
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        generation,
        dimension,
        table,
    })
}

fn column_as_string<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray, CoreError> {
    let column = batch.column_by_name(name).ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: column `{name}` missing from lancedb result batch"
        ))
    })?;
    column.as_any().downcast_ref::<StringArray>().ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: column `{name}` has unexpected Arrow type (expected Utf8) in lancedb result batch"
        ))
    })
}

fn column_as_u32<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt32Array, CoreError> {
    let column = batch.column_by_name(name).ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: column `{name}` missing from lancedb result batch"
        ))
    })?;
    column.as_any().downcast_ref::<UInt32Array>().ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: column `{name}` has unexpected Arrow type (expected UInt32) in lancedb result batch"
        ))
    })
}

fn column_as_f32<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Float32Array, CoreError> {
    let column = batch.column_by_name(name).ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: column `{name}` missing from lancedb result batch"
        ))
    })?;
    column.as_any().downcast_ref::<Float32Array>().ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: column `{name}` has unexpected Arrow type (expected Float32) in lancedb result batch"
        ))
    })
}

fn extract_candidates(
    batch: &RecordBatch,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    generation: ManifestGeneration,
    out: &mut Vec<LexicalCandidate>,
) -> Result<(), CoreError> {
    let id_col = column_as_string(batch, COLUMN_EMBEDDING_ID)?;
    let path_col = column_as_string(batch, COLUMN_REPO_RELATIVE_PATH)?;
    let start_col = column_as_u32(batch, COLUMN_START_LINE)?;
    let end_col = column_as_u32(batch, COLUMN_END_LINE)?;
    let snippet_col = column_as_string(batch, COLUMN_SNIPPET)?;
    let distance_col = column_as_f32(batch, COLUMN_DISTANCE)?;
    for row in 0..batch.num_rows() {
        let id = id_col.value(row).to_owned();
        let path = path_col.value(row).to_owned();
        let snippet = snippet_col.value(row).to_owned();
        let start_line = start_col.value(row);
        let end_line = end_col.value(row);
        let distance = distance_col.value(row);
        // Lancedb returns cosine *distance* in [0, 2]; the historical query
        // contract is cosine *similarity* in [-1, 1] (higher = better).
        let score = 1.0_f32 - distance;
        out.push(LexicalCandidate {
            candidate_id: id,
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            manifest_generation: generation,
            repo_relative_path: RepoRelativePath::new(path),
            start_line,
            end_line,
            score,
            snippet,
            // Semantic results carry no single lexical hit anchor.
            snippet_hit_offset: None,
        });
    }
    Ok(())
}

impl LoadedGeneration {
    fn check_query_dim(&self, query_vector: &[f32]) -> Result<(), CoreError> {
        if query_vector.len() == self.dimension {
            return Ok(());
        }
        Err(CoreError::Typed {
            code: LexicalErrorCode::SemDimMismatch.as_code_str().to_string(),
            message: format!(
                "semantic: query vector dim {} does not match index dim {} for generation {}",
                query_vector.len(),
                self.dimension,
                self.generation.get()
            ),
        })
    }

    async fn run_vector_query(
        &self,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<String>,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        let query_owned: Vec<f32> = query_vector.to_vec();
        let mut vector_query = self
            .table
            .vector_search(query_owned)
            .map_err(|err| lancedb_err("vector_search build", err))?
            .distance_type(DistanceType::Cosine)
            .limit(top_k);
        if let Some(predicate) = filter {
            vector_query = vector_query.only_if(predicate);
        }
        let stream = vector_query
            .execute()
            .await
            .map_err(|err| lancedb_err("vector_search execute", err))?;
        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|err| lancedb_err("vector_search stream", err))?;

        let mut out: Vec<LexicalCandidate> = Vec::with_capacity(top_k);
        for batch in batches {
            extract_candidates(
                &batch,
                &self.repo_id,
                &self.revision_id,
                self.generation,
                &mut out,
            )?;
            if out.len() >= top_k {
                break;
            }
        }
        out.truncate(top_k);
        Ok(out)
    }

    pub(crate) async fn search_async(
        &self,
        query_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        self.check_query_dim(query_vector)?;
        self.run_vector_query(query_vector, top_k, None).await
    }

    pub(crate) async fn search_scoped_async(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        top_k: usize,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        // Validate dim BEFORE the empty-allowlist early return so the typed
        // `SemDimMismatch` contract is symmetric between `search` and
        // `search_scoped` — an empty allowlist + wrong-dim query must still
        // surface the dim-mismatch typed error, not a silent empty result.
        self.check_query_dim(query_vector)?;
        if allowed_ids.is_empty() {
            return Ok(Vec::new());
        }
        let filter = build_id_in_filter(allowed_ids);
        self.run_vector_query(query_vector, top_k, Some(filter))
            .await
    }
}

/// Searcher over a single loaded sealed generation.
///
/// Bridges the sync `SemanticSearcher` port surface to the async lancedb API
/// via the adapter's shared tokio runtime — see [`crate::run_blocking`].
pub(crate) struct PersistedSemanticSearcher {
    loaded: Arc<LoadedGeneration>,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl PersistedSemanticSearcher {
    pub(crate) fn new(
        loaded: Arc<LoadedGeneration>,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        Self { loaded, runtime }
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
        crate::run_blocking(&self.runtime, self.loaded.search_async(query_vector, limit))
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
        crate::run_blocking(
            &self.runtime,
            self.loaded
                .search_scoped_async(query_vector, allowed_ids, limit),
        )
    }
}
