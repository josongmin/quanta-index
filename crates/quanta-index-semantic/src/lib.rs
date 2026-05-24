//! Semantic / vector index driven adapter.
//!
//! Implements `SearchPlaneSemanticIndexBuildPort` and
//! `SearchPlaneVectorIndexStorePort` on top of the Lance columnar format.
//!
//! The adapter writes each generation's vectors into a dedicated dataset
//! directory under `{state_root}/semantic/{manifest_generation}/` and uses a
//! `MARKER_OK` sentinel file to make the build idempotent.
//!
//! Lance is fully async, but the search-plane build/open ports are sync; the
//! adapter is the deliberate sync/async boundary and owns a dedicated current-
//! thread tokio runtime per call. Higher-level callers never see `block_on`.

#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![expect(
    clippy::multiple_crate_versions,
    reason = "lance 6.0.1 transitive (datafusion + arrow stack) brings duplicate \
              hashbrown / itertools / thiserror / object_store / cpufeatures / nom \
              versions inside its own subtree; bounded skip-tree entries are \
              documented in deny.toml"
)]

mod wire;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lance::Dataset;
use lance::dataset::WriteParams;
use lance::deps::arrow_array::{
    ArrayRef, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use lance::deps::arrow_schema::{DataType, Field, Schema};
use quanta_index_contract::{PublishedGenerationSet, PublishedSearchBundleManifest};
use quanta_index_core::CoreError;
use quanta_index_core::domains::materialization::outbound::{
    SearchPlaneSemanticIndexBuildPort, SearchPlaneVectorIndexStorePort, SemanticBuildInput,
};
use tokio::runtime::Builder;

use crate::wire::{EmbeddingPayload, decode_embedding_records};

const MARKER_OK: &str = "MARKER_OK";
const BUILDING_SUFFIX: &str = ".building";
const ENTITY_ID_COLUMN: &str = "entity_id";
const VECTOR_COLUMN: &str = "vector";

/// Lance-backed semantic index adapter.
///
/// All datasets are rooted under `{state_root}/semantic/`.
#[derive(Debug, Clone)]
pub struct LanceSemanticAdapter {
    state_root: PathBuf,
}

impl LanceSemanticAdapter {
    /// Construct an adapter with the given state-root directory.
    pub fn with_state_root(root: impl Into<PathBuf>) -> Self {
        Self {
            state_root: root.into(),
        }
    }

    fn dataset_dir(&self, generation: &PublishedGenerationSet) -> PathBuf {
        self.state_root
            .join("semantic")
            .join(generation.manifest_generation.get().to_string())
    }

    fn dataset_dir_for_manifest(&self, manifest: &PublishedSearchBundleManifest) -> PathBuf {
        self.state_root
            .join("semantic")
            .join(manifest.manifest_generation.get().to_string())
    }

    fn marker_path(target: &Path) -> PathBuf {
        target.join(MARKER_OK)
    }

    fn building_path(target: &Path) -> PathBuf {
        let mut buf = target.as_os_str().to_owned();
        buf.push(BUILDING_SUFFIX);
        PathBuf::from(buf)
    }

    /// Returns `true` when a complete (`MARKER_OK`) dataset directory exists
    /// for the given generation. Returns `false` when the directory is
    /// missing, partial, or `MARKER_OK` has not been written.
    ///
    /// The orchestrator uses this to decide whether the absence of a dataset
    /// is expected (lexical-only generation) or a not-ready failure.
    #[must_use]
    pub fn dataset_present(&self, generation: &PublishedGenerationSet) -> bool {
        let target = self.dataset_dir(generation);
        Self::marker_path(&target).is_file()
    }

    /// Open the Lance dataset for a generation if it exists.
    ///
    /// Returns `Ok(None)` when no semantic directory is present — callers
    /// that know the manifest had `embedding_records = Some(...)` must
    /// interpret this as not-ready; callers that know it was `None` must
    /// interpret it as the expected lexical-only outcome. `Ok(Some(dataset))`
    /// is returned when a completed dataset (with `MARKER_OK`) is available.
    ///
    /// This is a crate-public helper for the T4.1 query plumbing.
    pub fn open_dataset_for_query(
        &self,
        generation: &PublishedGenerationSet,
    ) -> Result<Option<Dataset>, CoreError> {
        let target = self.dataset_dir(generation);
        if !Self::marker_path(&target).is_file() {
            return Ok(None);
        }
        let uri = target_to_uri(&target)?;
        let runtime = build_runtime()?;
        let dataset = block_on_runtime(&runtime, Dataset::open(&uri))
            .map_err(|err| CoreError::Storage(format!("lance open failed: {err}")))?;
        Ok(Some(dataset))
    }

    /// Count the rows in the semantic dataset for the given generation.
    ///
    /// Returns `Ok(None)` when no semantic dataset is present (lexical-only
    /// generation or not-yet-built). Returns `Ok(Some(count))` for a
    /// completed dataset. This is intended primarily for diagnostics and
    /// tests; query paths should use `open_dataset_for_query`.
    pub fn dataset_row_count(
        &self,
        generation: &PublishedGenerationSet,
    ) -> Result<Option<u64>, CoreError> {
        let Some(dataset) = self.open_dataset_for_query(generation)? else {
            return Ok(None);
        };
        let runtime = build_runtime()?;
        let count = block_on_runtime(&runtime, dataset.count_rows(None))
            .map_err(|err| CoreError::Storage(format!("lance count_rows failed: {err}")))?;
        let count_u64 = u64::try_from(count)
            .map_err(|_err| CoreError::Storage("lance row count exceeds u64".to_owned()))?;
        Ok(Some(count_u64))
    }
}

impl SearchPlaneSemanticIndexBuildPort for LanceSemanticAdapter {
    fn build_semantic_index(
        &self,
        manifest: &PublishedSearchBundleManifest,
        input: SemanticBuildInput<'_>,
    ) -> Result<(), CoreError> {
        let Some(embedding_bytes) = input.embedding_records else {
            // Lexical-only generation: nothing to materialize.
            return Ok(());
        };

        let target = self.dataset_dir_for_manifest(manifest);
        let marker = Self::marker_path(&target);
        if marker.is_file() {
            return Ok(());
        }

        let payload = decode_embedding_records(embedding_bytes)?;
        validate_payload_dims(&payload)?;

        let building = Self::building_path(&target);
        clean_dir(&building)?;
        let parent = building.parent().ok_or_else(|| {
            CoreError::Storage(format!(
                "building path {} has no parent directory",
                building.display()
            ))
        })?;
        fs::create_dir_all(parent).map_err(|err| {
            CoreError::Storage(format!(
                "create_dir_all({}) failed: {err}",
                parent.display()
            ))
        })?;

        write_lance_dataset(&building, &payload)?;

        // Atomic rename of the staging dataset into place. If the target
        // already exists (concurrent build raced ahead), the completed
        // sibling wins and we drop our staging copy.
        if target.exists() {
            remove_dir_if_exists(&building)?;
        } else {
            fs::rename(&building, &target).map_err(|err| {
                CoreError::Storage(format!(
                    "rename({} -> {}) failed: {err}",
                    building.display(),
                    target.display()
                ))
            })?;
        }

        fs::write(&marker, b"ok").map_err(|err| {
            CoreError::Storage(format!(
                "write MARKER_OK at {} failed: {err}",
                marker.display()
            ))
        })?;

        Ok(())
    }
}

impl SearchPlaneVectorIndexStorePort for LanceSemanticAdapter {
    fn open_vector_store(&self, generation: &PublishedGenerationSet) -> Result<(), CoreError> {
        // Phase 1 contract: a missing semantic directory is interpreted as
        // "no embeddings for this generation" rather than not-ready. The
        // orchestrator (T3.5) is responsible for cross-checking against the
        // manifest's `embedding_records` field before invoking this port.
        let target = self.dataset_dir(generation);
        if !target.exists() {
            return Ok(());
        }
        if !Self::marker_path(&target).is_file() {
            return Err(CoreError::NotReady(format!(
                "semantic dataset at {} missing MARKER_OK",
                target.display()
            )));
        }
        Ok(())
    }
}

fn validate_payload_dims(payload: &EmbeddingPayload) -> Result<(), CoreError> {
    let expected_dim = usize::try_from(payload.vector_dim).map_err(|_err| {
        CoreError::InvalidContract("embedding_records: vector_dim does not fit in usize".to_owned())
    })?;
    for (idx, record) in payload.records.iter().enumerate() {
        if record.vector.len() != expected_dim {
            return Err(CoreError::InvalidContract(format!(
                "embedding_records: record {idx} dim {} != header dim {expected_dim}",
                record.vector.len()
            )));
        }
    }
    Ok(())
}

fn write_lance_dataset(staging: &Path, payload: &EmbeddingPayload) -> Result<(), CoreError> {
    let dim_i32 = i32::try_from(payload.vector_dim).map_err(|_err| {
        CoreError::InvalidContract(format!(
            "embedding_records: vector_dim {} exceeds i32 (Arrow FixedSizeList limit)",
            payload.vector_dim
        ))
    })?;
    let dim_usize = usize::try_from(payload.vector_dim).map_err(|_err| {
        CoreError::InvalidContract("embedding_records: vector_dim does not fit in usize".to_owned())
    })?;

    let item_field = Arc::new(Field::new("item", DataType::Float32, true));
    let schema = Arc::new(Schema::new(vec![
        Field::new(ENTITY_ID_COLUMN, DataType::Utf8, false),
        Field::new(
            VECTOR_COLUMN,
            DataType::FixedSizeList(Arc::clone(&item_field), dim_i32),
            false,
        ),
    ]));

    let entity_ids: Vec<&str> = payload
        .records
        .iter()
        .map(|r| r.entity_id.as_str())
        .collect();
    let entity_array: ArrayRef = Arc::new(StringArray::from(entity_ids));

    let total_floats = payload
        .records
        .len()
        .checked_mul(dim_usize)
        .ok_or_else(|| {
            CoreError::Storage("lance build: record_count * vector_dim overflowed usize".to_owned())
        })?;
    let mut flat: Vec<f32> = Vec::with_capacity(total_floats);
    for record in &payload.records {
        for lane in &record.vector {
            flat.push(*lane);
        }
    }
    let values = Arc::new(Float32Array::from(flat));
    let vector_array: ArrayRef = Arc::new(
        FixedSizeListArray::try_new(Arc::clone(&item_field), dim_i32, values, None)
            .map_err(|err| CoreError::Storage(format!("lance build: vector array: {err}")))?,
    );

    let batch = RecordBatch::try_new(Arc::clone(&schema), vec![entity_array, vector_array])
        .map_err(|err| CoreError::Storage(format!("lance build: record batch: {err}")))?;

    let uri = target_to_uri(staging)?;
    let runtime = build_runtime()?;
    let batches_iter = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), Arc::clone(&schema));
    let params = WriteParams::default();
    let dataset = block_on_runtime(&runtime, Dataset::write(batches_iter, &uri, Some(params)))
        .map_err(|err| CoreError::Storage(format!("lance write failed: {err}")))?;
    drop(dataset);
    Ok(())
}

fn build_runtime() -> Result<tokio::runtime::Runtime, CoreError> {
    Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| CoreError::Storage(format!("tokio runtime build failed: {err}")))
}

/// Drive an async future to completion using a dedicated tokio runtime.
///
/// Lance's storage API is async; this is the documented sync/async boundary
/// for the semantic adapter. Callers above the adapter never observe
/// `block_on`.
#[expect(
    clippy::disallowed_methods,
    reason = "semantic adapter is the documented sync/async boundary between the sync \
              SearchPlaneSemanticIndexBuildPort and the async lance::Dataset API"
)]
fn block_on_runtime<F: core::future::Future>(
    runtime: &tokio::runtime::Runtime,
    fut: F,
) -> F::Output {
    runtime.block_on(fut)
}

fn target_to_uri(target: &Path) -> Result<String, CoreError> {
    target
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| CoreError::Storage(format!("path {} is not valid utf-8", target.display())))
}

fn clean_dir(path: &Path) -> Result<(), CoreError> {
    remove_dir_if_exists(path)?;
    fs::create_dir_all(path).map_err(|err| {
        CoreError::Storage(format!("create_dir_all({}) failed: {err}", path.display()))
    })
}

fn remove_dir_if_exists(path: &Path) -> Result<(), CoreError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(CoreError::Storage(format!(
            "remove_dir_all({}) failed: {err}",
            path.display()
        ))),
    }
}
