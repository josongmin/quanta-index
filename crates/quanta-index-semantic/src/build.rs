//! Lancedb-backed durable generation build path (LDB-00 §3.2 revised).
//!
//! `build_stream` applies one streamed semantic batch to the generation's
//! lancedb dataset: open or create a connection rooted at a staging copy of
//! `{generation_dir}/dataset/`, open or create the `semantic` table, clear
//! the batch's surfaces, then take the batch's replace scopes one bounded
//! window at a time from the caller's source (QI-BB-021): each window is
//! admitted against the window policy, held to the model contract, its
//! owners' rows deleted and its rows appended as one Arrow `RecordBatch`,
//! and dropped before the next window is requested, so at most one window
//! of vectors is resident in this crate. The tombstones follow the last
//! window. On `seal` the manifest commitment runs once over the whole
//! table, then the staging dataset is promoted, the READY marker written,
//! and the scope manifest then the SEALED marker (last) so a crash mid-seal
//! stays not-ready. A window refused for any reason leaves the promoted
//! dataset untouched: the staging copy is discarded by the next build's
//! recovery. A sealed generation is immutable: a later batch targeting it
//! fails closed.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::{
    Array, BooleanArray, FixedSizeListArray, Float32Array, RecordBatch, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field};
use futures::TryStreamExt as _;
use lancedb::connect;
use lancedb::query::{ExecutableQuery as _, QueryBase as _};
use quanta_index_contract::{
    EmbeddingDistanceMetric, EmbeddingNormalization, EmbeddingRecord, OwnerDocKind,
    SearchScopeSurface, SemanticCorpusKindV1, SemanticReplaceScope, SemanticTombstoneScope,
    canonical_order::first_canonical_order_break_v1, cluster_membership_content_digest_v1,
};
use quanta_index_core::domains::semantic::SemanticPolicy;
use quanta_index_core::{
    CoreError, SemanticIngestHeaderV1, SemanticScopeSource, SemanticScopeWindowV1,
    SemanticStreamTallyV1, SemanticStreamWindowPolicy, owner_key_v1,
};

use crate::errors::{arrow_err, fs_err, lancedb_err};
use crate::generation_contract::GenerationContract;
use crate::layout::{
    self, CLUSTER_MEMBERSHIP_TABLE_NAME, COLUMN_AUTHORITY_DIGEST, COLUMN_CARD_SCHEMA_VERSION,
    COLUMN_CORPUS_KIND, COLUMN_MEMBERSHIP_AUTHORITY_DIGEST, COLUMN_MEMBERSHIP_CLUSTER_RECORD_ID,
    COLUMN_MEMBERSHIP_CONTENT_DIGEST, COLUMN_MEMBERSHIP_MEMBER_COUNT,
    COLUMN_MEMBERSHIP_MEMBER_SYMBOL_ID, COLUMN_MEMBERSHIP_ORDINAL, COLUMN_MEMBERSHIP_OWNER_ID,
    COLUMN_MEMBERSHIP_OWNER_KIND, COLUMN_OWNER_ID, COLUMN_OWNER_KIND, COLUMN_RENDER_POLICY_DIGEST,
    COLUMN_REPO_RELATIVE_PATH, COLUMN_VECTOR, TABLE_NAME, cluster_membership_schema,
    dimension_to_i32, semantic_schema,
};
use crate::manifest::{
    ClusterMembershipSealV1, SemanticCorpusCoverageV1, SemanticManifest, SemanticRowSealV1,
    VectorIndexSealV1,
};
use crate::membership_integrity::{
    ClusterMembershipCommitmentV1, ClusterMembershipStoredRowV1, cluster_membership_commitment_v1,
};
use crate::sealed_manifest::{build_sealed_manifest_bytes, sealed_manifest_path};
use crate::semantic_row_integrity_v1::semantic_row_commitment_v1;
use crate::vector_index::{VectorIndexSealInputV1, seal_vector_index_v1};

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
    if let Ok(configured) = std::env::var(PROMOTION_CRASH_BOUNDARY_ENV)
        && configured == boundary
    {
        std::process::exit(PROMOTION_CRASH_EXIT_CODE);
    }
}

#[cfg(any(test, debug_assertions))]
mod failpoint {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::sync::{Mutex, OnceLock};

    #[cfg(test)]
    static ATOMIC_WRITE_FAIL_BEFORE_RENAME_ACTION: OnceLock<Mutex<Option<String>>> =
        OnceLock::new();
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

    #[cfg(test)]
    pub(super) fn set_atomic_write_fail_before_rename_action(action: Option<&str>) {
        let slot = ATOMIC_WRITE_FAIL_BEFORE_RENAME_ACTION.get_or_init(|| Mutex::new(None));
        let mut guard = match slot.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        *guard = action.map(str::to_owned);
    }

    #[cfg(test)]
    pub(super) fn should_fail_atomic_write_before_rename(action: &str) -> bool {
        let slot = ATOMIC_WRITE_FAIL_BEFORE_RENAME_ACTION.get_or_init(|| Mutex::new(None));
        let guard = match slot.lock() {
            Ok(guard) => guard,
            Err(err) => err.into_inner(),
        };
        guard.as_deref() == Some(action)
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

static ATOMIC_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Crash-atomic and crash-durable file replacement for generation sidecars.
///
/// The file payload is synced before rename and the parent directory is synced
/// after rename. Pre-rename failures remove the unique temporary file so a
/// failed build cannot accumulate or later promote stale staging artifacts.
fn write_atomic(path: &Path, bytes: &[u8], action: &str) -> Result<(), CoreError> {
    let parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: {action} target has no parent: {}",
            path.display()
        ))
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: {action} target has no file name: {}",
            path.display()
        ))
    })?;
    let sequence = ATOMIC_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut staging_name = OsString::from(".");
    staging_name.push(file_name);
    staging_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
    let staging = parent.join(staging_name);

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .map_err(|err| fs_err(action, &staging, &err))?;
    if let Err(err) = file.write_all(bytes) {
        drop(file);
        return Err(cleanup_atomic_temporary(
            &staging,
            fs_err(action, &staging, &err),
        ));
    }
    if let Err(err) = file.sync_all() {
        drop(file);
        return Err(cleanup_atomic_temporary(
            &staging,
            fs_err(action, &staging, &err),
        ));
    }
    drop(file);

    #[cfg(test)]
    if failpoint::should_fail_atomic_write_before_rename(action) {
        return Err(cleanup_atomic_temporary(
            &staging,
            CoreError::Storage(format!("semantic: injected {action} failure before rename")),
        ));
    }

    if let Err(err) = fs::rename(&staging, path) {
        return Err(cleanup_atomic_temporary(
            &staging,
            fs_err(action, path, &err),
        ));
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|err| fs_err(action, parent, &err))
}

fn cleanup_atomic_temporary(staging: &Path, primary: CoreError) -> CoreError {
    match fs::remove_file(staging) {
        Ok(()) => primary,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => primary,
        Err(cleanup) => CoreError::Storage(format!(
            "semantic: {primary}; additionally failed to remove atomic temporary {}: {cleanup}",
            staging.display()
        )),
    }
}

/// The one dataset file `LanceDB` rewrites on every commit.
///
/// It is the pointer to the newest manifest: bookkeeping, not content, and it
/// must stay generation-local. Every other object `LanceDB` writes is an
/// immutable versioned file (G0-S), safe to share between generations by hard
/// link.
const LANCE_LATEST_VERSION_HINT_FILE_NAME: &str = "latest_version_hint.json";

fn is_dataset_local_entry(file_name: &std::ffi::OsStr) -> bool {
    file_name == LANCE_LATEST_VERSION_HINT_FILE_NAME
}

/// Materialize `dst` from the immutable dataset at `src` without copying its
/// bytes.
///
/// Regular files become hard links into `src`, directories recurse, and the
/// generation-local version hint is copied because every commit rewrites it.
/// Whether `LanceDB` replaces that file by rename or writes it in place is an
/// implementation detail this adapter does not rely on: a shared inode would
/// let one generation's commit repoint another's, so it is never linked.
/// A link failure is a typed error, not a silent full copy: both trees live
/// under the same state root, so a cross-device failure cannot happen, and any
/// other failure would otherwise cost the incremental guarantee unnoticed.
fn inherit_dataset_tree(src: &Path, dst: &Path) -> Result<(), CoreError> {
    fs::create_dir_all(dst).map_err(|err| fs_err("create inherited dataset dir", dst, &err))?;
    for entry in fs::read_dir(src).map_err(|err| fs_err("read source dataset dir", src, &err))? {
        let entry = entry.map_err(|err| fs_err("read source dataset entry", src, &err))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|err| fs_err("inherited dataset entry type", &from, &err))?;
        if file_type.is_dir() {
            inherit_dataset_tree(&from, &to)?;
        } else if is_dataset_local_entry(&entry.file_name()) {
            let _bytes = fs::copy(&from, &to)
                .map_err(|err| fs_err("copy generation-local dataset entry", &from, &err))?;
        } else {
            fs::hard_link(&from, &to)
                .map_err(|err| fs_err("link inherited dataset entry", &from, &err))?;
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

async fn ensure_cluster_membership_table(
    connection: &lancedb::Connection,
) -> Result<lancedb::Table, CoreError> {
    let names = connection
        .table_names()
        .execute()
        .await
        .map_err(|err| lancedb_err("table_names", err))?;
    if names
        .iter()
        .any(|name| name == CLUSTER_MEMBERSHIP_TABLE_NAME)
    {
        let table = connection
            .open_table(CLUSTER_MEMBERSHIP_TABLE_NAME)
            .execute()
            .await
            .map_err(|err| {
                lancedb_err(&format!("open_table {CLUSTER_MEMBERSHIP_TABLE_NAME}"), err)
            })?;
        let schema = table
            .schema()
            .await
            .map_err(|err| lancedb_err("read cluster membership schema", err))?;
        for expected in cluster_membership_schema().fields() {
            let live = schema.field_with_name(expected.name()).map_err(|err| {
                CoreError::Storage(format!(
                    "semantic: cluster membership table missing column `{}`: {err}",
                    expected.name()
                ))
            })?;
            if live.data_type() != expected.data_type() || live.is_nullable() {
                return Err(CoreError::Storage(format!(
                    "semantic: cluster membership column `{}` has incompatible schema",
                    expected.name()
                )));
            }
        }
        return Ok(table);
    }
    connection
        .create_empty_table(CLUSTER_MEMBERSHIP_TABLE_NAME, cluster_membership_schema())
        .execute()
        .await
        .map_err(|err| {
            lancedb_err(
                &format!("create_empty_table {CLUSTER_MEMBERSHIP_TABLE_NAME}"),
                err,
            )
        })
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

/// Every ingested vector is held to the batch's model contract (QI-BB-031).
///
/// That is exactly `dimension` finite components, and under `L2Unit` a unit
/// norm, so a generation sealed as `l2_unit` describes its rows' bytes.
fn validate_replace_scope(
    scope: &SemanticReplaceScope,
    dimension: usize,
    normalization: EmbeddingNormalization,
) -> Result<(), CoreError> {
    for embedding in &scope.embeddings {
        SemanticPolicy::validate_embedding_vector_v1(&embedding.vector, dimension, normalization)
            .map_err(|err| match err {
            CoreError::Typed { code, message } => CoreError::Typed {
                code,
                message: format!("{message} (embedding {})", embedding.embedding_id.as_str()),
            },
            other @ (CoreError::InvalidContract(_)
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => other,
        })?;
    }
    let mut cluster_record_ids = BTreeSet::new();
    for membership in &scope.cluster_memberships {
        membership.validate_v1().map_err(|message| {
            CoreError::InvalidContract(format!(
                "semantic: invalid cluster membership {}: {message}",
                membership.cluster_record_id
            ))
        })?;
        if !cluster_record_ids.insert(membership.cluster_record_id.as_str()) {
            return Err(CoreError::InvalidContract(format!(
                "semantic: duplicate cluster membership record_id {:?}",
                membership.cluster_record_id
            )));
        }
        let embedding = scope
            .embeddings
            .iter()
            .find(|embedding| embedding.record_id.as_ref() == membership.cluster_record_id)
            .ok_or_else(|| {
                CoreError::InvalidContract(format!(
                    "semantic: cluster membership {:?} has no embedding in the same replace scope",
                    membership.cluster_record_id
                ))
            })?;
        if embedding.corpus_kind != SemanticCorpusKindV1::ClusterCard {
            return Err(CoreError::InvalidContract(format!(
                "semantic: membership {:?} does not identify a ClusterCard embedding",
                membership.cluster_record_id
            )));
        }
        if embedding.authority_digest.as_ref() != membership.authority_digest {
            return Err(CoreError::InvalidContract(format!(
                "semantic: membership {:?} authority digest does not match its ClusterCard embedding",
                membership.cluster_record_id
            )));
        }
    }
    let cluster_embedding_count = scope
        .embeddings
        .iter()
        .filter(|embedding| embedding.corpus_kind == SemanticCorpusKindV1::ClusterCard)
        .count();
    if cluster_embedding_count != scope.cluster_memberships.len() {
        return Err(CoreError::InvalidContract(format!(
            "semantic: ClusterCard embeddings require one structured membership each; embeddings={cluster_embedding_count} memberships={}",
            scope.cluster_memberships.len()
        )));
    }
    if first_canonical_order_break_v1(&scope.cluster_memberships, |membership| {
        membership.cluster_record_id.as_str()
    })
    .is_some()
    {
        return Err(CoreError::InvalidContract(
            "semantic: cluster memberships must use canonical cluster_record_id order".to_string(),
        ));
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

/// The scope authority of one streamed batch, checked window by window.
///
/// The header's clear surfaces and tombstones are known before the first
/// window and validated against each other up front; every replace scope
/// is then checked as it arrives against them and against every replace
/// scope before it, so a conflict is refused at the window that carries
/// it. The rules are the ones the all-at-once validation applied: a
/// surface is not cleared and replaced or tombstoned in one batch, an owner
/// scope or path scope is not replaced and tombstoned in one batch, and no
/// record id or owner scope is replaced twice.
struct StreamScopeAuthorityV1 {
    clear_surfaces: BTreeSet<SearchScopeSurface>,
    tombstone_path_keys: BTreeSet<String>,
    tombstone_semantic_keys: BTreeSet<String>,
    replace_semantic_keys: BTreeSet<String>,
    record_ids: BTreeSet<String>,
}

impl StreamScopeAuthorityV1 {
    fn new(header: &SemanticIngestHeaderV1) -> Result<Self, CoreError> {
        let mut clear_surfaces = BTreeSet::new();
        for surface in &header.mutations.clear_surfaces {
            if !clear_surfaces.insert(*surface) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: duplicate clear surface {surface:?}"
                )));
            }
        }
        if first_canonical_order_break_v1(&header.mutations.clear_surfaces, |surface| surface)
            .is_some()
        {
            return Err(CoreError::InvalidContract(
                "semantic: clear surfaces must use canonical ascending order".to_string(),
            ));
        }
        let mut tombstone_path_keys = BTreeSet::new();
        let mut tombstone_semantic_keys = BTreeSet::new();
        for tombstone in &header.mutations.tombstone_scopes {
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
                if !tombstone_path_keys.insert(path_scope_key_v1(scope)) {
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
                if !tombstone_semantic_keys.insert(key) {
                    return Err(CoreError::InvalidContract(format!(
                        "semantic: duplicate tombstone owner scope {:?}",
                        scope.owner_id
                    )));
                }
            }
        }
        Ok(Self {
            clear_surfaces,
            tombstone_path_keys,
            tombstone_semantic_keys,
            replace_semantic_keys: BTreeSet::new(),
            record_ids: BTreeSet::new(),
        })
    }

    /// Admit one replace scope of the stream, or refuse it typed.
    fn admit_replace_scope(&mut self, scope: &SemanticReplaceScope) -> Result<(), CoreError> {
        if self.clear_surfaces.contains(&scope.scope.doc_surface) {
            return Err(CoreError::InvalidContract(format!(
                "semantic: surface {:?} cannot be cleared and replaced in one batch",
                scope.scope.doc_surface
            )));
        }
        if self
            .tombstone_path_keys
            .contains(&path_scope_key_v1(&scope.scope))
        {
            return Err(CoreError::InvalidContract(format!(
                "semantic: path scope {:?} cannot be replaced and tombstoned in one batch",
                scope.scope.repo_relative_path.as_str()
            )));
        }
        let mut scope_semantic_keys = BTreeSet::new();
        for embedding in &scope.embeddings {
            let surface = SearchScopeSurface::for_semantic_owner_v1(
                embedding.owner_kind,
                embedding.corpus_kind,
            );
            if self.clear_surfaces.contains(&surface) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: surface {surface:?} cannot be cleared and replaced in one batch"
                )));
            }
            if !self.record_ids.insert(embedding.record_id.to_string()) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: duplicate embedding record_id {:?}",
                    embedding.record_id
                )));
            }
            let (corpus_kind, owner_kind, owner_id) = owner_key_v1(embedding);
            let _scope_key_was_new = scope_semantic_keys.insert(semantic_scope_key_v1(
                corpus_kind,
                owner_kind,
                owner_id,
            ));
        }
        for semantic_key in scope_semantic_keys {
            if self.tombstone_semantic_keys.contains(&semantic_key) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: owner scope {semantic_key:?} cannot be replaced and tombstoned in one batch"
                )));
            }
            if !self.replace_semantic_keys.insert(semantic_key.clone()) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: duplicate replace semantic scope {semantic_key:?}"
                )));
            }
        }
        Ok(())
    }
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

async fn validate_cluster_membership_coverage_v1(
    semantic_table: &lancedb::Table,
    membership_table: &lancedb::Table,
) -> Result<(), CoreError> {
    let cluster_predicate = format!(
        "{COLUMN_CORPUS_KIND} = {}",
        crate::sql::quote_sql_string(SemanticCorpusKindV1::ClusterCard.as_code_str())
    );
    let semantic_batches: Vec<RecordBatch> = semantic_table
        .query()
        .only_if(cluster_predicate)
        .execute()
        .await
        .map_err(|err| lancedb_err("query ClusterCard coverage", err))?
        .try_collect()
        .await
        .map_err(|err| lancedb_err("collect ClusterCard coverage", err))?;
    let mut expected = BTreeSet::new();
    for batch in semantic_batches {
        let record_ids = column_as::<StringArray>(&batch, crate::layout::COLUMN_RECORD_ID, "Utf8")?;
        let authority_digests = column_as::<StringArray>(&batch, COLUMN_AUTHORITY_DIGEST, "Utf8")?;
        for row in 0..batch.num_rows() {
            if !expected.insert((
                record_ids.value(row).to_owned(),
                authority_digests.value(row).to_owned(),
            )) {
                return Err(CoreError::Storage(format!(
                    "semantic: duplicate ClusterCard record_id {:?} in sealed dataset",
                    record_ids.value(row)
                )));
            }
        }
    }
    let membership_batches: Vec<RecordBatch> = membership_table
        .query()
        .execute()
        .await
        .map_err(|err| lancedb_err("query cluster membership coverage", err))?
        .try_collect()
        .await
        .map_err(|err| lancedb_err("collect cluster membership coverage", err))?;
    let mut observed = BTreeSet::new();
    for batch in membership_batches {
        let record_ids =
            column_as::<StringArray>(&batch, COLUMN_MEMBERSHIP_CLUSTER_RECORD_ID, "Utf8")?;
        let authority_digests =
            column_as::<StringArray>(&batch, COLUMN_MEMBERSHIP_AUTHORITY_DIGEST, "Utf8")?;
        for row in 0..batch.num_rows() {
            let _inserted = observed.insert((
                record_ids.value(row).to_owned(),
                authority_digests.value(row).to_owned(),
            ));
        }
    }
    if observed != expected {
        return Err(CoreError::InvalidContract(format!(
            "semantic: sealed ClusterCard membership coverage mismatch; cluster_records={} membership_records={}",
            expected.len(),
            observed.len()
        )));
    }
    Ok(())
}

async fn collect_cluster_membership_commitment_v1(
    membership_table: &lancedb::Table,
) -> Result<ClusterMembershipCommitmentV1, CoreError> {
    let row_count = membership_table
        .count_rows(None)
        .await
        .map_err(|error| lancedb_err("count cluster membership rows", error))?;
    let mut rows = Vec::with_capacity(row_count);
    let mut stream = membership_table
        .query()
        .execute()
        .await
        .map_err(|error| lancedb_err("query cluster membership commitment", error))?;
    while let Some(batch) = stream
        .try_next()
        .await
        .map_err(|error| lancedb_err("stream cluster membership commitment", error))?
    {
        if rows
            .len()
            .checked_add(batch.num_rows())
            .is_none_or(|next| next > row_count)
        {
            return Err(CoreError::Storage(
                "semantic: cluster membership stream exceeded its counted row bound".to_string(),
            ));
        }
        if batch
            .columns()
            .iter()
            .any(|column| column.null_count() != 0)
        {
            return Err(CoreError::Storage(
                "semantic: cluster membership commitment stream contains null values".to_string(),
            ));
        }
        let cluster_record_ids =
            column_as::<StringArray>(&batch, COLUMN_MEMBERSHIP_CLUSTER_RECORD_ID, "Utf8")?;
        let authority_digests =
            column_as::<StringArray>(&batch, COLUMN_MEMBERSHIP_AUTHORITY_DIGEST, "Utf8")?;
        let owner_kinds = column_as::<StringArray>(&batch, COLUMN_MEMBERSHIP_OWNER_KIND, "Utf8")?;
        let owner_ids = column_as::<StringArray>(&batch, COLUMN_MEMBERSHIP_OWNER_ID, "Utf8")?;
        let member_symbol_ids =
            column_as::<StringArray>(&batch, COLUMN_MEMBERSHIP_MEMBER_SYMBOL_ID, "Utf8")?;
        let ordinals = column_as::<UInt32Array>(&batch, COLUMN_MEMBERSHIP_ORDINAL, "UInt32")?;
        let member_counts =
            column_as::<UInt32Array>(&batch, COLUMN_MEMBERSHIP_MEMBER_COUNT, "UInt32")?;
        let membership_digests =
            column_as::<StringArray>(&batch, COLUMN_MEMBERSHIP_CONTENT_DIGEST, "Utf8")?;
        for row in 0..batch.num_rows() {
            rows.push(ClusterMembershipStoredRowV1 {
                cluster_record_id: cluster_record_ids.value(row).to_owned(),
                authority_digest: authority_digests.value(row).to_owned(),
                owner_kind: owner_kinds.value(row).to_owned(),
                owner_id: owner_ids.value(row).to_owned(),
                member_symbol_id: member_symbol_ids.value(row).to_owned(),
                ordinal: ordinals.value(row),
                member_count: member_counts.value(row),
                membership_digest: membership_digests.value(row).to_owned(),
            });
        }
    }
    if rows.len() != row_count {
        return Err(CoreError::Storage(format!(
            "semantic: cluster membership stream rows {} != counted rows {row_count}",
            rows.len()
        )));
    }
    cluster_membership_commitment_v1(rows)
        .map_err(|error| CoreError::Storage(format!("semantic: {error}")))
}

/// One string column built straight from the rows, without an
/// intermediate `Vec<String>` copy of every value (QI-BB-021).
fn string_column<'a>(
    rows: &[&'a EmbeddingRecord],
    value: impl Fn(&'a EmbeddingRecord) -> &'a str,
) -> Arc<dyn Array> {
    Arc::new(StringArray::from_iter_values(
        rows.iter().map(|embedding| value(embedding)),
    ))
}

/// One nullable string column built straight from the rows.
fn optional_string_column<'a>(
    rows: &[&'a EmbeddingRecord],
    value: impl Fn(&'a EmbeddingRecord) -> Option<&'a str>,
) -> Arc<dyn Array> {
    Arc::new(
        rows.iter()
            .map(|embedding| value(embedding))
            .collect::<StringArray>(),
    )
}

/// One `u32` column built straight from the rows.
fn u32_column<'a>(
    rows: &[&'a EmbeddingRecord],
    value: impl Fn(&'a EmbeddingRecord) -> u32,
) -> Arc<dyn Array> {
    Arc::new(UInt32Array::from_iter_values(
        rows.iter().map(|embedding| value(embedding)),
    ))
}

/// The Arrow batch of one window's rows: every replace scope's embeddings
/// in window order.
fn build_record_batch(
    rows: &[&EmbeddingRecord],
    dimension: usize,
) -> Result<RecordBatch, CoreError> {
    let row_count = rows.len();
    // The vectors are the one column that must be copied: Arrow wants them
    // contiguous. Every metadata column is built from borrowed values.
    let mut flat_vectors: Vec<f32> = Vec::with_capacity(row_count.saturating_mul(dimension));
    for embedding in rows {
        flat_vectors.extend_from_slice(&embedding.vector);
    }
    let generated_array = rows
        .iter()
        .map(|embedding| Some(embedding.generated))
        .collect::<BooleanArray>();
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
            string_column(rows, |embedding| embedding.embedding_id.as_str()),
            string_column(rows, |embedding| embedding.record_id.as_ref()),
            string_column(rows, |embedding| embedding.repo_relative_path.as_str()),
            string_column(rows, |embedding| embedding.owner_id.as_ref()),
            string_column(rows, |embedding| embedding.owner_kind.as_code_str()),
            string_column(rows, |embedding| embedding.corpus_kind.as_code_str()),
            optional_string_column(rows, |embedding| embedding.parent_owner_id.as_deref()),
            string_column(rows, |embedding| embedding.source_doc_id.as_ref()),
            string_column(rows, |embedding| embedding.language.as_str()),
            optional_string_column(rows, |embedding| embedding.package.as_deref()),
            optional_string_column(rows, |embedding| {
                embedding
                    .symbol_kind
                    .as_ref()
                    .map(quanta_index_contract::lex::SymbolKindCode::as_str)
            }),
            optional_string_column(rows, |embedding| embedding.visibility.as_deref()),
            string_column(rows, |embedding| embedding.source_role.as_code_str()),
            Arc::new(generated_array),
            string_column(rows, |embedding| embedding.capability_status.as_code_str()),
            string_column(rows, |embedding| embedding.authority_digest.as_ref()),
            string_column(rows, |embedding| embedding.render_policy_digest.as_ref()),
            u32_column(rows, |embedding| embedding.card_schema_version),
            string_column(rows, |embedding| embedding.embedding_input_digest.as_ref()),
            string_column(rows, |embedding| embedding.vector_digest.as_ref()),
            u32_column(rows, |embedding| embedding.start_line),
            u32_column(rows, |embedding| embedding.end_line),
            string_column(rows, |embedding| embedding.snippet.as_ref()),
            Arc::new(vector_array),
        ],
    )
    .map_err(|err| arrow_err("RecordBatch::try_new", err))
}

fn build_cluster_membership_record_batch(
    scope: &SemanticReplaceScope,
) -> Result<Option<RecordBatch>, CoreError> {
    let row_count = scope
        .cluster_memberships
        .iter()
        .map(|membership| membership.members.len())
        .sum();
    if row_count == 0 {
        return Ok(None);
    }
    let mut cluster_record_ids = Vec::with_capacity(row_count);
    let mut authority_digests = Vec::with_capacity(row_count);
    let mut owner_kinds = Vec::with_capacity(row_count);
    let mut owner_ids = Vec::with_capacity(row_count);
    let mut member_symbol_ids = Vec::with_capacity(row_count);
    let mut ordinals = Vec::with_capacity(row_count);
    let mut member_counts = Vec::with_capacity(row_count);
    let mut membership_digests = Vec::with_capacity(row_count);
    for membership in &scope.cluster_memberships {
        let embedding = scope
            .embeddings
            .iter()
            .find(|embedding| embedding.record_id.as_ref() == membership.cluster_record_id)
            .ok_or_else(|| {
                CoreError::InvalidContract(format!(
                    "semantic: cluster membership {:?} lost its validated embedding",
                    membership.cluster_record_id
                ))
            })?;
        let member_count = u32::try_from(membership.members.len()).map_err(|error| {
            CoreError::InvalidContract(format!(
                "semantic: cluster membership member count overflow: {error}"
            ))
        })?;
        let membership_digest = cluster_membership_content_digest_v1(&membership.members);
        for (ordinal, member) in membership.members.iter().enumerate() {
            cluster_record_ids.push(membership.cluster_record_id.clone());
            authority_digests.push(membership.authority_digest.clone());
            owner_kinds.push(embedding.owner_kind.as_code_str().to_string());
            owner_ids.push(embedding.owner_id.to_string());
            member_symbol_ids.push(member.as_str().to_string());
            ordinals.push(u32::try_from(ordinal).map_err(|error| {
                CoreError::InvalidContract(format!(
                    "semantic: cluster membership ordinal overflow: {error}"
                ))
            })?);
            member_counts.push(member_count);
            membership_digests.push(membership_digest.clone());
        }
    }
    RecordBatch::try_new(
        cluster_membership_schema(),
        vec![
            Arc::new(StringArray::from(cluster_record_ids)),
            Arc::new(StringArray::from(authority_digests)),
            Arc::new(StringArray::from(owner_kinds)),
            Arc::new(StringArray::from(owner_ids)),
            Arc::new(StringArray::from(member_symbol_ids)),
            Arc::new(UInt32Array::from(ordinals)),
            Arc::new(UInt32Array::from(member_counts)),
            Arc::new(StringArray::from(membership_digests)),
        ],
    )
    .map(Some)
    .map_err(|error| {
        CoreError::Storage(format!(
            "semantic: build cluster membership record batch: {error}"
        ))
    })
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

async fn delete_cluster_membership_by_owner(
    table: &lancedb::Table,
    owner_kind: &str,
    owner_id: &str,
) -> Result<(), CoreError> {
    let predicate = format!(
        "{COLUMN_MEMBERSHIP_OWNER_KIND} = {} AND {COLUMN_MEMBERSHIP_OWNER_ID} = {}",
        crate::sql::quote_sql_string(owner_kind),
        crate::sql::quote_sql_string(owner_id),
    );
    let _result = table
        .delete(predicate.as_str())
        .await
        .map_err(|err| lancedb_err(&format!("delete membership predicate `{predicate}`"), err))?;
    Ok(())
}

async fn delete_cluster_membership_for_surface(
    table: &lancedb::Table,
    surface: SearchScopeSurface,
) -> Result<(), CoreError> {
    let owner_kinds = OwnerDocKind::ALL
        .iter()
        .copied()
        .filter(|owner_kind| {
            SearchScopeSurface::for_semantic_owner_v1(
                *owner_kind,
                SemanticCorpusKindV1::ClusterCard,
            ) == surface
        })
        .map(OwnerDocKind::as_code_str)
        .map(crate::sql::quote_sql_string)
        .map(|owner_kind| format!("{COLUMN_MEMBERSHIP_OWNER_KIND} = {owner_kind}"))
        .collect::<Vec<_>>();
    if owner_kinds.is_empty() {
        return Ok(());
    }
    let predicate = owner_kinds.join(" OR ");
    let _result = table.delete(predicate.as_str()).await.map_err(|err| {
        lancedb_err(
            &format!("delete cluster membership surface predicate `{predicate}`"),
            err,
        )
    })?;
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

async fn delete_replace_scope_memberships(
    table: &lancedb::Table,
    scope: &SemanticReplaceScope,
) -> Result<(), CoreError> {
    let mut owners = BTreeSet::new();
    for embedding in &scope.embeddings {
        if embedding.corpus_kind == SemanticCorpusKindV1::ClusterCard {
            let _inserted = owners.insert((
                embedding.owner_kind.as_code_str(),
                embedding.owner_id.as_ref(),
            ));
        }
    }
    for (owner_kind, owner_id) in owners {
        delete_cluster_membership_by_owner(table, owner_kind, owner_id).await?;
    }
    Ok(())
}

async fn delete_tombstone_memberships(
    table: &lancedb::Table,
    scope: &SemanticTombstoneScope,
) -> Result<(), CoreError> {
    let Some(semantic_scope) = scope.semantic_scope.as_ref() else {
        return Ok(());
    };
    if semantic_scope.corpus_kind != SemanticCorpusKindV1::ClusterCard {
        return Ok(());
    }
    delete_cluster_membership_by_owner(
        table,
        semantic_scope.owner_kind.as_code_str(),
        semantic_scope.owner_id.as_str(),
    )
    .await
}

/// Append every row of `window` as one batch, after its owners' rows are
/// gone.
async fn append_window(
    table: &lancedb::Table,
    window: &SemanticScopeWindowV1,
    dimension: usize,
) -> Result<(), CoreError> {
    let mut rows: Vec<&EmbeddingRecord> = Vec::new();
    for scope in window.scopes() {
        if failpoint::should_fail_append(scope.scope.repo_relative_path.as_str()) {
            return Err(CoreError::Storage(format!(
                "semantic: injected append failure for path {}",
                scope.scope.repo_relative_path.as_str()
            )));
        }
        rows.extend(scope.embeddings.iter());
    }
    if rows.is_empty() {
        return Ok(());
    }
    let batch = build_record_batch(&rows, dimension)?;
    let _result = table
        .add(batch)
        .execute()
        .await
        .map_err(|err| lancedb_err("table.add", err))?;
    Ok(())
}

async fn append_cluster_membership_scope(
    table: &lancedb::Table,
    scope: &SemanticReplaceScope,
) -> Result<(), CoreError> {
    let Some(batch) = build_cluster_membership_record_batch(scope)? else {
        return Ok(());
    };
    let _result = table
        .add(batch)
        .execute()
        .await
        .map_err(|err| lancedb_err("cluster membership table.add", err))?;
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
    header: &SemanticIngestHeaderV1,
) -> Result<GenerationContract, CoreError> {
    GenerationContract::validate_batch_shape(&header.contract)?;
    let contract_path = layout::build_contract_path(generation_dir);
    if contract_path.exists() {
        let bytes = fs::read(&contract_path)
            .map_err(|err| fs_err("read generation contract", &contract_path, &err))?;
        let contract = GenerationContract::decode(&bytes)?;
        return contract.merge_batch(&header.contract);
    }
    if layout::dataset_dir(generation_dir).exists() {
        return Err(CoreError::Storage(format!(
            "semantic: dataset exists but generation contract is missing for {}",
            generation_dir.display()
        )));
    }
    fs::create_dir_all(generation_dir)
        .map_err(|err| fs_err("create generation dir", generation_dir, &err))?;
    Ok(GenerationContract::from_batch(&header.contract))
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
        inherit_dataset_tree(&paths.dataset, &paths.staging)?;
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
        inherit_dataset_tree(&base_dataset, &paths.staging)?;
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

/// The sealed index contract of the base generation a delta inherited its
/// dataset from (QI-BB-027 W3).
///
/// Read from the base's scope manifest and validated against the base's
/// own scope. `None` when the generation has no base or the base's
/// manifest predates the contract.
///
/// The base's manifest is a required input of a delta seal: without it the
/// seal cannot tell whether the index the dataset carries is the policy's
/// own, or what lineage it has. A base that has vanished or become
/// unreadable since the delta's first batch therefore refuses the seal
/// rather than retraining as if nothing had been inherited.
fn inherited_vector_index_seal_v1(
    semantic_root: &Path,
    header: &SemanticIngestHeaderV1,
    generation_contract: &GenerationContract,
) -> Result<Option<VectorIndexSealV1>, CoreError> {
    let Some(base_generation) = generation_contract.base_generation else {
        return Ok(None);
    };
    let base_dir = layout::generation_dir(
        semantic_root,
        &header.pin.repo_id,
        &header.pin.revision_id,
        base_generation,
    );
    let manifest_path = layout::manifest_path(&base_dir);
    let bytes = fs::read(&manifest_path)
        .map_err(|err| fs_err("read delta base scope manifest", &manifest_path, &err))?;
    let manifest = SemanticManifest::decode(&bytes)?;
    manifest.validate_scope(
        &header.pin.repo_id,
        &header.pin.revision_id,
        base_generation,
    )?;
    Ok(manifest.vector_index_seal()?.cloned())
}

/// The scope manifest of the sealed generation, committed once over the
/// whole working table after the last window and the tombstones.
async fn build_manifest_bytes(
    semantic_root: &Path,
    table: &lancedb::Table,
    membership_table: &lancedb::Table,
    header: &SemanticIngestHeaderV1,
    generation_contract: &GenerationContract,
) -> Result<Vec<u8>, CoreError> {
    let row_count = table
        .count_rows(None)
        .await
        .map_err(|err| lancedb_err("count_rows", err))?;
    let row_count_u64 = u64::try_from(row_count)
        .map_err(|err| CoreError::Storage(format!("semantic: row count overflow: {err}")))?;
    let coverage = collect_manifest_coverage(table).await?;
    let semantic_rows = semantic_row_commitment_v1(table).await?;
    if semantic_rows.row_count != row_count_u64 {
        return Err(CoreError::Storage(format!(
            "semantic: row commitment count {} != table row count {row_count_u64}",
            semantic_rows.row_count
        )));
    }
    let membership_commitment = collect_cluster_membership_commitment_v1(membership_table).await?;

    // The dense lane's index contract (QI-BB-027): built with every
    // parameter explicit, read back from the library, and recorded in the
    // manifest so the open can verify the dataset against the seal. A
    // delta appends to the index it inherited when the base's contract
    // admits it and trains otherwise. The index is approximate; what the
    // seal makes deterministic is its recipe, effort and lineage, not its
    // top-k.
    let inherited = inherited_vector_index_seal_v1(semantic_root, header, generation_contract)?;
    let vector_index = seal_vector_index_v1(
        table,
        VectorIndexSealInputV1 {
            generation: header.pin.manifest_generation.get(),
            row_count: row_count_u64,
            inherited: inherited.as_ref(),
        },
    )
    .await?;

    let built_at = built_at_unix_nanos()?;
    let manifest = SemanticManifest::from_generation_contract(
        &header.pin,
        generation_contract,
        header.batch.manifest_digest.as_str(),
        SemanticRowSealV1 {
            row_count: row_count_u64,
            root_digest: semantic_rows.root_digest,
            built_at_unix_nanos: built_at,
        },
        SemanticCorpusCoverageV1 {
            present: coverage.present_corpora,
            required: generation_contract.required_corpora.clone(),
            card_schema_versions: coverage.card_schema_versions,
            render_policy_digests: coverage.render_policy_digests,
            policy_digest: generation_contract.corpus_policy_digest.clone(),
        },
        ClusterMembershipSealV1 {
            root_digest: membership_commitment.root_digest,
            cluster_count: membership_commitment.cluster_count,
            member_row_count: membership_commitment.member_row_count,
        },
        vector_index,
    );
    manifest.validate_corpus_coverage()?;
    manifest.encode()
}

/// The working tables of one build: the semantic table and its cluster
/// membership table on the staging dataset.
struct WorkingTables {
    table: lancedb::Table,
    membership_table: lancedb::Table,
}

async fn open_working_tables(
    working_dataset: &Path,
    dimension: usize,
) -> Result<WorkingTables, CoreError> {
    let connection = open_connection(working_dataset).await?;
    let table = ensure_table(&connection, dimension).await?;
    let membership_table = ensure_cluster_membership_table(&connection).await?;
    Ok(WorkingTables {
        table,
        membership_table,
    })
}

async fn apply_clear_surfaces(
    tables: &WorkingTables,
    surfaces: &[SearchScopeSurface],
) -> Result<(), CoreError> {
    for surface in surfaces {
        delete_surface_rows(&tables.table, *surface).await?;
        delete_cluster_membership_for_surface(&tables.membership_table, *surface).await?;
    }
    Ok(())
}

/// Replace one window: every scope's owners are deleted, then the window's
/// rows are appended as one batch and its memberships per scope.
async fn apply_window(
    tables: &WorkingTables,
    window: &SemanticScopeWindowV1,
    dimension: usize,
) -> Result<(), CoreError> {
    for scope in window.scopes() {
        delete_replace_scope_rows(&tables.table, scope).await?;
        delete_replace_scope_memberships(&tables.membership_table, scope).await?;
    }
    append_window(&tables.table, window, dimension).await?;
    for scope in window.scopes() {
        append_cluster_membership_scope(&tables.membership_table, scope).await?;
    }
    Ok(())
}

async fn apply_tombstones(
    tables: &WorkingTables,
    tombstones: &[SemanticTombstoneScope],
) -> Result<(), CoreError> {
    for scope in tombstones {
        delete_tombstone_scope_rows(&tables.table, scope).await?;
        delete_tombstone_memberships(&tables.membership_table, scope).await?;
    }
    Ok(())
}

async fn seal_manifest_bytes(
    semantic_root: &Path,
    tables: &WorkingTables,
    header: &SemanticIngestHeaderV1,
    generation_contract: &GenerationContract,
) -> Result<Vec<u8>, CoreError> {
    validate_cluster_membership_coverage_v1(&tables.table, &tables.membership_table).await?;
    build_manifest_bytes(
        semantic_root,
        &tables.table,
        &tables.membership_table,
        header,
        generation_contract,
    )
    .await
}

/// Take every window of `scopes`, admit it, hold it to the contract and
/// append it, dropping each before the next is requested.
///
/// Runs on the caller's thread: the source embeds on the calling thread,
/// outside the runtime, and only the storage steps are driven through the
/// crate's async seam.
fn apply_scope_stream(
    runtime: &tokio::runtime::Runtime,
    tables: &WorkingTables,
    policy: SemanticStreamWindowPolicy,
    header: &SemanticIngestHeaderV1,
    authority: &mut StreamScopeAuthorityV1,
    scopes: &mut dyn SemanticScopeSource,
) -> Result<SemanticStreamTallyV1, CoreError> {
    let dimension = header.dimension()?;
    let normalization = header.contract.model_contract.normalization;
    let mut tally = SemanticStreamTallyV1::default();
    while let Some(window) = scopes.next_window()? {
        let _fill = policy.admit(&window)?;
        for scope in window.scopes() {
            validate_replace_scope(scope, dimension, normalization)?;
            authority.admit_replace_scope(scope)?;
        }
        let rows = window.rows()?;
        crate::run_blocking(runtime, apply_window(tables, &window, dimension))?;
        tally.count_window(window.scopes().len(), rows, window.vector_bytes())?;
        drop(window);
    }
    Ok(tally)
}

/// Apply one streamed batch to the lancedb-backed durable generation,
/// sealing on the header's `seal`.
///
/// Returns what was appended, counted on this side: the caller compares it
/// with the source's own tally.
pub(crate) fn build_stream(
    runtime: &tokio::runtime::Runtime,
    semantic_root: &Path,
    policy: SemanticStreamWindowPolicy,
    header: &SemanticIngestHeaderV1,
    scopes: &mut dyn SemanticScopeSource,
) -> Result<SemanticStreamTallyV1, CoreError> {
    let generation_dir = layout::generation_dir(
        semantic_root,
        &header.pin.repo_id,
        &header.pin.revision_id,
        header.pin.manifest_generation,
    );
    if layout::sealed_marker_path(&generation_dir).exists() {
        return Err(CoreError::Storage(format!(
            "semantic: generation {} is already sealed; refusing in-place mutation",
            header.pin.manifest_generation.get()
        )));
    }
    if header.contract.model_contract.distance_metric != EmbeddingDistanceMetric::Cosine {
        return Err(CoreError::InvalidContract(format!(
            "semantic: unsupported distance metric {:?}; this backend serves cosine only",
            header.contract.model_contract.distance_metric
        )));
    }
    let dimension = header.dimension()?;
    // The non-streamed mutations are validated before the staging dataset
    // exists; every window is validated as it arrives, against the contract
    // and against everything before it. A refusal at any window leaves the
    // promoted dataset untouched: only the staging copy holds the rows
    // appended so far, and the next build's recovery discards it.
    let mut authority = StreamScopeAuthorityV1::new(header)?;

    recover_dataset_artifacts(&generation_dir)?;
    let generation_contract = ensure_generation_contract(&generation_dir, header)?;
    let working_dataset = prepare_staging_dataset(
        semantic_root,
        &generation_dir,
        &generation_contract,
        &header.pin.repo_id,
        &header.pin.revision_id,
    )?;

    let (tally, manifest_bytes) = {
        let tables =
            crate::run_blocking(runtime, open_working_tables(&working_dataset, dimension))?;
        crate::run_blocking(
            runtime,
            apply_clear_surfaces(&tables, &header.mutations.clear_surfaces),
        )?;
        let tally = apply_scope_stream(runtime, &tables, policy, header, &mut authority, scopes)?;
        crate::run_blocking(
            runtime,
            apply_tombstones(&tables, &header.mutations.tombstone_scopes),
        )?;
        let manifest_bytes = if header.batch.seal {
            Some(crate::run_blocking(
                runtime,
                seal_manifest_bytes(semantic_root, &tables, header, &generation_contract),
            )?)
        } else {
            None
        };
        (tally, manifest_bytes)
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
        // The file commitment every door re-measures instead of the rows
        // (QI-BB-017): measured after the dataset, the contract and the
        // scope manifest are durable, written before the marker that makes
        // the generation sealed.
        let sealed_manifest_bytes =
            build_sealed_manifest_bytes(&generation_dir, header.batch.manifest_digest.as_str())?;
        write_atomic(
            &sealed_manifest_path(&generation_dir),
            &sealed_manifest_bytes,
            "write sealed generation manifest",
        )?;
        write_atomic(
            &layout::sealed_marker_path(&generation_dir),
            header.batch.manifest_digest.as_bytes(),
            "write sealed marker",
        )?;
    }
    Ok(tally)
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "fixtures in this module are built with known fixed lengths; an out-of-range index is a test authoring bug that should fail loudly"
)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning tests assert with `assert!`/`panic!` on fixture invariants; a violated fixture invariant is not a propagatable error"
)]
mod tests {
    use std::env;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use arrow_array::Array;
    use futures::TryStreamExt as _;
    use lancedb::query::ExecutableQuery as _;
    use tempfile::tempdir;

    use quanta_index_contract::{
        BatchIngestMode, CapabilityStatusV1, ClusterMembershipBatchReadRequestV1,
        ClusterMembershipReadFailureV1, ClusterMembershipReadOutcomeV1,
        ClusterMembershipReadRequestV1, ClusterMembershipReplaceV1, EmbeddingDistanceMetric,
        EmbeddingId, EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord,
        GenerationPin, ManifestGeneration, OwnerDocKind, RepoId, RepoRelativePath, RevisionId,
        SearchScopeKey, SearchScopeSurface, SemanticCorpusKindV1, SemanticIngestBatch,
        SemanticReplaceScope, SemanticSourceScopeKeyV1, SemanticTombstoneScope, SourceRoleV1,
        SymbolId, lex::LanguageCode,
    };
    use quanta_index_core::{
        CoreError, ResidentScopeSource, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES, SemanticIndexOpenPort,
        SemanticIngestHeaderV1, SemanticScopeSource as _, SemanticStreamWindowPolicy,
        build_resident_semantic_batch_v1,
    };

    use super::{
        BACKUP_DIR_NAME, POST_DATASET_PRE_CONTRACT_PROMOTION, PRE_DATASET_PROMOTION,
        PROMOTION_CRASH_BOUNDARY_ENV, PROMOTION_CRASH_EXIT_CODE, STAGING_DIR_NAME,
        StreamScopeAuthorityV1, build_stream, column_as, ensure_generation_contract, failpoint,
        open_connection, persist_generation_contract, recover_dataset_artifacts,
        stage_generation_contract, write_atomic,
    };
    use crate::generation_contract::GenerationContract;
    use crate::layout::{
        self, CLUSTER_MEMBERSHIP_TABLE_NAME, COLUMN_AUTHORITY_DIGEST, COLUMN_CAPABILITY_STATUS,
        COLUMN_CARD_SCHEMA_VERSION, COLUMN_CORPUS_KIND, COLUMN_GENERATED, COLUMN_OWNER_ID,
        COLUMN_PACKAGE, COLUMN_PARENT_OWNER_ID, COLUMN_RECORD_ID, COLUMN_RENDER_POLICY_DIGEST,
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
            card_schema_version: u32::from(corpus_kind != SemanticCorpusKindV1::RawCodeFallback),
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
                cluster_memberships: Vec::new(),
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
        record.card_schema_version =
            u32::from(corpus_kind != SemanticCorpusKindV1::RawCodeFallback);
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
        let result = build(&runtime, root, &module_batch);
        match result {
            Ok(()) => Err("promotion child returned without abrupt exit".into()),
            Err(err) => {
                Err(format!("promotion child returned error instead of abrupt exit: {err}").into())
            }
        }
    }

    fn child_root_from_env() -> Option<PathBuf> {
        env::var_os(PROMOTION_CRASH_ROOT_ENV).map(PathBuf::from)
    }

    /// Build an already-resident batch through the streamed entry under the
    /// production window policy.
    fn build(
        runtime: &tokio::runtime::Runtime,
        root: &Path,
        batch: &SemanticIngestBatch,
    ) -> Result<(), CoreError> {
        let header = SemanticIngestHeaderV1::of_batch(batch);
        let mut source =
            ResidentScopeSource::new(&batch.replace_scopes, SemanticStreamWindowPolicy::DEFAULT)?;
        let _tally = build_stream(
            runtime,
            root,
            SemanticStreamWindowPolicy::DEFAULT,
            &header,
            &mut source,
        )?;
        Ok(())
    }

    /// Build through the adapter's port, discarding the tally.
    fn build_with(
        adapter: &crate::SemanticAdapter,
        batch: &SemanticIngestBatch,
    ) -> Result<(), CoreError> {
        let _tally =
            build_resident_semantic_batch_v1(adapter, batch, SemanticStreamWindowPolicy::DEFAULT)?;
        Ok(())
    }

    /// Admit every replace scope of `batch` through the streaming authority.
    fn admit_scope_authority(batch: &SemanticIngestBatch) -> Result<(), CoreError> {
        let header = SemanticIngestHeaderV1::of_batch(batch);
        let mut authority = StreamScopeAuthorityV1::new(&header)?;
        for scope in &batch.replace_scopes {
            authority.admit_replace_scope(scope)?;
        }
        Ok(())
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
        build(&runtime, root, &seal_batch)?;

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
            usize::from(boundary == POST_DATASET_PRE_CONTRACT_PROMOTION),
            "recovered dataset must match the contract selected by recovery"
        );
        Ok(())
    }

    fn atomic_temporary_paths(
        parent: &Path,
        target: &Path,
    ) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
        let file_name = target
            .file_name()
            .ok_or("atomic-write test target has no file name")?
            .to_string_lossy();
        let prefix = format!(".{file_name}.tmp-");
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                paths.push(entry.path());
            }
        }
        Ok(paths)
    }

    #[test]
    fn atomic_sidecar_write_persists_payload_without_staging_residue() -> TestResult {
        let temp = tempdir()?;
        let target = temp.path().join("manifest.cbor");

        write_atomic(&target, b"durable-manifest", "test durable manifest")?;

        assert_eq!(std::fs::read(&target)?, b"durable-manifest");
        assert!(atomic_temporary_paths(temp.path(), &target)?.is_empty());
        Ok(())
    }

    #[test]
    fn atomic_sidecar_write_cleans_staging_on_pre_rename_failure() -> TestResult {
        const ACTION: &str = "test injected durable manifest";

        let temp = tempdir()?;
        let target = temp.path().join("manifest.cbor");
        failpoint::set_atomic_write_fail_before_rename_action(Some(ACTION));
        let result = write_atomic(&target, b"must-not-promote", ACTION);
        failpoint::set_atomic_write_fail_before_rename_action(None);

        assert!(result.is_err());
        assert!(!target.exists());
        assert!(atomic_temporary_paths(temp.path(), &target)?.is_empty());
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts subprocess crash recovery invariants"
    )]
    fn subprocess_crash_during_generation_promotion_recovers_complete_pair() -> TestResult {
        if let Some(root) = child_root_from_env() {
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
            build(&runtime, &root, &base_batch)?;

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
            cluster_memberships: Vec::new(),
        });

        admit_scope_authority(&batch)?;

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
            cluster_memberships: Vec::new(),
        });
        let duplicate_owner = admit_scope_authority(&batch)
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
        let symbol_contract = ensure_generation_contract(
            &generation_dir,
            &SemanticIngestHeaderV1::of_batch(&symbol_batch),
        )?;
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
        let merged_contract = ensure_generation_contract(
            &generation_dir,
            &SemanticIngestHeaderV1::of_batch(&module_batch),
        )?;
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
        let sealed_contract = ensure_generation_contract(
            &generation_dir,
            &SemanticIngestHeaderV1::of_batch(&seal_batch),
        )?;
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
        let symbol_contract = GenerationContract::from_batch(
            &SemanticIngestHeaderV1::of_batch(&symbol_batch).contract,
        );
        persist_generation_contract(&generation_dir, &symbol_contract)?;

        let mut module_batch = symbol_batch;
        module_batch.required_corpora = vec![SemanticCorpusKindV1::ModuleCard];
        let merged_contract = symbol_contract
            .merge_batch(&SemanticIngestHeaderV1::of_batch(&module_batch).contract)?;
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
        build(&runtime, &root, &symbol_batch)?;

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
        let failure = build(&runtime, &root, &module_batch);
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
        build(&runtime, &root, &symbol_batch)?;

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
        let failure = build(&runtime, &root, &module_batch);
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
        build(&runtime, &root, &seal_batch)?;
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

        build(
            &runtime,
            &root,
            &batch(generation, "a.rs", "emb-1", vec![1.0, 0.0, 0.0], false)?,
        )?;

        failpoint::set_append_fail_path(Some("a.rs"));
        let Err(err) = build(
            &runtime,
            &root,
            &batch(generation, "a.rs", "emb-2", vec![0.0, 1.0, 0.0], false)?,
        ) else {
            return Err("injected append failure must surface".into());
        };
        failpoint::clear_append_fail_path("a.rs");
        assert!(matches!(err, CoreError::Storage(_)));

        build(
            &runtime,
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
                cluster_memberships: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        };
        build(&runtime, &root, &batch)?;

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
                cluster_memberships: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            seal: false,
        };
        build(&runtime, &root, &seed)?;
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
        build(&runtime, &root, &delete_one)?;
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
                        vec![0.6, 0.8, 0.0],
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
                cluster_memberships: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            seal: false,
        };
        build(&runtime, &root, &seed)?;

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
        build(&runtime, &root, &clear)?;

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
    fn delta_empty_seal_preserves_required_raw_corpus_coverage() -> TestResult {
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let base_generation = ManifestGeneration::new(51);
        let delta_generation = ManifestGeneration::new(52);

        let mut base = batch(
            base_generation,
            "src/base.rs",
            "base-raw",
            vec![1.0, 0.0, 0.0],
            false,
        )?;
        base.required_corpora = vec![SemanticCorpusKindV1::RawCodeFallback];
        build(&runtime, &root, &base)?;
        build(&runtime, &root, &seal_existing_generation_batch(base))?;

        let mut delta = batch(
            delta_generation,
            "src/base.rs",
            "delta-raw",
            vec![0.0, 1.0, 0.0],
            false,
        )?;
        delta.mode = BatchIngestMode::Delta;
        delta.base_generation = Some(base_generation);
        delta.required_corpora = vec![SemanticCorpusKindV1::RawCodeFallback];
        build(&runtime, &root, &delta)?;
        build(&runtime, &root, &seal_existing_generation_batch(delta))?;

        let generation_dir =
            layout::generation_dir(&root, &repo_id(), &revision_id(), delta_generation);
        let manifest =
            SemanticManifest::decode(&std::fs::read(layout::manifest_path(&generation_dir))?)?;
        assert_eq!(manifest.required_corpora, vec!["RawCodeFallback"]);
        assert_eq!(manifest.present_corpora, vec!["RawCodeFallback"]);
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
                cluster_memberships: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        };
        let err =
            build(&runtime, &root, &batch).expect_err("missing required corpus must fail seal");
        match err {
            CoreError::Storage(message) => assert!(
                message.contains("required corpus `ModuleCard` missing"),
                "seal failure must name the missing corpus, got {message}"
            ),
            other @ (CoreError::InvalidContract(_)
            | CoreError::Typed { .. }
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)) => {
                panic!("expected storage error for missing required corpus, got {other:?}")
            }
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
                cluster_memberships: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        };
        build(&runtime, &root, &batch)?;
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

    #[test]
    fn cluster_membership_same_seal_replace_base_clone_and_tombstone_v1() -> TestResult {
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let adapter = crate::SemanticAdapter::with_state_root(root.clone())?;
        let base_generation = ManifestGeneration::new(71);
        let replacement_generation = ManifestGeneration::new(72);
        let tombstone_generation = ManifestGeneration::new(73);
        let cluster_scope = SemanticSourceScopeKeyV1 {
            corpus_kind: SemanticCorpusKindV1::ClusterCard,
            owner_kind: OwnerDocKind::Module,
            owner_id: "cluster-owner".to_string(),
        };
        let cluster_batch = |generation: ManifestGeneration,
                             base_generation: Option<ManifestGeneration>,
                             id: &str,
                             members: Vec<SymbolId>|
         -> Result<SemanticIngestBatch, String> {
            Ok(SemanticIngestBatch {
                repo_id: repo_id(),
                revision_id: revision_id(),
                generation,
                base_generation,
                manifest_digest: format!("manifest:{}", generation.get()),
                batch_digest: format!("batch:{}", generation.get()),
                mode: if base_generation.is_some() {
                    BatchIngestMode::Delta
                } else {
                    BatchIngestMode::ReplaceGeneration
                },
                model_contract: model_contract(),
                required_corpora: vec![SemanticCorpusKindV1::ClusterCard],
                corpus_policy_digest: Some("policy:cluster:v1".to_string()),
                clear_surfaces: Vec::new(),
                replace_scopes: vec![SemanticReplaceScope {
                    scope: scope("src/cluster.rs"),
                    scope_digest: format!("scope:{id}"),
                    embeddings: vec![embedding(
                        id,
                        "src/cluster.rs",
                        OwnerDocKind::Module,
                        "cluster-owner",
                        SemanticCorpusKindV1::ClusterCard,
                        vec![1.0, 0.0, 0.0],
                    )?],
                    cluster_memberships: vec![ClusterMembershipReplaceV1 {
                        cluster_record_id: format!("record-{id}"),
                        authority_digest: format!("auth:{id}"),
                        members,
                    }],
                }],
                tombstone_scopes: Vec::new(),
                seal: true,
            })
        };
        build_with(
            &adapter,
            &cluster_batch(
                base_generation,
                None,
                "cluster-a",
                vec![SymbolId::new("symbol:a"), SymbolId::new("symbol:b")],
            )?,
        )?;
        build_with(
            &adapter,
            &cluster_batch(
                replacement_generation,
                Some(base_generation),
                "cluster-b",
                vec![SymbolId::new("symbol:c"), SymbolId::new("symbol:d")],
            )?,
        )?;

        let replacement_searcher =
            adapter.open(&repo_id(), &revision_id(), replacement_generation)?;
        let available = replacement_searcher
            .cluster_membership_batch_read(&ClusterMembershipBatchReadRequestV1::single_v1(
                ClusterMembershipReadRequestV1 {
                    cluster_record_id: "record-cluster-b".to_string(),
                    generation: GenerationPin::new(
                        repo_id(),
                        revision_id(),
                        replacement_generation,
                    ),
                    expected_authority_digest: "auth:cluster-b".to_string(),
                    limit: 1,
                },
            ))?
            .outcomes
            .into_iter()
            .next()
            .ok_or_else(|| {
                CoreError::InvalidContract(
                    "single membership batch returned no outcome".to_string(),
                )
            })?;
        assert!(matches!(
            available,
            ClusterMembershipReadOutcomeV1::Available(snapshot)
                if snapshot.members == [SymbolId::new("symbol:c")]
                    && snapshot.completeness
                        == quanta_index_contract::ClusterMembershipCompletenessV1::Truncated
        ));
        let replaced = replacement_searcher
            .cluster_membership_batch_read(&ClusterMembershipBatchReadRequestV1::single_v1(
                ClusterMembershipReadRequestV1 {
                    cluster_record_id: "record-cluster-a".to_string(),
                    generation: GenerationPin::new(
                        repo_id(),
                        revision_id(),
                        replacement_generation,
                    ),
                    expected_authority_digest: "auth:cluster-a".to_string(),
                    limit: 2,
                },
            ))?
            .outcomes
            .into_iter()
            .next()
            .ok_or_else(|| {
                CoreError::InvalidContract(
                    "single membership batch returned no outcome".to_string(),
                )
            })?;
        assert!(matches!(
            replaced,
            ClusterMembershipReadOutcomeV1::Rejected(rejection)
                if rejection.failure
                    == ClusterMembershipReadFailureV1::CurrentGenerationMissing
        ));

        build_with(
            &adapter,
            &SemanticIngestBatch {
                repo_id: repo_id(),
                revision_id: revision_id(),
                generation: tombstone_generation,
                base_generation: Some(replacement_generation),
                manifest_digest: "manifest:73".to_string(),
                batch_digest: "batch:73".to_string(),
                mode: BatchIngestMode::Delta,
                model_contract: model_contract(),
                required_corpora: Vec::new(),
                corpus_policy_digest: None,
                clear_surfaces: Vec::new(),
                replace_scopes: Vec::new(),
                tombstone_scopes: vec![SemanticTombstoneScope {
                    scope: None,
                    semantic_scope: Some(cluster_scope),
                }],
                seal: true,
            },
        )?;
        let tombstoned = adapter
            .open(&repo_id(), &revision_id(), tombstone_generation)?
            .cluster_membership_batch_read(&ClusterMembershipBatchReadRequestV1::single_v1(
                ClusterMembershipReadRequestV1 {
                    cluster_record_id: "record-cluster-b".to_string(),
                    generation: GenerationPin::new(repo_id(), revision_id(), tombstone_generation),
                    expected_authority_digest: "auth:cluster-b".to_string(),
                    limit: 2,
                },
            ))?
            .outcomes
            .into_iter()
            .next()
            .ok_or_else(|| {
                CoreError::InvalidContract(
                    "single membership batch returned no outcome".to_string(),
                )
            })?;
        assert!(matches!(
            tombstoned,
            ClusterMembershipReadOutcomeV1::Rejected(rejection)
                if rejection.failure
                    == ClusterMembershipReadFailureV1::CurrentGenerationMissing
        ));

        drop(replacement_searcher);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let replacement_dir =
            layout::generation_dir(&root, &repo_id(), &revision_id(), replacement_generation);
        let connection = crate::run_blocking(
            &runtime,
            open_connection(&layout::dataset_dir(&replacement_dir)),
        )?;
        let membership_table = crate::run_blocking(
            &runtime,
            connection
                .open_table(CLUSTER_MEMBERSHIP_TABLE_NAME)
                .execute(),
        )?;
        let _delete_result = crate::run_blocking(
            &runtime,
            membership_table.delete("member_symbol_id = 'symbol:d'"),
        )?;
        assert!(
            crate::run_blocking(
                &runtime,
                open_generation(&root, &repo_id(), &revision_id(), replacement_generation,),
            )
            .is_err(),
            "cold open must reject a membership row deleted after seal"
        );

        let base_dir = layout::generation_dir(&root, &repo_id(), &revision_id(), base_generation);
        let base_connection =
            crate::run_blocking(&runtime, open_connection(&layout::dataset_dir(&base_dir)))?;
        crate::run_blocking(
            &runtime,
            base_connection.drop_table(CLUSTER_MEMBERSHIP_TABLE_NAME, &[]),
        )?;
        assert!(
            crate::run_blocking(
                &runtime,
                open_generation(&root, &repo_id(), &revision_id(), base_generation,),
            )
            .is_err(),
            "cold open must reject a deleted membership sidecar table"
        );
        Ok(())
    }

    /// A same-cardinality content mutation after the seal is a new dataset
    /// version on disk; the sealed manifest's file commitment refuses it at
    /// cold open (QI-BB-017) without re-deriving the row root.
    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts same-cardinality content tampering is rejected at cold open"
    )]
    fn sealed_manifest_rejects_same_row_count_content_mutation() -> TestResult {
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let generation = ManifestGeneration::new(701);
        build(
            &runtime,
            &root,
            &batch(
                generation,
                "src/root.rs",
                "emb-root",
                vec![1.0, 0.0, 0.0],
                true,
            )?,
        )?;
        let generation_dir = layout::generation_dir(&root, &repo_id(), &revision_id(), generation);
        let connection = crate::run_blocking(
            &runtime,
            open_connection(&layout::dataset_dir(&generation_dir)),
        )?;
        let table = crate::run_blocking(&runtime, connection.open_table(TABLE_NAME).execute())?;
        let update = crate::run_blocking(
            &runtime,
            table
                .update()
                .only_if("embedding_id = 'emb-root'")
                .column(layout::COLUMN_SNIPPET, "'fn tampered() {}'")
                .execute(),
        )?;
        assert_eq!(update.rows_updated, 1);

        let Err(error) = crate::run_blocking(
            &runtime,
            open_generation(&root, &repo_id(), &revision_id(), generation),
        ) else {
            return Err("same-row-count content mutation must fail closed".into());
        };
        assert!(
            matches!(
                error,
                CoreError::Typed { ref code, .. } if code == "GENERATION_SIDECAR_CORRUPT"
            ),
            "{error:?}"
        );
        Ok(())
    }
    // ---- QI-BB-021 follow-up #2: scope-streamed build ----

    /// `count` legacy path scopes of `chunks_per_scope` chunk owners each,
    /// every row a distinct unit direction in the fixture's 3-space.
    fn streamed_scopes(
        count: usize,
        chunks_per_scope: usize,
    ) -> Result<Vec<SemanticReplaceScope>, Box<dyn std::error::Error>> {
        let mut scopes = Vec::with_capacity(count);
        let mut step = 0_usize;
        for scope_index in 0..count {
            let path = format!("src/streamed_{scope_index:02}.rs");
            let mut embeddings = Vec::with_capacity(chunks_per_scope);
            for chunk_index in 0..chunks_per_scope {
                let id = format!("s{scope_index:02}c{chunk_index}");
                // 0.4 rad apart: distinct directions, all unit length.
                let angle = 0.4_f32 * f32::from(u16::try_from(step)?);
                let vector = vec![angle.cos(), angle.sin(), 0.0];
                step = step.saturating_add(1);
                embeddings.push(embedding(
                    &id,
                    &path,
                    OwnerDocKind::Chunk,
                    &format!("owner-{id}"),
                    SemanticCorpusKindV1::RawCodeFallback,
                    vector,
                )?);
            }
            scopes.push(SemanticReplaceScope {
                scope: scope(&path),
                scope_digest: format!("scope:{path}"),
                embeddings,
                cluster_memberships: Vec::new(),
            });
        }
        Ok(scopes)
    }

    fn streamed_batch(
        generation: ManifestGeneration,
        scopes: Vec<SemanticReplaceScope>,
        seal: bool,
    ) -> SemanticIngestBatch {
        SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: format!("manifest:{}", generation.get()),
            batch_digest: format!("batch:{}:streamed:{seal}", generation.get()),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(),
            required_corpora: vec![SemanticCorpusKindV1::RawCodeFallback],
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: scopes,
            tombstone_scopes: Vec::new(),
            seal,
        }
    }

    /// The canonical row commitment, recomputed from the fixture's own rows.
    ///
    /// The same leaf and root framing `semantic_row_integrity_v1` documents,
    /// computed from the records the test built rather than read from the
    /// table, so the sealed root equals it only if the streamed appends
    /// landed exactly those rows.
    fn independent_row_root(records: &[&EmbeddingRecord]) -> String {
        use sha2::{Digest as _, Sha256};
        fn bytes(hasher: &mut Sha256, value: &[u8]) {
            hasher.update(
                u64::try_from(value.len())
                    .map_or(u64::MAX, |len| len)
                    .to_le_bytes(),
            );
            hasher.update(value);
        }
        fn optional(hasher: &mut Sha256, value: Option<&str>) {
            match value {
                Some(value) => {
                    hasher.update([1]);
                    bytes(hasher, value.as_bytes());
                }
                None => hasher.update([0]),
            }
        }
        let mut leaves: Vec<(String, String, [u8; 32])> = records
            .iter()
            .map(|record| {
                let mut leaf = Sha256::new();
                leaf.update(b"quanta-index-semantic-row-leaf-v1\0");
                bytes(&mut leaf, record.record_id.as_bytes());
                bytes(&mut leaf, record.embedding_id.as_str().as_bytes());
                for required in [
                    record.repo_relative_path.as_str(),
                    record.owner_id.as_ref(),
                    record.owner_kind.as_code_str(),
                    record.corpus_kind.as_code_str(),
                    record.source_doc_id.as_ref(),
                    record.language.as_str(),
                    record.source_role.as_code_str(),
                    record.capability_status.as_code_str(),
                    record.authority_digest.as_ref(),
                    record.render_policy_digest.as_ref(),
                    record.embedding_input_digest.as_ref(),
                    record.vector_digest.as_ref(),
                    record.snippet.as_ref(),
                ] {
                    optional(&mut leaf, Some(required));
                }
                optional(&mut leaf, record.parent_owner_id.as_deref());
                optional(&mut leaf, record.package.as_deref());
                optional(
                    &mut leaf,
                    record
                        .symbol_kind
                        .as_ref()
                        .map(quanta_index_contract::lex::SymbolKindCode::as_str),
                );
                optional(&mut leaf, record.visibility.as_deref());
                leaf.update([u8::from(record.generated)]);
                leaf.update(record.card_schema_version.to_le_bytes());
                leaf.update(record.start_line.to_le_bytes());
                leaf.update(record.end_line.to_le_bytes());
                leaf.update(
                    u64::try_from(record.vector.len())
                        .map_or(u64::MAX, |len| len)
                        .to_le_bytes(),
                );
                for value in &record.vector {
                    leaf.update(value.to_bits().to_le_bytes());
                }
                (
                    record.record_id.to_string(),
                    record.embedding_id.as_str().to_string(),
                    leaf.finalize().into(),
                )
            })
            .collect();
        leaves.sort();
        let mut root = Sha256::new();
        root.update(b"quanta-index-semantic-row-root-v1\0");
        root.update(
            u64::try_from(leaves.len())
                .map_or(u64::MAX, |len| len)
                .to_le_bytes(),
        );
        for (_record_id, _embedding_id, leaf) in &leaves {
            root.update(leaf);
        }
        let digest = root.finalize();
        let mut hex = String::with_capacity(71);
        hex.push_str("sha256:");
        for byte in digest {
            use std::fmt::Write as _;
            let _written = write!(hex, "{byte:02x}");
        }
        hex
    }

    fn manifest_bytes_without_clock(
        root: &Path,
        generation: ManifestGeneration,
    ) -> Result<(SemanticManifest, Vec<u8>), Box<dyn std::error::Error>> {
        let generation_dir = layout::generation_dir(root, &repo_id(), &revision_id(), generation);
        let mut manifest =
            SemanticManifest::decode(&std::fs::read(layout::manifest_path(&generation_dir))?)?;
        manifest.built_at_unix_nanos = 0;
        let bytes = manifest.encode()?;
        Ok((manifest, bytes))
    }

    async fn promoted_row_count(
        root: &Path,
        generation: ManifestGeneration,
    ) -> Result<usize, CoreError> {
        let generation_dir = layout::generation_dir(root, &repo_id(), &revision_id(), generation);
        let connection = open_connection(&layout::dataset_dir(&generation_dir)).await?;
        let table = connection
            .open_table(TABLE_NAME)
            .execute()
            .await
            .map_err(|err| CoreError::Storage(format!("open promoted table: {err}")))?;
        table
            .count_rows(None)
            .await
            .map_err(|err| CoreError::Storage(format!("count promoted rows: {err}")))
    }

    // CASE-COVERS: a batch of N scopes streamed under a two-owner window
    // never has more than one window (two rows of vectors) resident — the
    // source's residency ledger is the instrument — and seals to a manifest
    // byte-identical (but for the clock) to the one the same batch seals to
    // in one window, whose row root equals a commitment recomputed here
    // from the fixture's own rows. Every row is then served.
    #[test]
    fn streamed_windows_stay_bounded_and_seal_to_the_all_at_once_manifest() -> TestResult {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let generation = ManifestGeneration::new(811);
        let scopes = streamed_scopes(7, 2)?;
        let rows: Vec<&EmbeddingRecord> = scopes
            .iter()
            .flat_map(|scope| scope.embeddings.iter())
            .collect();
        let batch = streamed_batch(generation, scopes.clone(), true);
        let header = SemanticIngestHeaderV1::of_batch(&batch);
        let two_owners = SemanticStreamWindowPolicy::new(2, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
        let two_rows = SemanticStreamWindowPolicy::vector_bytes(2, 3)?;

        // Streamed: seven windows of two chunk owners.
        let streamed_root = tempdir()?;
        let mut source = ResidentScopeSource::new(&batch.replace_scopes, two_owners)?;
        let residency = std::sync::Arc::clone(source.residency());
        let tally = build_stream(
            &runtime,
            streamed_root.path(),
            two_owners,
            &header,
            &mut source,
        )?;
        assert_eq!(tally, source.tally(), "sink and source tallies agree");
        assert_eq!(tally.windows, 7);
        assert_eq!(tally.rows, 14);
        assert_eq!(tally.peak_vector_bytes, two_rows);
        assert_eq!(
            residency.peak_windows(),
            1,
            "at most one window was resident at any time"
        );
        assert_eq!(
            residency.peak_vector_bytes(),
            two_rows,
            "at most two rows of vectors were resident at any time"
        );
        assert_eq!(residency.outstanding_windows(), 0);

        // All at once: the same batch in one window.
        let whole_root = tempdir()?;
        let mut whole =
            ResidentScopeSource::new(&batch.replace_scopes, SemanticStreamWindowPolicy::DEFAULT)?;
        let whole_tally = build_stream(
            &runtime,
            whole_root.path(),
            SemanticStreamWindowPolicy::DEFAULT,
            &header,
            &mut whole,
        )?;
        assert_eq!(whole_tally.windows, 1);
        assert_eq!(whole_tally.rows, 14);

        let (streamed_manifest, streamed_bytes) =
            manifest_bytes_without_clock(streamed_root.path(), generation)?;
        let (_whole_manifest, whole_bytes) =
            manifest_bytes_without_clock(whole_root.path(), generation)?;
        assert_eq!(
            streamed_bytes, whole_bytes,
            "the sealed manifest is byte-identical but for the clock"
        );
        assert_eq!(streamed_manifest.row_count, 14);
        assert_eq!(
            streamed_manifest.semantic_row_root_digest,
            independent_row_root(&rows),
            "the sealed row root is the commitment over exactly the fixture's rows"
        );

        // Every row is served from the streamed generation.
        let loaded = crate::run_blocking(
            &runtime,
            open_generation(streamed_root.path(), &repo_id(), &revision_id(), generation),
        )?;
        let hits = crate::run_blocking(
            &runtime,
            loaded.search_hits_filtered_async(&[1.0, 0.0, 0.0], 14, None),
        )?;
        let mut served: Vec<String> = hits.into_iter().map(|hit| hit.record_id).collect();
        served.sort();
        let mut expected: Vec<String> = rows.iter().map(|row| row.record_id.to_string()).collect();
        expected.sort();
        assert_eq!(served, expected, "every streamed row is served");
        Ok(())
    }

    // CASE-COVERS: a window that fails the model contract aborts the build
    // typed: the windows before it never reach the promoted dataset, nothing
    // is sealed, and the next build of the generation starts from the rows
    // that were promoted before, with the aborted staging copy discarded.
    #[test]
    fn a_refused_third_window_seals_nothing_and_leaves_no_partial_rows() -> TestResult {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let generation = ManifestGeneration::new(812);
        let generation_dir = layout::generation_dir(&root, &repo_id(), &revision_id(), generation);

        // Batch one: two owners, promoted unsealed.
        build(
            &runtime,
            &root,
            &streamed_batch(generation, streamed_scopes(2, 1)?, false),
        )?;
        assert_eq!(
            crate::run_blocking(&runtime, promoted_row_count(&root, generation))?,
            2
        );

        // Batch two: three more owners, the third breaking the L2Unit contract,
        // streamed one owner per window and sealing.
        let mut scopes = streamed_scopes(3, 1)?;
        for (index, replace_scope) in scopes.iter_mut().enumerate() {
            let path = format!("src/second_{index}.rs");
            replace_scope.scope = scope(&path);
            replace_scope.scope_digest = format!("scope:{path}");
            for record in &mut replace_scope.embeddings {
                record.embedding_id = EmbeddingId::new(format!("second-{index}"));
                record.record_id = format!("record-second-{index}").into_boxed_str();
                record.owner_id = format!("owner-second-{index}").into_boxed_str();
                record.repo_relative_path = RepoRelativePath::new(&path);
            }
        }
        scopes[2].embeddings[0].vector = vec![1.0, 1.0, 0.0];
        let sealing = streamed_batch(generation, scopes, true);
        let header = SemanticIngestHeaderV1::of_batch(&sealing);
        let one_owner = SemanticStreamWindowPolicy::new(1, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
        let mut source = ResidentScopeSource::new(&sealing.replace_scopes, one_owner)?;
        let Err(error) = build_stream(&runtime, &root, one_owner, &header, &mut source) else {
            return Err("a window breaking the contract must abort the build".into());
        };
        assert!(
            matches!(&error, CoreError::Typed { message, .. } if message.contains("second-2")),
            "the refusal names the offending embedding: {error:?}"
        );
        assert_eq!(
            source.tally().windows,
            3,
            "the third window was issued and refused"
        );
        assert!(
            !layout::sealed_marker_path(&generation_dir).exists()
                && !layout::manifest_path(&generation_dir).exists(),
            "nothing is sealed"
        );
        assert_eq!(
            crate::run_blocking(&runtime, promoted_row_count(&root, generation))?,
            2,
            "the promoted dataset holds only the rows promoted before"
        );
        assert!(
            generation_dir.join(STAGING_DIR_NAME).exists(),
            "the aborted staging copy is left for the next recovery"
        );

        // The next build of the generation starts from the promoted rows and
        // discards the aborted copy; a valid seal serves exactly the promoted
        // rows plus its own.
        let mut third = streamed_scopes(1, 1)?;
        let path = "src/third.rs";
        third[0].scope = scope(path);
        third[0].scope_digest = format!("scope:{path}");
        third[0].embeddings[0].embedding_id = EmbeddingId::new("third-0");
        third[0].embeddings[0].record_id = "record-third-0".to_string().into_boxed_str();
        third[0].embeddings[0].owner_id = "owner-third-0".to_string().into_boxed_str();
        third[0].embeddings[0].repo_relative_path = RepoRelativePath::new(path);
        build(&runtime, &root, &streamed_batch(generation, third, true))?;
        assert!(!generation_dir.join(STAGING_DIR_NAME).exists());
        let loaded = crate::run_blocking(
            &runtime,
            open_generation(&root, &repo_id(), &revision_id(), generation),
        )?;
        let hits = crate::run_blocking(
            &runtime,
            loaded.search_hits_filtered_async(&[1.0, 0.0, 0.0], 10, None),
        )?;
        let mut served: Vec<String> = hits.into_iter().map(|hit| hit.record_id).collect();
        served.sort();
        assert_eq!(
            served,
            vec![
                "record-s00c0".to_string(),
                "record-s01c0".to_string(),
                "record-third-0".to_string()
            ],
            "no row of the refused batch is visible"
        );
        Ok(())
    }
}
