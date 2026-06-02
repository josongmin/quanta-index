//! Lancedb-backed durable generation build path (LDB-00 §3.2 revised).
//!
//! `build_batch` applies a `SemanticIngestBatch` to the generation's lancedb
//! dataset: open or create a connection rooted at
//! `{generation_dir}/dataset/`, open or create the `semantic` table, delete
//! rows by `repo_relative_path` for replace/tombstone scopes, append new
//! embedding rows as an Arrow `RecordBatch`, write the READY marker, and on
//! `seal` write the scope manifest then the SEALED marker (last) so a crash
//! mid-seal stays not-ready. A sealed generation is immutable: a later batch
//! targeting it fails closed.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::{Array, FixedSizeListArray, Float32Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use lancedb::DistanceType;
use lancedb::connect;
use lancedb::index::Index;
use lancedb::index::vector::IvfHnswSqIndexBuilder;
use quanta_index_contract::{EmbeddingDistanceMetric, SemanticIngestBatch, SemanticReplaceScope};
use quanta_index_core::CoreError;
use quanta_index_core::domains::semantic::SemanticPolicy;

use crate::generation_contract::GenerationContract;
use crate::layout;
use crate::manifest::SemanticManifest;

pub(crate) const TABLE_NAME: &str = "semantic";

/// Row-count floor below which we skip ANN index construction at seal.
///
/// Lancedb IVF training requires a meaningful sample, and at this scale a
/// brute-force scan is genuinely faster than the IVF training + per-query
/// partition traversal overhead. 256 is **our** policy choice (not a lancedb-
/// imposed minimum) — matching lancedb's PQ `sample_rate=256` default so that
/// the auto-derived `num_partitions=1` for ~256 rows trains without sampling.
const VECTOR_INDEX_MIN_ROWS: u64 = 256;

/// Sibling-of-`dataset/` staging dir used to make delta-base cloning
/// crash-atomic — see `prepare_generation_dir`.
const STAGING_DIR_NAME: &str = "dataset.staging";
const BACKUP_DIR_NAME: &str = "dataset.backup";

#[cfg(any(test, debug_assertions))]
mod failpoint {
    use std::sync::{Mutex, OnceLock};

    static APPEND_FAIL_PATH: OnceLock<Mutex<Option<String>>> = OnceLock::new();

    fn slot() -> &'static Mutex<Option<String>> {
        APPEND_FAIL_PATH.get_or_init(|| Mutex::new(None))
    }

    fn lock_slot() -> std::sync::MutexGuard<'static, Option<String>> {
        match slot().lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        }
    }

    pub(super) fn set_append_fail_path(path: Option<&str>) {
        let mut guard = lock_slot();
        *guard = path.map(str::to_owned);
    }

    pub(super) fn should_fail_append(path: &str) -> bool {
        lock_slot().as_deref() == Some(path)
    }
}

#[cfg(not(any(test, debug_assertions)))]
mod failpoint {
    pub(super) fn should_fail_append(_path: &str) -> bool {
        false
    }
}

#[cfg(any(test, debug_assertions))]
pub(crate) fn set_append_fail_path_for_debug(path: Option<&str>) {
    failpoint::set_append_fail_path(path);
}

pub(crate) const COLUMN_EMBEDDING_ID: &str = "embedding_id";
pub(crate) const COLUMN_REPO_RELATIVE_PATH: &str = "repo_relative_path";
pub(crate) const COLUMN_START_LINE: &str = "start_line";
pub(crate) const COLUMN_END_LINE: &str = "end_line";
pub(crate) const COLUMN_SNIPPET: &str = "snippet";
pub(crate) const COLUMN_VECTOR: &str = "vector";

fn fs_err(action: &str, path: &Path, err: &std::io::Error) -> CoreError {
    CoreError::Storage(format!("semantic: {action} {}: {err}", path.display()))
}

fn lancedb_err(action: &str, err: impl core::fmt::Display) -> CoreError {
    CoreError::Storage(format!("semantic: lancedb {action}: {err}"))
}

fn arrow_err(action: &str, err: impl core::fmt::Display) -> CoreError {
    CoreError::Storage(format!("semantic: arrow {action}: {err}"))
}

/// Crash-atomic file write for our scope-metadata markers/manifest.
fn write_atomic(path: &Path, bytes: &[u8], action: &str) -> Result<(), CoreError> {
    let staging = path.with_extension("tmp");
    fs::write(&staging, bytes).map_err(|err| fs_err(action, &staging, &err))?;
    fs::rename(&staging, path).map_err(|err| fs_err(action, path, &err))
}

fn copy_dir(src: &Path, dst: &Path) -> Result<(), CoreError> {
    fs::create_dir_all(dst).map_err(|err| fs_err("create copy destination", dst, &err))?;
    for entry in fs::read_dir(src).map_err(|err| fs_err("read source dir", src, &err))? {
        let entry = entry.map_err(|err| fs_err("read source entry", src, &err))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|err| fs_err("file type", &from, &err))?;
        if file_type.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            let _bytes = fs::copy(&from, &to).map_err(|err| fs_err("copy file", &from, &err))?;
        }
    }
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

/// Arrow schema for the lancedb `semantic` table at the given vector dimension.
pub(crate) fn semantic_schema(dimension: i32) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(COLUMN_EMBEDDING_ID, DataType::Utf8, false),
        Field::new(COLUMN_REPO_RELATIVE_PATH, DataType::Utf8, false),
        Field::new(COLUMN_START_LINE, DataType::UInt32, false),
        Field::new(COLUMN_END_LINE, DataType::UInt32, false),
        Field::new(COLUMN_SNIPPET, DataType::Utf8, false),
        Field::new(
            COLUMN_VECTOR,
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimension,
            ),
            false,
        ),
    ]))
}

fn dimension_to_i32(dimension: usize) -> Result<i32, CoreError> {
    i32::try_from(dimension).map_err(|err| {
        CoreError::Storage(format!(
            "semantic: dimension {dimension} does not fit in i32 (Arrow FixedSizeList list size): {err}"
        ))
    })
}

pub(crate) fn dataset_uri(generation_dir: &Path) -> Result<String, CoreError> {
    let dataset_dir = layout::dataset_dir(generation_dir);
    dataset_dir.to_str().map(str::to_owned).ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: dataset path is not valid UTF-8: {}",
            dataset_dir.display()
        ))
    })
}

fn dataset_dir_uri(dataset_dir: &Path) -> Result<String, CoreError> {
    dataset_dir.to_str().map(str::to_owned).ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: dataset path is not valid UTF-8: {}",
            dataset_dir.display()
        ))
    })
}

async fn open_connection(dataset_dir: &Path) -> Result<lancedb::Connection, CoreError> {
    let uri = dataset_dir_uri(dataset_dir)?;
    connect(&uri)
        .execute()
        .await
        .map_err(|err| lancedb_err(&format!("connect {uri}"), err))
}

async fn ensure_table(
    connection: &lancedb::Connection,
    dimension: usize,
) -> Result<lancedb::Table, CoreError> {
    let names = connection
        .table_names()
        .execute()
        .await
        .map_err(|err| lancedb_err("table_names", err))?;
    if names.iter().any(|name| name == TABLE_NAME) {
        let table = connection
            .open_table(TABLE_NAME)
            .execute()
            .await
            .map_err(|err| lancedb_err(&format!("open_table {TABLE_NAME}"), err))?;
        // Cross-batch contract guard: an already-existing unsealed table fixes
        // the FixedSizeList vector dimension at the FIRST batch's contract dim.
        // A follow-up batch on the same gen MUST match it, otherwise a typed
        // `InvalidContract` is the correct surface (not the late opaque arrow
        // schema-mismatch error that `table.add` would otherwise produce).
        verify_table_dimension(&table, dimension).await?;
        return Ok(table);
    }
    let schema = semantic_schema(dimension_to_i32(dimension)?);
    connection
        .create_empty_table(TABLE_NAME, schema)
        .execute()
        .await
        .map_err(|err| lancedb_err(&format!("create_empty_table {TABLE_NAME}"), err))
}

async fn verify_table_dimension(
    table: &lancedb::Table,
    expected_dimension: usize,
) -> Result<(), CoreError> {
    let schema = table
        .schema()
        .await
        .map_err(|err| lancedb_err("read table schema", err))?;
    let field = schema.field_with_name(COLUMN_VECTOR).map_err(|err| {
        CoreError::Storage(format!(
            "semantic: existing table missing `{COLUMN_VECTOR}` column: {err}"
        ))
    })?;
    let DataType::FixedSizeList(_, prior_dim_i32) = field.data_type() else {
        return Err(CoreError::Storage(format!(
            "semantic: existing table `{COLUMN_VECTOR}` column is not FixedSizeList<Float32>: got {:?}",
            field.data_type()
        )));
    };
    let prior_dim = usize::try_from(*prior_dim_i32).map_err(|err| {
        CoreError::Storage(format!(
            "semantic: existing table dimension {prior_dim_i32} does not fit in usize: {err}"
        ))
    })?;
    if prior_dim != expected_dimension {
        return Err(CoreError::InvalidContract(format!(
            "semantic: existing unsealed generation has vector dim {prior_dim} but batch contract dim is {expected_dimension}; cannot mix dimensions within one generation"
        )));
    }
    Ok(())
}

fn validate_replace_scope(scope: &SemanticReplaceScope, dimension: usize) -> Result<(), CoreError> {
    for embedding in &scope.embeddings {
        SemanticPolicy::validate_query_vector(&embedding.vector)?;
        if embedding.vector.len() != dimension {
            return Err(CoreError::InvalidContract(format!(
                "semantic: embedding {} dim {} != contract dim {}",
                embedding.embedding_id.as_str(),
                embedding.vector.len(),
                dimension
            )));
        }
    }
    Ok(())
}

fn build_record_batch(
    scope: &SemanticReplaceScope,
    dimension: usize,
) -> Result<RecordBatch, CoreError> {
    let row_count = scope.embeddings.len();
    let mut ids: Vec<String> = Vec::with_capacity(row_count);
    let mut paths: Vec<String> = Vec::with_capacity(row_count);
    let mut starts: Vec<u32> = Vec::with_capacity(row_count);
    let mut ends: Vec<u32> = Vec::with_capacity(row_count);
    let mut snippets: Vec<String> = Vec::with_capacity(row_count);
    let mut flat_vectors: Vec<f32> = Vec::with_capacity(row_count.saturating_mul(dimension));
    for embedding in &scope.embeddings {
        ids.push(embedding.embedding_id.as_str().to_owned());
        paths.push(embedding.repo_relative_path.as_str().to_owned());
        starts.push(embedding.start_line);
        ends.push(embedding.end_line);
        snippets.push(embedding.snippet.as_ref().to_owned());
        flat_vectors.extend_from_slice(&embedding.vector);
    }

    let id_array = StringArray::from(ids);
    let path_array = StringArray::from(paths);
    let start_array = UInt32Array::from(starts);
    let end_array = UInt32Array::from(ends);
    let snippet_array = StringArray::from(snippets);
    let flat_value_array = Float32Array::from(flat_vectors);
    let dim_i32 = dimension_to_i32(dimension)?;
    let vector_field = Arc::new(Field::new("item", DataType::Float32, true));
    let flat_value_dyn: Arc<dyn Array> = Arc::new(flat_value_array);
    let vector_array = FixedSizeListArray::try_new(vector_field, dim_i32, flat_value_dyn, None)
        .map_err(|err| arrow_err("FixedSizeListArray build", err))?;

    let schema = semantic_schema(dim_i32);
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_array),
            Arc::new(path_array),
            Arc::new(start_array),
            Arc::new(end_array),
            Arc::new(snippet_array),
            Arc::new(vector_array),
        ],
    )
    .map_err(|err| arrow_err("RecordBatch::try_new", err))
}

async fn delete_by_path(table: &lancedb::Table, path: &str) -> Result<(), CoreError> {
    let predicate = format!(
        "{COLUMN_REPO_RELATIVE_PATH} = {}",
        crate::sql::quote_sql_string(path)
    );
    let _result = table
        .delete(predicate.as_str())
        .await
        .map_err(|err| lancedb_err(&format!("delete predicate `{predicate}`"), err))?;
    Ok(())
}

async fn append_scope(
    table: &lancedb::Table,
    scope: &SemanticReplaceScope,
    dimension: usize,
) -> Result<(), CoreError> {
    if scope.embeddings.is_empty() {
        return Ok(());
    }
    if failpoint::should_fail_append(scope.scope.repo_relative_path.as_str()) {
        return Err(CoreError::Storage(format!(
            "semantic: injected append failure for path {}",
            scope.scope.repo_relative_path.as_str()
        )));
    }
    let batch = build_record_batch(scope, dimension)?;
    let _result = table
        .add(batch)
        .execute()
        .await
        .map_err(|err| lancedb_err("table.add", err))?;
    Ok(())
}

fn recover_dataset_artifacts(generation_dir: &Path) -> Result<(), CoreError> {
    let dataset_dir = layout::dataset_dir(generation_dir);
    let staging_dir = generation_dir.join(STAGING_DIR_NAME);
    let backup_dir = generation_dir.join(BACKUP_DIR_NAME);

    if dataset_dir.exists() {
        if staging_dir.exists() {
            fs::remove_dir_all(&staging_dir)
                .map_err(|err| fs_err("clean stale staging dir", &staging_dir, &err))?;
        }
        if backup_dir.exists() {
            fs::remove_dir_all(&backup_dir)
                .map_err(|err| fs_err("clean stale backup dir", &backup_dir, &err))?;
        }
        return Ok(());
    }

    if backup_dir.exists() {
        fs::rename(&backup_dir, &dataset_dir)
            .map_err(|err| fs_err("restore backup dataset", &backup_dir, &err))?;
    }
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir)
            .map_err(|err| fs_err("clean stale staging dir", &staging_dir, &err))?;
    }
    Ok(())
}

fn ensure_generation_contract(
    generation_dir: &Path,
    batch: &SemanticIngestBatch,
) -> Result<GenerationContract, CoreError> {
    GenerationContract::validate_batch_shape(batch)?;
    let contract_path = layout::build_contract_path(generation_dir);
    if contract_path.exists() {
        let bytes = fs::read(&contract_path)
            .map_err(|err| fs_err("read generation contract", &contract_path, &err))?;
        let contract = GenerationContract::decode(&bytes)?;
        contract.validate_batch(batch)?;
        return Ok(contract);
    }
    if layout::dataset_dir(generation_dir).exists() {
        return Err(CoreError::Storage(format!(
            "semantic: dataset exists but generation contract is missing for {}",
            generation_dir.display()
        )));
    }
    fs::create_dir_all(generation_dir)
        .map_err(|err| fs_err("create generation dir", generation_dir, &err))?;
    let contract = GenerationContract::from_batch(batch);
    let bytes = contract.encode()?;
    write_atomic(&contract_path, &bytes, "write generation contract")?;
    Ok(contract)
}

fn prepare_staging_dataset(
    semantic_root: &Path,
    generation_dir: &Path,
    generation_contract: &GenerationContract,
    repo_id: &quanta_index_contract::RepoId,
    revision_id: &quanta_index_contract::RevisionId,
) -> Result<std::path::PathBuf, CoreError> {
    recover_dataset_artifacts(generation_dir)?;
    let dataset_dir = layout::dataset_dir(generation_dir);
    let staging_dir = generation_dir.join(STAGING_DIR_NAME);
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir)
            .map_err(|err| fs_err("clean stale staging dir", &staging_dir, &err))?;
    }
    if dataset_dir.exists() {
        copy_dir(&dataset_dir, &staging_dir)?;
        return Ok(staging_dir);
    }
    if let Some(base_generation) = generation_contract.base_generation {
        let base_dir = layout::generation_dir(semantic_root, repo_id, revision_id, base_generation);
        let base_dataset = layout::dataset_dir(&base_dir);
        if !base_dataset.exists() {
            return Err(CoreError::NotReady(format!(
                "semantic: delta base generation {} is absent for repo={} revision={}",
                base_generation.get(),
                repo_id.as_str(),
                revision_id.as_str()
            )));
        }
        if !layout::sealed_marker_path(&base_dir).exists() {
            return Err(CoreError::NotReady(format!(
                "semantic: delta base generation {} is not sealed for repo={} revision={}; cannot clone an in-progress base",
                base_generation.get(),
                repo_id.as_str(),
                revision_id.as_str()
            )));
        }
        copy_dir(&base_dataset, &staging_dir)?;
        return Ok(staging_dir);
    }
    fs::create_dir_all(&staging_dir)
        .map_err(|err| fs_err("create staging dataset dir", &staging_dir, &err))?;
    Ok(staging_dir)
}

fn promote_staging_dataset(generation_dir: &Path) -> Result<(), CoreError> {
    let dataset_dir = layout::dataset_dir(generation_dir);
    let staging_dir = generation_dir.join(STAGING_DIR_NAME);
    let backup_dir = generation_dir.join(BACKUP_DIR_NAME);
    if backup_dir.exists() {
        fs::remove_dir_all(&backup_dir)
            .map_err(|err| fs_err("clean stale backup dir", &backup_dir, &err))?;
    }
    if dataset_dir.exists() {
        fs::rename(&dataset_dir, &backup_dir)
            .map_err(|err| fs_err("move current dataset to backup", &dataset_dir, &err))?;
    }
    match fs::rename(&staging_dir, &dataset_dir) {
        Ok(()) => {
            if backup_dir.exists() {
                fs::remove_dir_all(&backup_dir)
                    .map_err(|err| fs_err("remove promoted backup dataset", &backup_dir, &err))?;
            }
            Ok(())
        }
        Err(err) => {
            if backup_dir.exists() && !dataset_dir.exists() {
                fs::rename(&backup_dir, &dataset_dir).map_err(|restore_err| {
                    fs_err("restore backup dataset", &backup_dir, &restore_err)
                })?;
            }
            Err(fs_err("promote staging dataset", &staging_dir, &err))
        }
    }
}

async fn build_manifest_bytes(
    table: &lancedb::Table,
    batch: &SemanticIngestBatch,
    generation_contract: &GenerationContract,
) -> Result<Vec<u8>, CoreError> {
    let row_count = table
        .count_rows(None)
        .await
        .map_err(|err| lancedb_err("count_rows", err))?;
    let row_count_u64 = u64::try_from(row_count)
        .map_err(|err| CoreError::Storage(format!("semantic: row count overflow: {err}")))?;

    // SOTA++: build the ANN vector index once at seal so query-time
    // `vector_search` uses IVF_HNSW_SQ (lancedb's HNSW + scalar quantization)
    // instead of a brute-force flat scan. Distance type pinned to Cosine to
    // match the query path. Skipped below `VECTOR_INDEX_MIN_ROWS` because
    // lancedb cannot meaningfully train IVF on too few rows (and brute-force
    // is faster at small scale). The index is built on an immutable sealed
    // dataset so determinism is preserved.
    if row_count_u64 >= VECTOR_INDEX_MIN_ROWS {
        table
            .create_index(
                &[COLUMN_VECTOR],
                Index::IvfHnswSq(
                    IvfHnswSqIndexBuilder::default().distance_type(DistanceType::Cosine),
                ),
            )
            .execute()
            .await
            .map_err(|err| lancedb_err("create_index IvfHnswSq(cosine)", err))?;
    }

    let built_at = built_at_unix_nanos()?;
    let manifest = SemanticManifest::from_generation_contract(
        &batch.repo_id,
        &batch.revision_id,
        batch.generation,
        generation_contract,
        batch.manifest_digest.as_str(),
        row_count_u64,
        built_at,
    );
    manifest.encode()
}

/// Apply a batch to the lancedb-backed durable generation, sealing on `seal`.
pub(crate) async fn build_batch(
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
    if batch.model_contract.distance_metric != EmbeddingDistanceMetric::Cosine {
        return Err(CoreError::InvalidContract(format!(
            "semantic: unsupported distance metric {:?}; this backend serves cosine only",
            batch.model_contract.distance_metric
        )));
    }
    let dimension = usize::try_from(batch.model_contract.dimension).map_err(|err| {
        CoreError::InvalidContract(format!(
            "semantic: model contract dimension overflow: {err}"
        ))
    })?;
    if dimension == 0 {
        return Err(CoreError::InvalidContract(
            "semantic: model contract dimension must be > 0".to_string(),
        ));
    }

    // Validate EVERY replace scope before any destructive operation runs on
    // disk. If we validated lazily inside the apply loop, a later scope's
    // validation failure would leave earlier scopes' deletes already committed
    // to the unsealed lancedb table — `?` propagates the error but the on-disk
    // state is silently mutated. CLAUDE.md "no error swallowing or
    // error-to-default": destructive operations may only run after the entire
    // batch is known to be applicable.
    for scope in &batch.replace_scopes {
        validate_replace_scope(scope, dimension)?;
    }

    recover_dataset_artifacts(&generation_dir)?;
    let generation_contract = ensure_generation_contract(&generation_dir, batch)?;
    let working_dataset = prepare_staging_dataset(
        semantic_root,
        &generation_dir,
        &generation_contract,
        &batch.repo_id,
        &batch.revision_id,
    )?;

    let manifest_bytes = {
        let connection = open_connection(&working_dataset).await?;
        let table = ensure_table(&connection, dimension).await?;

        for scope in &batch.replace_scopes {
            delete_by_path(&table, scope.scope.repo_relative_path.as_str()).await?;
            append_scope(&table, scope, dimension).await?;
        }
        for scope in &batch.tombstone_scopes {
            delete_by_path(&table, scope.scope.repo_relative_path.as_str()).await?;
        }

        if batch.seal {
            Some(build_manifest_bytes(&table, batch, &generation_contract).await?)
        } else {
            None
        }
    };

    promote_staging_dataset(&generation_dir)?;

    write_atomic(
        &layout::ready_marker_path(&generation_dir),
        b"ready",
        "write ready marker",
    )?;
    if let Some(manifest_bytes) = manifest_bytes {
        write_atomic(
            &layout::manifest_path(&generation_dir),
            &manifest_bytes,
            "write manifest",
        )?;
        write_atomic(
            &layout::sealed_marker_path(&generation_dir),
            batch.manifest_digest.as_bytes(),
            "write sealed marker",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use quanta_index_contract::{
        BatchIngestMode, EmbeddingDistanceMetric, EmbeddingId, EmbeddingModelContract,
        EmbeddingNormalization, EmbeddingRecord, ManifestGeneration, OwnerDocKind, RepoId,
        RepoRelativePath, RevisionId, SearchScopeKey, SearchScopeSurface, SemanticIngestBatch,
        SemanticReplaceScope, lex::LanguageCode,
    };
    use quanta_index_core::CoreError;

    use super::{build_batch, failpoint};
    use crate::search::open_generation;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn repo_id() -> RepoId {
        RepoId::new("repo-build")
    }

    fn revision_id() -> RevisionId {
        RevisionId::new("rev-build")
    }

    fn model_contract() -> EmbeddingModelContract {
        EmbeddingModelContract {
            model_id: "test-model".to_string().into_boxed_str(),
            model_version: Some("1".to_string().into_boxed_str()),
            dimension: 3,
            normalization: EmbeddingNormalization::L2Unit,
            distance_metric: EmbeddingDistanceMetric::Cosine,
            policy_digest: "policy:test".to_string().into_boxed_str(),
            view_policy_digest: None,
        }
    }

    fn scope(path: &str) -> SearchScopeKey {
        SearchScopeKey {
            doc_surface: SearchScopeSurface::Chunk,
            repo_relative_path: RepoRelativePath::new(path),
        }
    }

    fn embedding(id: &str, path: &str, vector: Vec<f32>) -> Result<EmbeddingRecord, String> {
        Ok(EmbeddingRecord {
            embedding_id: EmbeddingId::new(id),
            owner_kind: OwnerDocKind::Chunk,
            owner_id: format!("owner-{id}").into_boxed_str(),
            source_doc_id: format!("doc-{id}").into_boxed_str(),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust").map_err(std::string::ToString::to_string)?,
            symbol_kind: None,
            start_byte: 0,
            end_byte: 8,
            start_line: 1,
            end_line: 1,
            snippet: format!("fn {id}() {{}}").into_boxed_str(),
            embedding_input_digest: format!("input:{id}").into_boxed_str(),
            vector_digest: format!("vector:{id}").into_boxed_str(),
            view_kind: "raw_chunk".to_string().into_boxed_str(),
            vector,
        })
    }

    fn batch(
        generation: ManifestGeneration,
        path: &str,
        id: &str,
        vector: Vec<f32>,
        seal: bool,
    ) -> Result<SemanticIngestBatch, String> {
        Ok(SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: format!("manifest:{}", generation.get()),
            batch_digest: format!("batch:{}:{path}", generation.get()),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: scope(path),
                scope_digest: format!("scope:{path}"),
                embeddings: vec![embedding(id, path, vector)?],
            }],
            tombstone_scopes: Vec::new(),
            seal,
        })
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts append-failure rollback via assert macros"
    )]
    fn failed_append_does_not_delete_prior_rows() -> TestResult {
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let generation = ManifestGeneration::new(1);

        crate::run_blocking(
            &runtime,
            build_batch(
                &root,
                &batch(generation, "a.rs", "emb-1", vec![1.0, 0.0, 0.0], false)?,
            ),
        )?;

        failpoint::set_append_fail_path(Some("a.rs"));
        let Err(err) = crate::run_blocking(
            &runtime,
            build_batch(
                &root,
                &batch(generation, "a.rs", "emb-2", vec![0.0, 1.0, 0.0], false)?,
            ),
        ) else {
            return Err("injected append failure must surface".into());
        };
        failpoint::set_append_fail_path(None);
        assert!(matches!(err, CoreError::Storage(_)));

        crate::run_blocking(
            &runtime,
            build_batch(
                &root,
                &SemanticIngestBatch {
                    repo_id: repo_id(),
                    revision_id: revision_id(),
                    generation,
                    base_generation: None,
                    manifest_digest: "manifest:1".to_string(),
                    batch_digest: "batch:1:seal".to_string(),
                    mode: BatchIngestMode::ReplaceGeneration,
                    model_contract: model_contract(),
                    replace_scopes: Vec::new(),
                    tombstone_scopes: Vec::new(),
                    seal: true,
                },
            ),
        )?;

        let loaded = crate::run_blocking(
            &runtime,
            open_generation(&root, &repo_id(), &revision_id(), generation),
        )?;
        let hits = crate::run_blocking(&runtime, loaded.search_async(&[1.0, 0.0, 0.0], 5))?;
        let ids: Vec<String> = hits
            .into_iter()
            .map(|candidate| candidate.candidate_id)
            .collect();
        assert!(
            ids.contains(&"emb-1".to_string()),
            "append failure must preserve prior rows; got {ids:?}"
        );
        Ok(())
    }
}
