//! Lancedb-backed durable generation build path (LDB-00 §3.2 revised).
//!
//! `build_batch` applies a `SemanticIngestBatch` to the generation's lancedb
//! dataset: open or create a connection rooted at
//! `{generation_dir}/dataset/`, open or create the `semantic` table, delete
//! rows by semantic owner identity (with legacy path tombstones during
//! migration), append new embedding rows as an Arrow `RecordBatch`, write the
//! READY marker, and on
//! `seal` write the scope manifest then the SEALED marker (last) so a crash
//! mid-seal stays not-ready. A sealed generation is immutable: a later batch
//! targeting it fails closed.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::{
    Array, BooleanArray, FixedSizeListArray, Float32Array, RecordBatch, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field};
use futures::TryStreamExt as _;
use lancedb::DistanceType;
use lancedb::connect;
use lancedb::index::Index;
use lancedb::index::vector::IvfHnswSqIndexBuilder;
use lancedb::query::ExecutableQuery as _;
use quanta_index_contract::{
    EmbeddingDistanceMetric, OwnerDocKind, SearchScopeSurface, SemanticCorpusKindV1,
    SemanticIngestBatch, SemanticReplaceScope, SemanticTombstoneScope,
};
use quanta_index_core::CoreError;
use quanta_index_core::domains::semantic::SemanticPolicy;

use crate::errors::{arrow_err, fs_err, lancedb_err};
use crate::generation_contract::GenerationContract;
use crate::layout::{
    self, COLUMN_CARD_SCHEMA_VERSION, COLUMN_CORPUS_KIND, COLUMN_OWNER_ID, COLUMN_OWNER_KIND,
    COLUMN_RENDER_POLICY_DIGEST, COLUMN_REPO_RELATIVE_PATH, COLUMN_VECTOR, TABLE_NAME,
    dimension_to_i32, semantic_schema,
};
use crate::manifest::SemanticManifest;

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

/// Test-only environment variable consumed by the subprocess crash matrix.
///
/// The hook is compiled only into this crate's unit-test binary. It exits
/// without unwinding so the parent test exercises the exact on-disk state a
/// process loss leaves at each promotion boundary.
#[cfg(test)]
const PROMOTION_CRASH_BOUNDARY_ENV: &str = "QUANTA_INDEX_SEMANTIC_PROMOTION_CRASH_BOUNDARY";
#[cfg(test)]
const PROMOTION_CRASH_EXIT_CODE: i32 = 86;
#[cfg(test)]
const PRE_DATASET_PROMOTION: &str = "pre-dataset-promotion";
#[cfg(test)]
const POST_DATASET_PRE_CONTRACT_PROMOTION: &str = "post-dataset-pre-contract-promotion";

#[cfg(test)]
#[expect(
    clippy::exit,
    reason = "the subprocess-only crash matrix must terminate without stack unwinding"
)]
fn exit_for_promotion_crash_boundary(boundary: &str) {
    let configured = std::env::var(PROMOTION_CRASH_BOUNDARY_ENV).ok();
    if configured.as_deref() == Some(boundary) {
        std::process::exit(PROMOTION_CRASH_EXIT_CODE);
    }
}

#[cfg(any(test, debug_assertions))]
mod failpoint {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::sync::{Mutex, OnceLock};

    static CONTRACT_PROMOTION_FAIL_DIR: OnceLock<Mutex<Option<String>>> = OnceLock::new();

    thread_local! {
        static APPEND_FAIL_PATHS: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
    }

    fn contract_promotion_slot() -> &'static Mutex<Option<String>> {
        CONTRACT_PROMOTION_FAIL_DIR.get_or_init(|| Mutex::new(None))
    }

    fn lock_contract_promotion_slot() -> std::sync::MutexGuard<'static, Option<String>> {
        match contract_promotion_slot().lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        }
    }

    pub(super) fn set_append_fail_path(path: Option<&str>) {
        APPEND_FAIL_PATHS.with(|paths| {
            let mut paths = paths.borrow_mut();
            if let Some(path) = path {
                let _inserted = paths.insert(path.to_owned());
            } else {
                paths.clear();
            }
        });
    }

    #[cfg(test)]
    pub(super) fn clear_append_fail_path(path: &str) {
        APPEND_FAIL_PATHS.with(|paths| {
            let _removed = paths.borrow_mut().remove(path);
        });
    }

    pub(super) fn should_fail_append(path: &str) -> bool {
        APPEND_FAIL_PATHS.with(|paths| paths.borrow().contains(path))
    }

    #[cfg(test)]
    pub(super) fn set_contract_promotion_fail_dir(path: Option<&str>) {
        let mut guard = lock_contract_promotion_slot();
        *guard = path.map(str::to_owned);
    }

    pub(super) fn should_fail_contract_promotion(path: &str) -> bool {
        lock_contract_promotion_slot().as_deref() == Some(path)
    }
}

#[cfg(not(any(test, debug_assertions)))]
mod failpoint {
    pub(super) fn should_fail_append(_path: &str) -> bool {
        false
    }

    pub(super) fn should_fail_contract_promotion(_path: &str) -> bool {
        false
    }
}

#[cfg(any(test, debug_assertions))]
pub(crate) fn set_append_fail_path_for_debug(path: Option<&str>) {
    failpoint::set_append_fail_path(path);
}

#[cfg(any(test, debug_assertions))]
const _: fn(Option<&str>) = set_append_fail_path_for_debug;

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

/// The `dataset/`, `dataset.staging`, and `dataset.backup` sibling directories.
///
/// The triple makes dataset replacement crash-atomic: we build into staging and
/// park the previous live dataset in backup during promotion, then recover from
/// whichever combination a crash leaves behind.
struct DatasetPaths {
    dataset: PathBuf,
    staging: PathBuf,
    backup: PathBuf,
}

impl DatasetPaths {
    fn for_generation(generation_dir: &Path) -> Self {
        Self {
            dataset: layout::dataset_dir(generation_dir),
            staging: generation_dir.join(STAGING_DIR_NAME),
            backup: generation_dir.join(BACKUP_DIR_NAME),
        }
    }
}

/// Remove `dir` and its contents if it exists, tagging IO failure with `action`.
fn remove_dir_if_exists(action: &str, dir: &Path) -> Result<(), CoreError> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|err| fs_err(action, dir, &err))?;
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

async fn open_connection(dataset_dir: &Path) -> Result<lancedb::Connection, CoreError> {
    let uri = layout::dataset_dir_to_uri(dataset_dir)?;
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

fn semantic_scope_key_v1(corpus_kind: &str, owner_kind: &str, owner_id: &str) -> String {
    format!("{corpus_kind}\u{1f}{owner_kind}\u{1f}{owner_id}")
}

fn path_scope_key_v1(scope: &quanta_index_contract::SearchScopeKey) -> String {
    format!(
        "{:?}\u{1f}{}",
        scope.doc_surface,
        scope.repo_relative_path.as_str()
    )
}

fn validate_batch_scope_authority_v1(batch: &SemanticIngestBatch) -> Result<(), CoreError> {
    let mut clear_surfaces = BTreeSet::new();
    for surface in &batch.clear_surfaces {
        if !clear_surfaces.insert(*surface) {
            return Err(CoreError::InvalidContract(format!(
                "semantic: duplicate clear surface {surface:?}"
            )));
        }
    }
    if !batch
        .clear_surfaces
        .windows(2)
        .all(|pair| pair[0] < pair[1])
    {
        return Err(CoreError::InvalidContract(
            "semantic: clear surfaces must use canonical ascending order".to_string(),
        ));
    }
    let mut replace_semantic_keys = BTreeSet::new();
    let mut replace_path_keys = BTreeSet::new();
    let mut record_ids = BTreeSet::new();
    for scope in &batch.replace_scopes {
        if clear_surfaces.contains(&scope.scope.doc_surface) {
            return Err(CoreError::InvalidContract(format!(
                "semantic: surface {:?} cannot be cleared and replaced in one batch",
                scope.scope.doc_surface
            )));
        }
        let path_key = path_scope_key_v1(&scope.scope);
        let _path_was_new = replace_path_keys.insert(path_key);
        let mut scope_semantic_keys = BTreeSet::new();
        for embedding in &scope.embeddings {
            let surface = SearchScopeSurface::for_semantic_owner_v1(
                embedding.owner_kind,
                embedding.corpus_kind,
            );
            if clear_surfaces.contains(&surface) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: surface {surface:?} cannot be cleared and replaced in one batch"
                )));
            }
            if !record_ids.insert(embedding.record_id.as_ref()) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: duplicate embedding record_id {:?}",
                    embedding.record_id
                )));
            }
            let semantic_key = semantic_scope_key_v1(
                embedding.corpus_kind.as_code_str(),
                embedding.owner_kind.as_code_str(),
                embedding.owner_id.as_ref(),
            );
            let _scope_key_was_new = scope_semantic_keys.insert(semantic_key);
        }
        for semantic_key in scope_semantic_keys {
            if !replace_semantic_keys.insert(semantic_key.clone()) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: duplicate replace semantic scope {semantic_key:?}"
                )));
            }
        }
    }

    let mut tombstone_semantic_keys = BTreeSet::new();
    let mut tombstone_path_keys = BTreeSet::new();
    for tombstone in &batch.tombstone_scopes {
        if tombstone.scope.is_none() && tombstone.semantic_scope.is_none() {
            return Err(CoreError::InvalidContract(
                "semantic: tombstone requires legacy path scope or semantic owner scope"
                    .to_string(),
            ));
        }
        if let Some(scope) = tombstone.scope.as_ref() {
            if clear_surfaces.contains(&scope.doc_surface) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: surface {:?} cannot be cleared and tombstoned in one batch",
                    scope.doc_surface
                )));
            }
            let key = path_scope_key_v1(scope);
            if replace_path_keys.contains(&key) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: path scope {:?} cannot be replaced and tombstoned in one batch",
                    scope.repo_relative_path.as_str()
                )));
            }
            if !tombstone_path_keys.insert(key) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: duplicate tombstone path scope {:?}",
                    scope.repo_relative_path.as_str()
                )));
            }
        }
        if let Some(scope) = tombstone.semantic_scope.as_ref() {
            let surface =
                SearchScopeSurface::for_semantic_owner_v1(scope.owner_kind, scope.corpus_kind);
            if clear_surfaces.contains(&surface) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: surface {surface:?} cannot be cleared and tombstoned in one batch"
                )));
            }
            let key = semantic_scope_key_v1(
                scope.corpus_kind.as_code_str(),
                scope.owner_kind.as_code_str(),
                scope.owner_id.as_str(),
            );
            if replace_semantic_keys.contains(&key) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: owner scope {:?} cannot be replaced and tombstoned in one batch",
                    scope.owner_id
                )));
            }
            if !tombstone_semantic_keys.insert(key) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: duplicate tombstone owner scope {:?}",
                    scope.owner_id
                )));
            }
        }
    }
    Ok(())
}

fn column_as<'a, T: Array + 'static>(
    batch: &'a RecordBatch,
    name: &str,
    arrow_type: &str,
) -> Result<&'a T, CoreError> {
    let column = batch.column_by_name(name).ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: column `{name}` missing from lancedb result batch"
        ))
    })?;
    column.as_any().downcast_ref::<T>().ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: column `{name}` has unexpected Arrow type (expected {arrow_type}) in lancedb result batch"
        ))
    })
}

struct ManifestCoverageSummary {
    present_corpora: Vec<String>,
    card_schema_versions: Vec<u32>,
    render_policy_digests: Vec<String>,
}

async fn collect_manifest_coverage(
    table: &lancedb::Table,
) -> Result<ManifestCoverageSummary, CoreError> {
    let stream = table
        .query()
        .execute()
        .await
        .map_err(|err| lancedb_err("table.query execute", err))?;
    let batches: Vec<RecordBatch> = stream
        .try_collect()
        .await
        .map_err(|err| lancedb_err("table.query stream", err))?;

    let mut present_corpora = BTreeSet::new();
    let mut card_schema_versions = BTreeSet::new();
    let mut render_policy_digests = BTreeSet::new();
    for batch in batches {
        let corpus_col = column_as::<StringArray>(&batch, COLUMN_CORPUS_KIND, "Utf8")?;
        let card_schema_col =
            column_as::<UInt32Array>(&batch, COLUMN_CARD_SCHEMA_VERSION, "UInt32")?;
        let render_policy_col =
            column_as::<StringArray>(&batch, COLUMN_RENDER_POLICY_DIGEST, "Utf8")?;
        for row in 0..batch.num_rows() {
            let _inserted_corpus = present_corpora.insert(corpus_col.value(row).to_owned());
            let _inserted_schema = card_schema_versions.insert(card_schema_col.value(row));
            let _inserted_render =
                render_policy_digests.insert(render_policy_col.value(row).to_owned());
        }
    }

    Ok(ManifestCoverageSummary {
        present_corpora: present_corpora.into_iter().collect(),
        card_schema_versions: card_schema_versions.into_iter().collect(),
        render_policy_digests: render_policy_digests.into_iter().collect(),
    })
}

fn build_record_batch(
    scope: &SemanticReplaceScope,
    dimension: usize,
) -> Result<RecordBatch, CoreError> {
    let row_count = scope.embeddings.len();
    let mut ids: Vec<String> = Vec::with_capacity(row_count);
    let mut record_ids: Vec<String> = Vec::with_capacity(row_count);
    let mut paths: Vec<String> = Vec::with_capacity(row_count);
    let mut owner_ids: Vec<String> = Vec::with_capacity(row_count);
    let mut owner_kinds: Vec<String> = Vec::with_capacity(row_count);
    let mut corpus_kinds: Vec<String> = Vec::with_capacity(row_count);
    let mut parent_owner_ids: Vec<Option<String>> = Vec::with_capacity(row_count);
    let mut source_doc_ids: Vec<String> = Vec::with_capacity(row_count);
    let mut languages: Vec<String> = Vec::with_capacity(row_count);
    let mut packages: Vec<Option<String>> = Vec::with_capacity(row_count);
    let mut symbol_kinds: Vec<Option<String>> = Vec::with_capacity(row_count);
    let mut visibilities: Vec<Option<String>> = Vec::with_capacity(row_count);
    let mut source_roles: Vec<String> = Vec::with_capacity(row_count);
    let mut generateds: Vec<bool> = Vec::with_capacity(row_count);
    let mut capability_statuses: Vec<String> = Vec::with_capacity(row_count);
    let mut authority_digests: Vec<String> = Vec::with_capacity(row_count);
    let mut render_policy_digests: Vec<String> = Vec::with_capacity(row_count);
    let mut card_schema_versions: Vec<u32> = Vec::with_capacity(row_count);
    let mut embedding_input_digests: Vec<String> = Vec::with_capacity(row_count);
    let mut vector_digests: Vec<String> = Vec::with_capacity(row_count);
    let mut starts: Vec<u32> = Vec::with_capacity(row_count);
    let mut ends: Vec<u32> = Vec::with_capacity(row_count);
    let mut snippets: Vec<String> = Vec::with_capacity(row_count);
    let mut flat_vectors: Vec<f32> = Vec::with_capacity(row_count.saturating_mul(dimension));
    for embedding in &scope.embeddings {
        ids.push(embedding.embedding_id.as_str().to_owned());
        record_ids.push(embedding.record_id.as_ref().to_owned());
        paths.push(embedding.repo_relative_path.as_str().to_owned());
        owner_ids.push(embedding.owner_id.as_ref().to_owned());
        owner_kinds.push(embedding.owner_kind.as_code_str().to_owned());
        corpus_kinds.push(embedding.corpus_kind.as_code_str().to_owned());
        parent_owner_ids.push(embedding.parent_owner_id.as_deref().map(str::to_owned));
        source_doc_ids.push(embedding.source_doc_id.as_ref().to_owned());
        languages.push(embedding.language.as_str().to_owned());
        packages.push(embedding.package.as_deref().map(str::to_owned));
        symbol_kinds.push(
            embedding
                .symbol_kind
                .as_ref()
                .map(|symbol_kind| symbol_kind.as_str().to_owned()),
        );
        visibilities.push(embedding.visibility.as_deref().map(str::to_owned));
        source_roles.push(embedding.source_role.as_code_str().to_owned());
        generateds.push(embedding.generated);
        capability_statuses.push(embedding.capability_status.as_code_str().to_owned());
        authority_digests.push(embedding.authority_digest.as_ref().to_owned());
        render_policy_digests.push(embedding.render_policy_digest.as_ref().to_owned());
        card_schema_versions.push(embedding.card_schema_version);
        embedding_input_digests.push(embedding.embedding_input_digest.as_ref().to_owned());
        vector_digests.push(embedding.vector_digest.as_ref().to_owned());
        starts.push(embedding.start_line);
        ends.push(embedding.end_line);
        snippets.push(embedding.snippet.as_ref().to_owned());
        flat_vectors.extend_from_slice(&embedding.vector);
    }

    let id_array = StringArray::from(ids);
    let record_id_array = StringArray::from(record_ids);
    let path_array = StringArray::from(paths);
    let owner_id_array = StringArray::from(owner_ids);
    let owner_kind_array = StringArray::from(owner_kinds);
    let corpus_kind_array = StringArray::from(corpus_kinds);
    let parent_owner_id_array = StringArray::from(parent_owner_ids);
    let source_doc_id_array = StringArray::from(source_doc_ids);
    let language_array = StringArray::from(languages);
    let package_array = StringArray::from(packages);
    let symbol_kind_array = StringArray::from(symbol_kinds);
    let visibility_array = StringArray::from(visibilities);
    let source_role_array = StringArray::from(source_roles);
    let generated_array = BooleanArray::from(generateds);
    let capability_status_array = StringArray::from(capability_statuses);
    let authority_digest_array = StringArray::from(authority_digests);
    let render_policy_digest_array = StringArray::from(render_policy_digests);
    let card_schema_version_array = UInt32Array::from(card_schema_versions);
    let embedding_input_digest_array = StringArray::from(embedding_input_digests);
    let vector_digest_array = StringArray::from(vector_digests);
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
            Arc::new(record_id_array),
            Arc::new(path_array),
            Arc::new(owner_id_array),
            Arc::new(owner_kind_array),
            Arc::new(corpus_kind_array),
            Arc::new(parent_owner_id_array),
            Arc::new(source_doc_id_array),
            Arc::new(language_array),
            Arc::new(package_array),
            Arc::new(symbol_kind_array),
            Arc::new(visibility_array),
            Arc::new(source_role_array),
            Arc::new(generated_array),
            Arc::new(capability_status_array),
            Arc::new(authority_digest_array),
            Arc::new(render_policy_digest_array),
            Arc::new(card_schema_version_array),
            Arc::new(embedding_input_digest_array),
            Arc::new(vector_digest_array),
            Arc::new(start_array),
            Arc::new(end_array),
            Arc::new(snippet_array),
            Arc::new(vector_array),
        ],
    )
    .map_err(|err| arrow_err("RecordBatch::try_new", err))
}

fn semantic_scope_tuple_from_embedding(
    embedding: &quanta_index_contract::EmbeddingRecord,
) -> (String, String, String) {
    (
        embedding.corpus_kind.as_code_str().to_owned(),
        embedding.owner_kind.as_code_str().to_owned(),
        embedding.owner_id.as_ref().to_owned(),
    )
}

fn semantic_scopes_for_replace_scope(
    scope: &SemanticReplaceScope,
) -> BTreeSet<(String, String, String)> {
    scope
        .embeddings
        .iter()
        .map(semantic_scope_tuple_from_embedding)
        .collect()
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

async fn delete_by_semantic_scope(
    table: &lancedb::Table,
    corpus_kind: &str,
    owner_kind: &str,
    owner_id: &str,
) -> Result<(), CoreError> {
    let predicate = format!(
        "{COLUMN_CORPUS_KIND} = {} AND {COLUMN_OWNER_KIND} = {} AND {COLUMN_OWNER_ID} = {}",
        crate::sql::quote_sql_string(corpus_kind),
        crate::sql::quote_sql_string(owner_kind),
        crate::sql::quote_sql_string(owner_id),
    );
    let _result = table
        .delete(predicate.as_str())
        .await
        .map_err(|err| lancedb_err(&format!("delete predicate `{predicate}`"), err))?;
    Ok(())
}

fn semantic_surface_delete_predicate_v1(surface: SearchScopeSurface) -> Result<String, CoreError> {
    let mut owner_clauses = Vec::new();
    for owner_kind in OwnerDocKind::ALL {
        let matching_corpora: Vec<&'static str> = SemanticCorpusKindV1::ALL
            .iter()
            .copied()
            .filter(|corpus_kind| {
                SearchScopeSurface::for_semantic_owner_v1(*owner_kind, *corpus_kind) == surface
            })
            .map(SemanticCorpusKindV1::as_code_str)
            .collect();
        if matching_corpora.is_empty() {
            continue;
        }
        let owner_predicate = format!(
            "{COLUMN_OWNER_KIND} = {}",
            crate::sql::quote_sql_string(owner_kind.as_code_str())
        );
        if matching_corpora.len() == SemanticCorpusKindV1::ALL.len() {
            owner_clauses.push(owner_predicate);
            continue;
        }
        let corpus_predicate = matching_corpora
            .into_iter()
            .map(crate::sql::quote_sql_string)
            .map(|corpus_kind| format!("{COLUMN_CORPUS_KIND} = {corpus_kind}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        owner_clauses.push(format!("({owner_predicate} AND ({corpus_predicate}))"));
    }
    if owner_clauses.is_empty() {
        return Err(CoreError::InvalidContract(format!(
            "semantic: no owner/corpus mapping exists for clear surface {surface:?}"
        )));
    }
    Ok(owner_clauses.join(" OR "))
}

async fn delete_surface_rows(
    table: &lancedb::Table,
    surface: SearchScopeSurface,
) -> Result<(), CoreError> {
    let predicate = semantic_surface_delete_predicate_v1(surface)?;
    let _result = table.delete(predicate.as_str()).await.map_err(|err| {
        lancedb_err(
            &format!("delete semantic surface {surface:?} predicate `{predicate}`"),
            err,
        )
    })?;
    Ok(())
}

async fn delete_replace_scope_rows(
    table: &lancedb::Table,
    scope: &SemanticReplaceScope,
) -> Result<(), CoreError> {
    for (corpus_kind, owner_kind, owner_id) in semantic_scopes_for_replace_scope(scope) {
        delete_by_semantic_scope(table, &corpus_kind, &owner_kind, &owner_id).await?;
    }
    Ok(())
}

async fn delete_tombstone_scope_rows(
    table: &lancedb::Table,
    scope: &SemanticTombstoneScope,
) -> Result<(), CoreError> {
    if let Some(semantic_scope) = scope.semantic_scope.as_ref() {
        return delete_by_semantic_scope(
            table,
            semantic_scope.corpus_kind.as_code_str(),
            semantic_scope.owner_kind.as_code_str(),
            semantic_scope.owner_id.as_str(),
        )
        .await;
    }
    // LEGACY-MIGRATION-ONLY: path tombstones remain openable while producers cut over
    // to semantic owner/corpus-scoped deletes.
    let legacy_scope = scope.scope.as_ref().ok_or_else(|| {
        CoreError::InvalidContract(
            "semantic: tombstone requires legacy path scope or semantic owner scope".to_string(),
        )
    })?;
    delete_by_path(table, legacy_scope.repo_relative_path.as_str()).await
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
    let paths = DatasetPaths::for_generation(generation_dir);
    let contract_path = layout::build_contract_path(generation_dir);
    let staged_contract_path = contract_path.with_extension("next");

    if paths.dataset.exists() {
        if staged_contract_path.exists() {
            if paths.backup.exists() || !contract_path.exists() {
                fs::rename(&staged_contract_path, &contract_path).map_err(|err| {
                    fs_err(
                        "recover promoted generation contract",
                        &staged_contract_path,
                        &err,
                    )
                })?;
            } else {
                fs::remove_file(&staged_contract_path).map_err(|err| {
                    fs_err(
                        "abort pre-promotion generation contract",
                        &staged_contract_path,
                        &err,
                    )
                })?;
            }
        }
        remove_dir_if_exists("clean stale staging dir", &paths.staging)?;
        remove_dir_if_exists("clean stale backup dir", &paths.backup)?;
        return Ok(());
    }

    if staged_contract_path.exists() {
        fs::remove_file(&staged_contract_path).map_err(|err| {
            fs_err(
                "remove aborted generation contract",
                &staged_contract_path,
                &err,
            )
        })?;
    }
    if paths.backup.exists() {
        fs::rename(&paths.backup, &paths.dataset)
            .map_err(|err| fs_err("restore backup dataset", &paths.backup, &err))?;
    }
    remove_dir_if_exists("clean stale staging dir", &paths.staging)?;
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
        return contract.merge_batch(batch);
    }
    if layout::dataset_dir(generation_dir).exists() {
        return Err(CoreError::Storage(format!(
            "semantic: dataset exists but generation contract is missing for {}",
            generation_dir.display()
        )));
    }
    fs::create_dir_all(generation_dir)
        .map_err(|err| fs_err("create generation dir", generation_dir, &err))?;
    Ok(GenerationContract::from_batch(batch))
}

#[cfg(test)]
fn persist_generation_contract(
    generation_dir: &Path,
    contract: &GenerationContract,
) -> Result<(), CoreError> {
    let bytes = contract.encode()?;
    write_atomic(
        &layout::build_contract_path(generation_dir),
        &bytes,
        "write generation contract",
    )
}

fn stage_generation_contract(
    generation_dir: &Path,
    contract: &GenerationContract,
) -> Result<PathBuf, CoreError> {
    let staged_path = layout::build_contract_path(generation_dir).with_extension("next");
    let bytes = contract.encode()?;
    write_atomic(
        &staged_path,
        &bytes,
        "stage generation contract for dataset promotion",
    )?;
    Ok(staged_path)
}

fn promote_staged_generation_contract(
    generation_dir: &Path,
    staged_path: &Path,
) -> Result<(), CoreError> {
    if failpoint::should_fail_contract_promotion(generation_dir.to_string_lossy().as_ref()) {
        return Err(CoreError::Storage(
            "semantic: injected generation contract promotion failure".to_string(),
        ));
    }
    let contract_path = layout::build_contract_path(generation_dir);
    fs::rename(staged_path, &contract_path)
        .map_err(|err| fs_err("promote generation contract", staged_path, &err))
}

fn prepare_staging_dataset(
    semantic_root: &Path,
    generation_dir: &Path,
    generation_contract: &GenerationContract,
    repo_id: &quanta_index_contract::RepoId,
    revision_id: &quanta_index_contract::RevisionId,
) -> Result<std::path::PathBuf, CoreError> {
    recover_dataset_artifacts(generation_dir)?;
    let paths = DatasetPaths::for_generation(generation_dir);
    remove_dir_if_exists("clean stale staging dir", &paths.staging)?;
    if paths.dataset.exists() {
        copy_dir(&paths.dataset, &paths.staging)?;
        return Ok(paths.staging);
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
        copy_dir(&base_dataset, &paths.staging)?;
        return Ok(paths.staging);
    }
    fs::create_dir_all(&paths.staging)
        .map_err(|err| fs_err("create staging dataset dir", &paths.staging, &err))?;
    Ok(paths.staging)
}

fn promote_staging_dataset(generation_dir: &Path) -> Result<(), CoreError> {
    let paths = DatasetPaths::for_generation(generation_dir);
    remove_dir_if_exists("clean stale backup dir", &paths.backup)?;
    if paths.dataset.exists() {
        fs::rename(&paths.dataset, &paths.backup)
            .map_err(|err| fs_err("move current dataset to backup", &paths.dataset, &err))?;
    }
    match fs::rename(&paths.staging, &paths.dataset) {
        Ok(()) => Ok(()),
        Err(err) => {
            if paths.backup.exists() && !paths.dataset.exists() {
                fs::rename(&paths.backup, &paths.dataset).map_err(|restore_err| {
                    fs_err("restore backup dataset", &paths.backup, &restore_err)
                })?;
            }
            Err(fs_err("promote staging dataset", &paths.staging, &err))
        }
    }
}

fn finalize_promoted_dataset(generation_dir: &Path) -> Result<(), CoreError> {
    let paths = DatasetPaths::for_generation(generation_dir);
    remove_dir_if_exists("remove promoted backup dataset", &paths.backup)
}

fn rollback_promoted_dataset(generation_dir: &Path) -> Result<(), CoreError> {
    let paths = DatasetPaths::for_generation(generation_dir);
    remove_dir_if_exists("remove uncommitted promoted dataset", &paths.dataset)?;
    if paths.backup.exists() {
        fs::rename(&paths.backup, &paths.dataset).map_err(|err| {
            fs_err(
                "restore dataset after contract failure",
                &paths.backup,
                &err,
            )
        })?;
    }
    Ok(())
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
    let coverage = collect_manifest_coverage(table).await?;

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
        coverage.present_corpora,
        generation_contract.required_corpora.clone(),
        coverage.card_schema_versions,
        coverage.render_policy_digests,
        generation_contract.corpus_policy_digest.clone(),
    );
    manifest.validate_corpus_coverage()?;
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
    validate_batch_scope_authority_v1(batch)?;

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

        for surface in &batch.clear_surfaces {
            delete_surface_rows(&table, *surface).await?;
        }
        for scope in &batch.replace_scopes {
            delete_replace_scope_rows(&table, scope).await?;
            append_scope(&table, scope, dimension).await?;
        }
        for scope in &batch.tombstone_scopes {
            delete_tombstone_scope_rows(&table, scope).await?;
        }

        if batch.seal {
            Some(build_manifest_bytes(&table, batch, &generation_contract).await?)
        } else {
            None
        }
    };

    // Stage the generation-wide policy only after every table mutation and
    // manifest validation has succeeded. The final rename follows dataset
    // promotion; recovery promotes this staged sidecar only when the dataset is
    // already durable, so a rejected append/tombstone/seal cannot advance policy
    // independently of its rows.
    let staged_contract_path = stage_generation_contract(&generation_dir, &generation_contract)?;
    #[cfg(test)]
    exit_for_promotion_crash_boundary(PRE_DATASET_PROMOTION);
    if let Err(promote_err) = promote_staging_dataset(&generation_dir) {
        if let Err(cleanup_err) = fs::remove_file(&staged_contract_path) {
            return Err(CoreError::Storage(format!(
                "semantic: dataset promotion failed ({promote_err}); cleanup staged generation contract {} also failed: {cleanup_err}",
                staged_contract_path.display()
            )));
        }
        return Err(promote_err);
    }
    #[cfg(test)]
    exit_for_promotion_crash_boundary(POST_DATASET_PRE_CONTRACT_PROMOTION);
    if let Err(contract_err) =
        promote_staged_generation_contract(&generation_dir, &staged_contract_path)
    {
        let rollback_result = rollback_promoted_dataset(&generation_dir);
        let cleanup_result = fs::remove_file(&staged_contract_path);
        if let Err(rollback_err) = rollback_result {
            return Err(CoreError::Storage(format!(
                "semantic: generation contract promotion failed ({contract_err}); dataset rollback also failed: {rollback_err}"
            )));
        }
        if let Err(cleanup_err) = cleanup_result {
            return Err(CoreError::Storage(format!(
                "semantic: generation contract promotion failed ({contract_err}); staged contract cleanup {} also failed: {cleanup_err}",
                staged_contract_path.display()
            )));
        }
        return Err(contract_err);
    }
    finalize_promoted_dataset(&generation_dir)?;

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
    use std::env;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use arrow_array::Array;
    use futures::TryStreamExt as _;
    use lancedb::query::ExecutableQuery as _;
    use tempfile::tempdir;

    use quanta_index_contract::{
        BatchIngestMode, CapabilityStatusV1, EmbeddingDistanceMetric, EmbeddingId,
        EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord, ManifestGeneration,
        OwnerDocKind, RepoId, RepoRelativePath, RevisionId, SearchScopeKey, SearchScopeSurface,
        SemanticCorpusKindV1, SemanticIngestBatch, SemanticReplaceScope, SemanticSourceScopeKeyV1,
        SemanticTombstoneScope, SourceRoleV1, lex::LanguageCode,
    };
    use quanta_index_core::CoreError;

    use super::{
        BACKUP_DIR_NAME, POST_DATASET_PRE_CONTRACT_PROMOTION, PRE_DATASET_PROMOTION,
        PROMOTION_CRASH_BOUNDARY_ENV, PROMOTION_CRASH_EXIT_CODE, STAGING_DIR_NAME, build_batch,
        column_as, ensure_generation_contract, failpoint, open_connection,
        persist_generation_contract, recover_dataset_artifacts, stage_generation_contract,
        validate_batch_scope_authority_v1,
    };
    use crate::generation_contract::GenerationContract;
    use crate::layout::{
        self, COLUMN_AUTHORITY_DIGEST, COLUMN_CAPABILITY_STATUS, COLUMN_CARD_SCHEMA_VERSION,
        COLUMN_CORPUS_KIND, COLUMN_GENERATED, COLUMN_OWNER_ID, COLUMN_PACKAGE,
        COLUMN_PARENT_OWNER_ID, COLUMN_RECORD_ID, COLUMN_RENDER_POLICY_DIGEST,
        COLUMN_SOURCE_DOC_ID, COLUMN_SOURCE_ROLE, COLUMN_VISIBILITY, TABLE_NAME,
    };
    use crate::manifest::SemanticManifest;
    use crate::search::open_generation;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const PROMOTION_CRASH_ROOT_ENV: &str = "QUANTA_INDEX_SEMANTIC_PROMOTION_CRASH_ROOT";

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

    fn semantic_scope(
        corpus_kind: SemanticCorpusKindV1,
        owner_kind: OwnerDocKind,
        owner_id: &str,
    ) -> SemanticSourceScopeKeyV1 {
        SemanticSourceScopeKeyV1 {
            corpus_kind,
            owner_kind,
            owner_id: owner_id.to_string(),
        }
    }

    fn embedding(
        id: &str,
        path: &str,
        owner_kind: OwnerDocKind,
        owner_id: &str,
        corpus_kind: SemanticCorpusKindV1,
        vector: Vec<f32>,
    ) -> Result<EmbeddingRecord, String> {
        let source_role = match corpus_kind {
            SemanticCorpusKindV1::RawCodeFallback => SourceRoleV1::RawFallbackText,
            SemanticCorpusKindV1::DocumentSummary => SourceRoleV1::SummaryText,
            SemanticCorpusKindV1::DocumentLeaf | SemanticCorpusKindV1::DocumentSection => {
                SourceRoleV1::DocumentText
            }
            SemanticCorpusKindV1::SymbolCard
            | SemanticCorpusKindV1::ModuleCard
            | SemanticCorpusKindV1::ClusterCard
            | SemanticCorpusKindV1::TestBehavior
            | SemanticCorpusKindV1::RepositorySummary => SourceRoleV1::CardText,
        };
        let capability_status = if corpus_kind == SemanticCorpusKindV1::RawCodeFallback {
            CapabilityStatusV1::Degraded
        } else {
            CapabilityStatusV1::Full
        };
        Ok(EmbeddingRecord {
            embedding_id: EmbeddingId::new(id),
            record_id: format!("record-{id}").into_boxed_str(),
            owner_kind,
            owner_id: owner_id.to_string().into_boxed_str(),
            corpus_kind,
            parent_owner_id: (corpus_kind == SemanticCorpusKindV1::RawCodeFallback)
                .then(|| owner_id.to_string().into_boxed_str()),
            source_doc_id: format!("doc-{id}").into_boxed_str(),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust").map_err(std::string::ToString::to_string)?,
            package: Some("crate".to_string().into_boxed_str()),
            symbol_kind: None,
            visibility: Some("pub".to_string().into_boxed_str()),
            source_role,
            generated: false,
            capability_status,
            authority_digest: format!("auth:{id}").into_boxed_str(),
            render_policy_digest: format!("render:{id}").into_boxed_str(),
            card_schema_version: if corpus_kind == SemanticCorpusKindV1::RawCodeFallback {
                0
            } else {
                1
            },
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
            required_corpora: Vec::new(),
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: scope(path),
                scope_digest: format!("scope:{path}"),
                embeddings: vec![embedding(
                    id,
                    path,
                    OwnerDocKind::Chunk,
                    &format!("owner-{id}"),
                    SemanticCorpusKindV1::RawCodeFallback,
                    vector,
                )?],
            }],
            tombstone_scopes: Vec::new(),
            seal,
        })
    }

    fn promotion_batch(
        generation: ManifestGeneration,
        id: &str,
        owner_kind: OwnerDocKind,
        owner_id: &str,
        corpus_kind: SemanticCorpusKindV1,
        required_corpora: Vec<SemanticCorpusKindV1>,
        vector: Vec<f32>,
    ) -> Result<SemanticIngestBatch, String> {
        let mut batch = batch(generation, "src/promotion.rs", id, vector, false)?;
        let record = &mut batch.replace_scopes[0].embeddings[0];
        record.owner_kind = owner_kind;
        record.owner_id = owner_id.to_string().into_boxed_str();
        record.corpus_kind = corpus_kind;
        record.source_role = match corpus_kind {
            SemanticCorpusKindV1::RawCodeFallback => SourceRoleV1::RawFallbackText,
            SemanticCorpusKindV1::DocumentSummary => SourceRoleV1::SummaryText,
            SemanticCorpusKindV1::DocumentLeaf | SemanticCorpusKindV1::DocumentSection => {
                SourceRoleV1::DocumentText
            }
            SemanticCorpusKindV1::SymbolCard
            | SemanticCorpusKindV1::ModuleCard
            | SemanticCorpusKindV1::ClusterCard
            | SemanticCorpusKindV1::TestBehavior
            | SemanticCorpusKindV1::RepositorySummary => SourceRoleV1::CardText,
        };
        record.capability_status = if corpus_kind == SemanticCorpusKindV1::RawCodeFallback {
            CapabilityStatusV1::Degraded
        } else {
            CapabilityStatusV1::Full
        };
        record.parent_owner_id = None;
        record.card_schema_version = if corpus_kind == SemanticCorpusKindV1::RawCodeFallback {
            0
        } else {
            1
        };
        batch.required_corpora = required_corpora;
        batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
        Ok(batch)
    }

    fn seal_existing_generation_batch(mut batch: SemanticIngestBatch) -> SemanticIngestBatch {
        batch.batch_digest = format!("{}:seal", batch.batch_digest);
        batch.required_corpora.clear();
        batch.replace_scopes.clear();
        batch.tombstone_scopes.clear();
        batch.seal = true;
        batch
    }

    fn run_promotion_crash_child(root: &Path, boundary: &str) -> TestResult {
        if boundary != PRE_DATASET_PROMOTION && boundary != POST_DATASET_PRE_CONTRACT_PROMOTION {
            return Err(format!("unknown promotion crash boundary {boundary}").into());
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let generation = ManifestGeneration::new(90);
        let module_batch = promotion_batch(
            generation,
            "module-after-crash",
            OwnerDocKind::Module,
            "module:after-crash",
            SemanticCorpusKindV1::ModuleCard,
            vec![SemanticCorpusKindV1::ModuleCard],
            vec![0.0, 1.0, 0.0],
        )?;
        let result = crate::run_blocking(&runtime, build_batch(root, &module_batch));
        match result {
            Ok(()) => Err("promotion child returned without abrupt exit".into()),
            Err(err) => {
                Err(format!("promotion child returned error instead of abrupt exit: {err}").into())
            }
        }
    }

    fn child_root_from_env() -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
        match env::var_os(PROMOTION_CRASH_ROOT_ENV) {
            Some(root) => Ok(Some(PathBuf::from(root))),
            None => Ok(None),
        }
    }

    fn assert_recovered_promotion_state(
        root: &Path,
        boundary: &str,
        base_batch: SemanticIngestBatch,
    ) -> TestResult {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let generation = base_batch.generation;
        let generation_dir = layout::generation_dir(root, &repo_id(), &revision_id(), generation);
        let contract_path = layout::build_contract_path(&generation_dir);
        let staged_contract_path = contract_path.with_extension("next");

        let before_recovery = GenerationContract::decode(&std::fs::read(&contract_path)?)?;
        assert_eq!(before_recovery.required_corpora, vec!["SymbolCard"]);
        assert!(
            staged_contract_path.exists(),
            "crash must leave staged contract"
        );
        match boundary {
            PRE_DATASET_PROMOTION => {
                assert!(
                    generation_dir.join(STAGING_DIR_NAME).exists(),
                    "pre-dataset crash must leave unpromoted staging dataset"
                );
                assert!(
                    !generation_dir.join(BACKUP_DIR_NAME).exists(),
                    "pre-dataset crash must not create backup dataset"
                );
            }
            POST_DATASET_PRE_CONTRACT_PROMOTION => {
                assert!(
                    !generation_dir.join(STAGING_DIR_NAME).exists(),
                    "post-dataset crash must have consumed staging dataset"
                );
                assert!(
                    generation_dir.join(BACKUP_DIR_NAME).exists(),
                    "post-dataset crash must preserve prior dataset as backup"
                );
            }
            other => return Err(format!("unknown promotion crash boundary {other}").into()),
        }

        let seal_batch = seal_existing_generation_batch(base_batch);
        crate::run_blocking(&runtime, build_batch(root, &seal_batch))?;

        let recovered = GenerationContract::decode(&std::fs::read(&contract_path)?)?;
        let expected_corpora = match boundary {
            PRE_DATASET_PROMOTION => vec!["SymbolCard"],
            POST_DATASET_PRE_CONTRACT_PROMOTION => vec!["ModuleCard", "SymbolCard"],
            other => return Err(format!("unknown promotion crash boundary {other}").into()),
        };
        assert_eq!(recovered.required_corpora, expected_corpora);
        assert!(!staged_contract_path.exists());
        assert!(!generation_dir.join(STAGING_DIR_NAME).exists());
        assert!(!generation_dir.join(BACKUP_DIR_NAME).exists());

        let loaded = crate::run_blocking(
            &runtime,
            open_generation(root, &repo_id(), &revision_id(), generation),
        )?;
        let symbol_hits = crate::run_blocking(
            &runtime,
            loaded.search_hits_filtered_async(&[1.0, 0.0, 0.0], 10, Some("SymbolCard")),
        )?;
        let module_hits = crate::run_blocking(
            &runtime,
            loaded.search_hits_filtered_async(&[0.0, 1.0, 0.0], 10, Some("ModuleCard")),
        )?;
        assert_eq!(symbol_hits.len(), 1);
        assert_eq!(
            module_hits.len(),
            if boundary == POST_DATASET_PRE_CONTRACT_PROMOTION {
                1
            } else {
                0
            },
            "recovered dataset must match the contract selected by recovery"
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts subprocess crash recovery invariants"
    )]
    fn subprocess_crash_during_generation_promotion_recovers_complete_pair() -> TestResult {
        if let Some(root) = child_root_from_env()? {
            let boundary = env::var(PROMOTION_CRASH_BOUNDARY_ENV)
                .map_err(|err| format!("promotion crash child missing boundary: {err}"))?;
            return run_promotion_crash_child(&root, &boundary);
        }

        for boundary in [PRE_DATASET_PROMOTION, POST_DATASET_PRE_CONTRACT_PROMOTION] {
            let temp = tempdir()?;
            let root = temp.path().to_path_buf();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let base_batch = promotion_batch(
                ManifestGeneration::new(90),
                "symbol-before-crash",
                OwnerDocKind::Symbol,
                "symbol:before-crash",
                SemanticCorpusKindV1::SymbolCard,
                vec![SemanticCorpusKindV1::SymbolCard],
                vec![1.0, 0.0, 0.0],
            )?;
            crate::run_blocking(&runtime, build_batch(&root, &base_batch))?;

            let test_name =
                "build::tests::subprocess_crash_during_generation_promotion_recovers_complete_pair";
            let status = Command::new(env::current_exe()?)
                .arg("--exact")
                .arg(test_name)
                .arg("--nocapture")
                .env(PROMOTION_CRASH_ROOT_ENV, &root)
                .env(PROMOTION_CRASH_BOUNDARY_ENV, boundary)
                .status()?;
            assert_eq!(
                status.code(),
                Some(PROMOTION_CRASH_EXIT_CODE),
                "child must stop exactly at {boundary}; status={status}"
            );

            assert_recovered_promotion_state(&root, boundary, base_batch)?;
        }
        Ok(())
    }

    #[test]
    fn semantic_scope_authority_allows_distinct_owners_on_same_path() -> TestResult {
        let generation = ManifestGeneration::new(45);
        let mut batch = batch(
            generation,
            "src/shared.rs",
            "symbol-a",
            vec![1.0, 0.0, 0.0],
            false,
        )?;
        batch.replace_scopes.push(SemanticReplaceScope {
            scope: scope("src/shared.rs"),
            scope_digest: "scope:src/shared.rs:module-b".to_string(),
            embeddings: vec![embedding(
                "module-b",
                "src/shared.rs",
                OwnerDocKind::Module,
                "module-b",
                SemanticCorpusKindV1::ModuleCard,
                vec![0.0, 1.0, 0.0],
            )?],
        });

        validate_batch_scope_authority_v1(&batch)?;

        batch.replace_scopes.push(SemanticReplaceScope {
            scope: scope("src/other.rs"),
            scope_digest: "scope:src/other.rs:symbol-a".to_string(),
            embeddings: vec![embedding(
                "symbol-a-duplicate",
                "src/other.rs",
                OwnerDocKind::Chunk,
                "owner-symbol-a",
                SemanticCorpusKindV1::RawCodeFallback,
                vec![0.0, 0.0, 1.0],
            )?],
        });
        let duplicate_owner = validate_batch_scope_authority_v1(&batch)
            .expect_err("duplicate semantic owner authority must remain rejected");
        assert!(matches!(
            duplicate_owner,
            CoreError::InvalidContract(message)
                if message.contains("duplicate replace semantic scope")
        ));
        Ok(())
    }

    #[test]
    fn generation_contract_accumulates_corpora_across_mutation_and_seal_batches() -> TestResult {
        let temp = tempdir()?;
        let generation_dir = temp.path().join("generation");
        let generation = ManifestGeneration::new(46);
        let mut symbol_batch = batch(
            generation,
            "src/shared.rs",
            "symbol-a",
            vec![1.0, 0.0, 0.0],
            false,
        )?;
        symbol_batch.required_corpora = vec![SemanticCorpusKindV1::SymbolCard];
        symbol_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
        let symbol_contract = ensure_generation_contract(&generation_dir, &symbol_batch)?;
        assert_eq!(symbol_contract.required_corpora, vec!["SymbolCard"]);
        assert!(!layout::build_contract_path(&generation_dir).exists());
        persist_generation_contract(&generation_dir, &symbol_contract)?;

        let mut module_batch = batch(
            generation,
            "src/shared.rs",
            "module-b",
            vec![0.0, 1.0, 0.0],
            false,
        )?;
        module_batch.replace_scopes[0].embeddings[0].owner_kind = OwnerDocKind::Module;
        module_batch.replace_scopes[0].embeddings[0].owner_id = "module-b".into();
        module_batch.replace_scopes[0].embeddings[0].corpus_kind = SemanticCorpusKindV1::ModuleCard;
        module_batch.required_corpora = vec![SemanticCorpusKindV1::ModuleCard];
        module_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
        let merged_contract = ensure_generation_contract(&generation_dir, &module_batch)?;
        assert_eq!(
            merged_contract.required_corpora,
            vec!["ModuleCard", "SymbolCard"]
        );
        let still_symbol_only = GenerationContract::decode(&std::fs::read(
            layout::build_contract_path(&generation_dir),
        )?)?;
        assert_eq!(still_symbol_only.required_corpora, vec!["SymbolCard"]);
        persist_generation_contract(&generation_dir, &merged_contract)?;

        let mut seal_batch = module_batch;
        seal_batch.batch_digest = "batch:46:seal".to_string();
        seal_batch.required_corpora.clear();
        seal_batch.replace_scopes.clear();
        seal_batch.seal = true;
        let sealed_contract = ensure_generation_contract(&generation_dir, &seal_batch)?;
        assert_eq!(
            sealed_contract.required_corpora,
            merged_contract.required_corpora
        );
        persist_generation_contract(&generation_dir, &sealed_contract)?;

        let persisted = GenerationContract::decode(&std::fs::read(layout::build_contract_path(
            &generation_dir,
        ))?)?;
        assert_eq!(persisted.required_corpora, sealed_contract.required_corpora);
        Ok(())
    }

    #[test]
    fn generation_contract_recovery_distinguishes_pre_and_post_dataset_promotion() -> TestResult {
        let temp = tempdir()?;
        let generation_dir = temp.path().join("generation");
        std::fs::create_dir_all(layout::dataset_dir(&generation_dir))?;
        let generation = ManifestGeneration::new(49);
        let mut symbol_batch = batch(
            generation,
            "src/recovery.rs",
            "symbol-a",
            vec![1.0, 0.0, 0.0],
            false,
        )?;
        symbol_batch.required_corpora = vec![SemanticCorpusKindV1::SymbolCard];
        symbol_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
        let symbol_contract = GenerationContract::from_batch(&symbol_batch);
        persist_generation_contract(&generation_dir, &symbol_contract)?;

        let mut module_batch = symbol_batch.clone();
        module_batch.required_corpora = vec![SemanticCorpusKindV1::ModuleCard];
        let merged_contract = symbol_contract.merge_batch(&module_batch)?;
        let staged_path = stage_generation_contract(&generation_dir, &merged_contract)?;
        recover_dataset_artifacts(&generation_dir)?;
        assert!(!staged_path.exists());
        let pre_promotion_recovered = GenerationContract::decode(&std::fs::read(
            layout::build_contract_path(&generation_dir),
        )?)?;
        assert_eq!(pre_promotion_recovered.required_corpora, vec!["SymbolCard"]);

        let staged_path = stage_generation_contract(&generation_dir, &merged_contract)?;
        std::fs::create_dir_all(generation_dir.join(BACKUP_DIR_NAME))?;
        recover_dataset_artifacts(&generation_dir)?;
        assert!(!staged_path.exists());
        assert!(!generation_dir.join(BACKUP_DIR_NAME).exists());
        let post_promotion_recovered = GenerationContract::decode(&std::fs::read(
            layout::build_contract_path(&generation_dir),
        )?)?;
        assert_eq!(
            post_promotion_recovered.required_corpora,
            vec!["ModuleCard", "SymbolCard"]
        );
        Ok(())
    }

    #[test]
    fn failed_append_does_not_advance_generation_corpus_policy() -> TestResult {
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let generation = ManifestGeneration::new(47);
        let mut symbol_batch = batch(
            generation,
            "src/policy.rs",
            "symbol-a",
            vec![1.0, 0.0, 0.0],
            false,
        )?;
        symbol_batch.replace_scopes[0].embeddings[0].owner_kind = OwnerDocKind::Symbol;
        symbol_batch.replace_scopes[0].embeddings[0].owner_id = "symbol-a".into();
        symbol_batch.replace_scopes[0].embeddings[0].corpus_kind = SemanticCorpusKindV1::SymbolCard;
        symbol_batch.required_corpora = vec![SemanticCorpusKindV1::SymbolCard];
        symbol_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
        crate::run_blocking(&runtime, build_batch(&root, &symbol_batch))?;

        let mut module_batch = batch(
            generation,
            "src/policy.rs",
            "module-b",
            vec![0.0, 1.0, 0.0],
            false,
        )?;
        module_batch.replace_scopes[0].embeddings[0].owner_kind = OwnerDocKind::Module;
        module_batch.replace_scopes[0].embeddings[0].owner_id = "module-b".into();
        module_batch.replace_scopes[0].embeddings[0].corpus_kind = SemanticCorpusKindV1::ModuleCard;
        module_batch.required_corpora = vec![SemanticCorpusKindV1::ModuleCard];
        module_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
        failpoint::set_append_fail_path(Some("src/policy.rs"));
        let failure = crate::run_blocking(&runtime, build_batch(&root, &module_batch));
        failpoint::clear_append_fail_path("src/policy.rs");
        assert!(matches!(failure, Err(CoreError::Storage(_))));

        let generation_dir = layout::generation_dir(&root, &repo_id(), &revision_id(), generation);
        let persisted = GenerationContract::decode(&std::fs::read(layout::build_contract_path(
            &generation_dir,
        ))?)?;
        assert_eq!(persisted.required_corpora, vec!["SymbolCard"]);
        Ok(())
    }

    #[test]
    fn failed_contract_promotion_rolls_back_dataset_and_policy() -> TestResult {
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let generation = ManifestGeneration::new(48);
        let generation_dir = layout::generation_dir(&root, &repo_id(), &revision_id(), generation);
        let mut symbol_batch = batch(
            generation,
            "src/transaction.rs",
            "symbol-a",
            vec![1.0, 0.0, 0.0],
            false,
        )?;
        symbol_batch.replace_scopes[0].embeddings[0].owner_kind = OwnerDocKind::Symbol;
        symbol_batch.replace_scopes[0].embeddings[0].owner_id = "symbol-a".into();
        symbol_batch.replace_scopes[0].embeddings[0].corpus_kind = SemanticCorpusKindV1::SymbolCard;
        symbol_batch.required_corpora = vec![SemanticCorpusKindV1::SymbolCard];
        symbol_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
        crate::run_blocking(&runtime, build_batch(&root, &symbol_batch))?;

        let mut module_batch = batch(
            generation,
            "src/transaction.rs",
            "module-b",
            vec![0.0, 1.0, 0.0],
            false,
        )?;
        module_batch.replace_scopes[0].embeddings[0].owner_kind = OwnerDocKind::Module;
        module_batch.replace_scopes[0].embeddings[0].owner_id = "module-b".into();
        module_batch.replace_scopes[0].embeddings[0].corpus_kind = SemanticCorpusKindV1::ModuleCard;
        module_batch.required_corpora = vec![SemanticCorpusKindV1::ModuleCard];
        module_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
        let generation_dir_text = generation_dir.to_string_lossy().into_owned();
        failpoint::set_contract_promotion_fail_dir(Some(generation_dir_text.as_str()));
        let failure = crate::run_blocking(&runtime, build_batch(&root, &module_batch));
        failpoint::set_contract_promotion_fail_dir(None);
        assert!(matches!(failure, Err(CoreError::Storage(_))));

        let persisted = GenerationContract::decode(&std::fs::read(layout::build_contract_path(
            &generation_dir,
        ))?)?;
        assert_eq!(persisted.required_corpora, vec!["SymbolCard"]);
        assert!(
            !layout::build_contract_path(&generation_dir)
                .with_extension("next")
                .exists()
        );

        let mut seal_batch = symbol_batch;
        seal_batch.batch_digest = "batch:48:seal".to_string();
        seal_batch.required_corpora.clear();
        seal_batch.replace_scopes.clear();
        seal_batch.seal = true;
        crate::run_blocking(&runtime, build_batch(&root, &seal_batch))?;
        let loaded = crate::run_blocking(
            &runtime,
            open_generation(&root, &repo_id(), &revision_id(), generation),
        )?;
        let module_hits = crate::run_blocking(
            &runtime,
            loaded.search_hits_filtered_async(&[0.0, 1.0, 0.0], 10, Some("ModuleCard")),
        )?;
        assert!(
            module_hits.is_empty(),
            "failed contract promotion must roll back appended ModuleCard rows: {module_hits:?}"
        );
        Ok(())
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
        failpoint::clear_append_fail_path("a.rs");
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
                    required_corpora: Vec::new(),
                    corpus_policy_digest: None,
                    clear_surfaces: Vec::new(),
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

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts v4 metadata preservation via exact storage checks"
    )]
    fn scv2_02_v4_round_trip_preserves_metadata_fields() -> TestResult {
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let generation = ManifestGeneration::new(41);
        let batch = SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:41".to_string(),
            batch_digest: "batch:41".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(),
            required_corpora: vec![SemanticCorpusKindV1::SymbolCard],
            corpus_policy_digest: Some("policy:semantic:v1".to_string()),
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: scope("src/lib.rs"),
                scope_digest: "scope:src/lib.rs".to_string(),
                embeddings: vec![embedding(
                    "meta-1",
                    "src/lib.rs",
                    OwnerDocKind::Symbol,
                    "symbol-1",
                    SemanticCorpusKindV1::SymbolCard,
                    vec![1.0, 0.0, 0.0],
                )?],
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        };
        crate::run_blocking(&runtime, build_batch(&root, &batch))?;

        let generation_dir = layout::generation_dir(&root, &repo_id(), &revision_id(), generation);
        let manifest =
            SemanticManifest::decode(&std::fs::read(layout::manifest_path(&generation_dir))?)?;
        assert_eq!(manifest.present_corpora, vec!["SymbolCard".to_string()]);
        assert_eq!(manifest.required_corpora, vec!["SymbolCard".to_string()]);
        assert_eq!(manifest.card_schema_versions, vec![1]);
        assert_eq!(
            manifest.render_policy_digests,
            vec!["render:meta-1".to_string()]
        );
        assert_eq!(
            manifest.corpus_policy_digest,
            Some("policy:semantic:v1".to_string())
        );

        let connection = crate::run_blocking(
            &runtime,
            open_connection(&layout::dataset_dir(&generation_dir)),
        )?;
        let table = crate::run_blocking(&runtime, connection.open_table(TABLE_NAME).execute())?;
        let stream = crate::run_blocking(&runtime, table.query().execute())?;
        let batches: Vec<arrow_array::RecordBatch> =
            crate::run_blocking(&runtime, stream.try_collect())?;
        let batch = batches
            .into_iter()
            .next()
            .ok_or("expected one result batch")?;
        let record_id_col =
            column_as::<arrow_array::StringArray>(&batch, COLUMN_RECORD_ID, "Utf8")?;
        let owner_id_col = column_as::<arrow_array::StringArray>(&batch, COLUMN_OWNER_ID, "Utf8")?;
        let corpus_kind_col =
            column_as::<arrow_array::StringArray>(&batch, COLUMN_CORPUS_KIND, "Utf8")?;
        let parent_owner_col =
            column_as::<arrow_array::StringArray>(&batch, COLUMN_PARENT_OWNER_ID, "Utf8")?;
        let source_doc_col =
            column_as::<arrow_array::StringArray>(&batch, COLUMN_SOURCE_DOC_ID, "Utf8")?;
        let package_col = column_as::<arrow_array::StringArray>(&batch, COLUMN_PACKAGE, "Utf8")?;
        let visibility_col =
            column_as::<arrow_array::StringArray>(&batch, COLUMN_VISIBILITY, "Utf8")?;
        let source_role_col =
            column_as::<arrow_array::StringArray>(&batch, COLUMN_SOURCE_ROLE, "Utf8")?;
        let generated_col =
            column_as::<arrow_array::BooleanArray>(&batch, COLUMN_GENERATED, "Boolean")?;
        let capability_col =
            column_as::<arrow_array::StringArray>(&batch, COLUMN_CAPABILITY_STATUS, "Utf8")?;
        let authority_col =
            column_as::<arrow_array::StringArray>(&batch, COLUMN_AUTHORITY_DIGEST, "Utf8")?;
        let render_col =
            column_as::<arrow_array::StringArray>(&batch, COLUMN_RENDER_POLICY_DIGEST, "Utf8")?;
        let schema_col =
            column_as::<arrow_array::UInt32Array>(&batch, COLUMN_CARD_SCHEMA_VERSION, "UInt32")?;
        assert_eq!(record_id_col.value(0), "record-meta-1");
        assert_eq!(owner_id_col.value(0), "symbol-1");
        assert_eq!(corpus_kind_col.value(0), "SymbolCard");
        assert!(parent_owner_col.is_null(0));
        assert_eq!(source_doc_col.value(0), "doc-meta-1");
        assert_eq!(package_col.value(0), "crate");
        assert_eq!(visibility_col.value(0), "pub");
        assert_eq!(source_role_col.value(0), "CardText");
        assert!(!generated_col.value(0));
        assert_eq!(capability_col.value(0), "Full");
        assert_eq!(authority_col.value(0), "auth:meta-1");
        assert_eq!(render_col.value(0), "render:meta-1");
        assert_eq!(schema_col.value(0), 1);
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts semantic-scope tombstone isolation via exact search assertions"
    )]
    fn scv2_02_same_path_delete_one_owner_keeps_other() -> TestResult {
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let generation = ManifestGeneration::new(42);
        let seed = SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:42a".to_string(),
            batch_digest: "batch:42a".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(),
            required_corpora: Vec::new(),
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: scope("src/shared.rs"),
                scope_digest: "scope:src/shared.rs".to_string(),
                embeddings: vec![
                    embedding(
                        "symbol-a",
                        "src/shared.rs",
                        OwnerDocKind::Symbol,
                        "symbol-a",
                        SemanticCorpusKindV1::SymbolCard,
                        vec![1.0, 0.0, 0.0],
                    )?,
                    embedding(
                        "module-b",
                        "src/shared.rs",
                        OwnerDocKind::Module,
                        "module-b",
                        SemanticCorpusKindV1::ModuleCard,
                        vec![0.0, 1.0, 0.0],
                    )?,
                ],
            }],
            tombstone_scopes: Vec::new(),
            seal: false,
        };
        crate::run_blocking(&runtime, build_batch(&root, &seed))?;
        let delete_one = SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:42b".to_string(),
            batch_digest: "batch:42b".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(),
            required_corpora: Vec::new(),
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: vec![SemanticTombstoneScope {
                scope: None,
                semantic_scope: Some(semantic_scope(
                    SemanticCorpusKindV1::SymbolCard,
                    OwnerDocKind::Symbol,
                    "symbol-a",
                )),
            }],
            seal: true,
        };
        crate::run_blocking(&runtime, build_batch(&root, &delete_one))?;
        let loaded = crate::run_blocking(
            &runtime,
            open_generation(&root, &repo_id(), &revision_id(), generation),
        )?;
        let removed = crate::run_blocking(
            &runtime,
            loaded.search_hits_filtered_async(&[1.0, 0.0, 0.0], 10, Some("SymbolCard")),
        )?;
        assert!(
            removed.is_empty(),
            "deleted semantic scope must be gone: {removed:?}"
        );
        let kept = crate::run_blocking(
            &runtime,
            loaded.search_hits_filtered_async(&[0.0, 1.0, 0.0], 10, Some("ModuleCard")),
        )?;
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].record_id, "record-module-b");
        assert_eq!(kept[0].owner_id, "module-b");
        Ok(())
    }

    #[test]
    fn clear_symbol_surface_removes_exact_and_fallback_rows_only_v1() -> TestResult {
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let generation = ManifestGeneration::new(52);
        let seed = SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:52a".to_string(),
            batch_digest: "batch:52a".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(),
            required_corpora: Vec::new(),
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: scope("src/shared.rs"),
                scope_digest: "scope:src/shared.rs".to_string(),
                embeddings: vec![
                    embedding(
                        "symbol-exact",
                        "src/shared.rs",
                        OwnerDocKind::Symbol,
                        "symbol-exact",
                        SemanticCorpusKindV1::SymbolCard,
                        vec![1.0, 0.0, 0.0],
                    )?,
                    embedding(
                        "symbol-fallback",
                        "src/shared.rs",
                        OwnerDocKind::Callsite,
                        "callsite-fallback",
                        SemanticCorpusKindV1::RawCodeFallback,
                        vec![0.9, 0.1, 0.0],
                    )?,
                    embedding(
                        "module-kept",
                        "src/shared.rs",
                        OwnerDocKind::Module,
                        "module-kept",
                        SemanticCorpusKindV1::ModuleCard,
                        vec![0.0, 1.0, 0.0],
                    )?,
                ],
            }],
            tombstone_scopes: Vec::new(),
            seal: false,
        };
        crate::run_blocking(&runtime, build_batch(&root, &seed))?;

        let clear = SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:52b".to_string(),
            batch_digest: "batch:52b".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(),
            required_corpora: Vec::new(),
            corpus_policy_digest: None,
            clear_surfaces: vec![SearchScopeSurface::Symbol],
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            seal: true,
        };
        crate::run_blocking(&runtime, build_batch(&root, &clear))?;

        let loaded = crate::run_blocking(
            &runtime,
            open_generation(&root, &repo_id(), &revision_id(), generation),
        )?;
        let exact_symbol = crate::run_blocking(
            &runtime,
            loaded.search_hits_filtered_async(&[1.0, 0.0, 0.0], 10, Some("SymbolCard")),
        )?;
        let fallback_symbol = crate::run_blocking(
            &runtime,
            loaded.search_hits_filtered_async(&[0.9, 0.1, 0.0], 10, Some("RawCodeFallback")),
        )?;
        let module = crate::run_blocking(
            &runtime,
            loaded.search_hits_filtered_async(&[0.0, 1.0, 0.0], 10, Some("ModuleCard")),
        )?;
        assert!(exact_symbol.is_empty());
        assert!(fallback_symbol.is_empty());
        assert_eq!(module.len(), 1);
        assert_eq!(module[0].record_id, "record-module-kept");
        Ok(())
    }

    #[test]
    fn scv2_02_missing_required_corpus_fails_seal() -> TestResult {
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let generation = ManifestGeneration::new(43);
        let batch = SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:43".to_string(),
            batch_digest: "batch:43".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(),
            required_corpora: vec![
                SemanticCorpusKindV1::SymbolCard,
                SemanticCorpusKindV1::ModuleCard,
            ],
            corpus_policy_digest: Some("policy:semantic:v1".to_string()),
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: scope("src/missing.rs"),
                scope_digest: "scope:src/missing.rs".to_string(),
                embeddings: vec![embedding(
                    "missing-1",
                    "src/missing.rs",
                    OwnerDocKind::Symbol,
                    "symbol-missing",
                    SemanticCorpusKindV1::SymbolCard,
                    vec![1.0, 0.0, 0.0],
                )?],
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        };
        let err = crate::run_blocking(&runtime, build_batch(&root, &batch))
            .expect_err("missing required corpus must fail seal");
        match err {
            CoreError::Storage(message) => assert!(
                message.contains("required corpus `ModuleCard` missing"),
                "seal failure must name the missing corpus, got {message}"
            ),
            other => panic!("expected storage error for missing required corpus, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts corpus filter excludes other corpora via exact hit metadata"
    )]
    fn scv2_02_filtered_search_excludes_other_corpora() -> TestResult {
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let generation = ManifestGeneration::new(44);
        let batch = SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:44".to_string(),
            batch_digest: "batch:44".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(),
            required_corpora: vec![
                SemanticCorpusKindV1::SymbolCard,
                SemanticCorpusKindV1::ModuleCard,
            ],
            corpus_policy_digest: Some("policy:semantic:v1".to_string()),
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: scope("src/filter.rs"),
                scope_digest: "scope:src/filter.rs".to_string(),
                embeddings: vec![
                    embedding(
                        "filter-symbol",
                        "src/filter.rs",
                        OwnerDocKind::Symbol,
                        "symbol-filter",
                        SemanticCorpusKindV1::SymbolCard,
                        vec![1.0, 0.0, 0.0],
                    )?,
                    embedding(
                        "filter-module",
                        "src/filter.rs",
                        OwnerDocKind::Module,
                        "module-filter",
                        SemanticCorpusKindV1::ModuleCard,
                        vec![0.0, 1.0, 0.0],
                    )?,
                ],
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        };
        crate::run_blocking(&runtime, build_batch(&root, &batch))?;
        let loaded = crate::run_blocking(
            &runtime,
            open_generation(&root, &repo_id(), &revision_id(), generation),
        )?;
        let symbol_hits = crate::run_blocking(
            &runtime,
            loaded.search_hits_filtered_async(&[1.0, 0.0, 0.0], 10, Some("SymbolCard")),
        )?;
        assert_eq!(symbol_hits.len(), 1);
        assert_eq!(symbol_hits[0].record_id, "record-filter-symbol");
        assert_eq!(symbol_hits[0].owner_id, "symbol-filter");
        assert_eq!(symbol_hits[0].corpus_kind.as_deref(), Some("SymbolCard"));
        let module_hits = crate::run_blocking(
            &runtime,
            loaded.search_hits_filtered_async(&[0.0, 1.0, 0.0], 10, Some("ModuleCard")),
        )?;
        assert_eq!(module_hits.len(), 1);
        assert_eq!(module_hits[0].record_id, "record-filter-module");
        assert_eq!(module_hits[0].owner_id, "module-filter");
        assert_eq!(module_hits[0].corpus_kind.as_deref(), Some("ModuleCard"));
        Ok(())
    }
}
