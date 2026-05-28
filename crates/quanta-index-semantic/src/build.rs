//! Durable generation build path.
//!
//! `build_batch` materializes `SemanticIngestBatch` mutations into the working
//! rows of a generation directory and, on `seal`, builds the HNSW graph once
//! and writes the manifest + `MARKER_SEALED`. A sealed generation is immutable:
//! a later batch targeting it fails closed rather than patching serve state.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use quanta_index_contract::SemanticIngestBatch;
use quanta_index_core::CoreError;
use quanta_index_core::domains::semantic::SemanticPolicy;

use crate::dataset::{DatasetShard, DatasetWorkingSet, SemanticRow};
use crate::hnsw::HnswIndex;
use crate::manifest::SemanticManifest;
use crate::{codec, graph, layout};

fn fs_err(action: &str, path: &Path, err: &std::io::Error) -> CoreError {
    CoreError::Storage(format!("semantic: {action} {}: {err}", path.display()))
}

/// Apply a batch to the durable generation, sealing it when `batch.seal`.
pub(crate) fn build_batch(
    semantic_root: &Path,
    batch: &SemanticIngestBatch,
) -> Result<(), CoreError> {
    let generation_dir = layout::generation_dir(
        semantic_root,
        &batch.repo_id,
        &batch.revision_id,
        batch.generation,
    );
    if layout::sealed_marker_path(&generation_dir).exists() {
        return Err(CoreError::Storage(format!(
            "semantic: generation {} is already sealed; refusing in-place mutation",
            batch.generation.get()
        )));
    }

    let mut working = load_or_init_working_set(semantic_root, &generation_dir, batch)?;
    let expected_dim = usize::try_from(batch.model_contract.dimension).map_err(|err| {
        CoreError::InvalidContract(format!(
            "semantic: model contract dimension overflow: {err}"
        ))
    })?;

    for scope in &batch.replace_scopes {
        working.remove_path(scope.scope.repo_relative_path.as_str());
        for embedding in &scope.embeddings {
            SemanticPolicy::validate_query_vector(&embedding.vector)?;
            if embedding.vector.len() != expected_dim {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: embedding {} dim {} != contract dim {}",
                    embedding.embedding_id.as_str(),
                    embedding.vector.len(),
                    expected_dim
                )));
            }
            working.set_dimension(batch.model_contract.dimension)?;
            working.upsert(SemanticRow {
                embedding_id: embedding.embedding_id.as_str().to_owned(),
                repo_relative_path: embedding.repo_relative_path.as_str().to_owned(),
                start_line: embedding.start_line,
                end_line: embedding.end_line,
                snippet: embedding.snippet.as_ref().to_owned(),
                vector_le_bytes: codec::f32_slice_to_le_bytes(&embedding.vector),
            });
        }
    }

    for scope in &batch.tombstone_scopes {
        working.remove_path(scope.scope.repo_relative_path.as_str());
    }

    let rows_bytes = persist_working(&generation_dir, &working)?;
    if batch.seal {
        seal_generation(&generation_dir, &working, batch, &rows_bytes)?;
    }
    Ok(())
}

/// Load an in-progress generation's working rows, clone a delta base, or start
/// empty. A delta with a missing base degrades to empty here; readiness/seal
/// validation is what ultimately gates serving.
fn load_or_init_working_set(
    semantic_root: &Path,
    generation_dir: &Path,
    batch: &SemanticIngestBatch,
) -> Result<DatasetWorkingSet, CoreError> {
    let rows_path = layout::rows_path(generation_dir);
    if rows_path.exists() {
        let bytes =
            fs::read(&rows_path).map_err(|err| fs_err("read working dataset", &rows_path, &err))?;
        let shard = DatasetShard::decode(&bytes)?;
        return DatasetWorkingSet::from_shard(shard);
    }
    if let Some(base_generation) = batch.base_generation {
        let base_dir = layout::generation_dir(
            semantic_root,
            &batch.repo_id,
            &batch.revision_id,
            base_generation,
        );
        let base_rows = layout::rows_path(&base_dir);
        if base_rows.exists() {
            let bytes = fs::read(&base_rows)
                .map_err(|err| fs_err("read base dataset", &base_rows, &err))?;
            let shard = DatasetShard::decode(&bytes)?;
            return DatasetWorkingSet::from_shard(shard);
        }
    }
    Ok(DatasetWorkingSet::empty())
}

/// Persist the working rows and the READY (materialized) marker. Returns the
/// encoded rows bytes so seal can checksum them without a re-read.
fn persist_working(
    generation_dir: &Path,
    working: &DatasetWorkingSet,
) -> Result<Vec<u8>, CoreError> {
    let dataset_dir = layout::dataset_dir(generation_dir);
    fs::create_dir_all(&dataset_dir)
        .map_err(|err| fs_err("create dataset directory", &dataset_dir, &err))?;
    let shard = working.to_shard();
    let bytes = shard.encode()?;
    let rows_path = layout::rows_path(generation_dir);
    fs::write(&rows_path, &bytes)
        .map_err(|err| fs_err("write working dataset", &rows_path, &err))?;
    let ready_path = layout::ready_marker_path(generation_dir);
    fs::write(&ready_path, b"ready")
        .map_err(|err| fs_err("write ready marker", &ready_path, &err))?;
    Ok(bytes)
}

/// Finalize a generation: build + persist the graph (when non-empty), write the
/// manifest, then the SEALED marker last so a crash mid-seal stays not-ready.
fn seal_generation(
    generation_dir: &Path,
    working: &DatasetWorkingSet,
    batch: &SemanticIngestBatch,
    rows_bytes: &[u8],
) -> Result<(), CoreError> {
    let dimension = working.dimension();
    let row_count = working.row_count()?;
    let graph_bytes: Vec<u8> = if working.is_empty() {
        Vec::new()
    } else {
        if dimension != batch.model_contract.dimension {
            return Err(CoreError::InvalidContract(format!(
                "semantic: sealed generation dimension {dimension} != contract dimension {}",
                batch.model_contract.dimension
            )));
        }
        let dim = usize::try_from(dimension)
            .map_err(|err| CoreError::Storage(format!("semantic: dimension overflow: {err}")))?;
        let mut index = HnswIndex::new(dim);
        for (id, vector) in working.graph_rows()? {
            index.insert(id, &vector)?;
        }
        let bytes = graph::encode_graph(&index)?;
        let graph_path = layout::graph_path(generation_dir);
        fs::write(&graph_path, &bytes).map_err(|err| fs_err("write graph", &graph_path, &err))?;
        bytes
    };

    let checksum = codec::content_checksum(&[rows_bytes, &graph_bytes]);
    let built_at = built_at_unix_nanos()?;
    let manifest = SemanticManifest::from_build(
        &batch.repo_id,
        &batch.revision_id,
        batch.generation,
        &batch.model_contract,
        batch.manifest_digest.as_str(),
        dimension,
        row_count,
        built_at,
        checksum,
    );
    let manifest_bytes = manifest.encode()?;
    let manifest_path = layout::manifest_path(generation_dir);
    fs::write(&manifest_path, &manifest_bytes)
        .map_err(|err| fs_err("write manifest", &manifest_path, &err))?;
    let sealed_path = layout::sealed_marker_path(generation_dir);
    fs::write(&sealed_path, batch.manifest_digest.as_bytes())
        .map_err(|err| fs_err("write sealed marker", &sealed_path, &err))?;
    Ok(())
}

fn built_at_unix_nanos() -> Result<u64, CoreError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| {
            CoreError::Storage(format!("semantic: system clock before unix epoch: {err}"))
        })?;
    u64::try_from(duration.as_nanos())
        .map_err(|err| CoreError::Storage(format!("semantic: build timestamp overflow: {err}")))
}
