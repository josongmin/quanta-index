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
use std::fs::{self};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use arrow_array::{
    Array, BooleanArray, FixedSizeListArray, Float32Array, RecordBatch, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field};
use futures::TryStreamExt as _;
use lancedb::connect;
use lancedb::query::{ExecutableQuery as _, QueryBase as _};
use quanta_index_contract::{
    IngestStageReport,
    EmbeddingDistanceMetric, EmbeddingNormalization, EmbeddingRecord, OwnerDocKind,
    SearchScopeSurface, SemanticCorpusKindV1, SemanticReplaceScope, SemanticTombstoneScope,
    canonical_order::first_canonical_order_break_v1, cluster_membership_content_digest_v1,
};
use quanta_index_core::domains::semantic::SemanticPolicy;
use quanta_index_core::{
    CoreError, SemanticIngestHeaderV1, SemanticScopeSource, SemanticScopeWindowV1,
    SemanticStreamTallyV1, SemanticStreamWindowPolicy, owner_key_v1,
};

use crate::durable_write::write_atomic;
use crate::errors::{arrow_err, fs_err, lancedb_err};
use crate::generation_contract::GenerationContract;
use crate::integrity::SealTalliesV1;
use crate::layout::{
    self, CLUSTER_MEMBERSHIP_TABLE_NAME, COLUMN_AUTHORITY_DIGEST, COLUMN_CARD_SCHEMA_VERSION,
    COLUMN_CORPUS_KIND, COLUMN_MEMBERSHIP_AUTHORITY_DIGEST, COLUMN_MEMBERSHIP_CLUSTER_RECORD_ID,
    COLUMN_MEMBERSHIP_CONTENT_DIGEST, COLUMN_MEMBERSHIP_MEMBER_COUNT,
    COLUMN_MEMBERSHIP_MEMBER_SYMBOL_ID, COLUMN_MEMBERSHIP_ORDINAL, COLUMN_MEMBERSHIP_OWNER_ID,
    COLUMN_MEMBERSHIP_OWNER_KIND, COLUMN_OWNER_ID, COLUMN_OWNER_KIND, COLUMN_RENDER_POLICY_DIGEST,
    COLUMN_VECTOR, TABLE_NAME, cluster_membership_schema, dimension_to_i32, semantic_schema,
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

/// Monotonic nanos elapsed since `started`, saturating at `u64::MAX`.
///
/// Observation only: a clock reading that does not fit `u64` saturates
/// instead of failing a build whose semantics do not depend on it.
#[expect(
    clippy::manual_unwrap_or,
    clippy::option_if_let_else,
    reason = "`Result::unwrap_or`/`map_or` are repo-disallowed silent-default shapes (clippy.toml); the explicit match keeps the saturation a visible, deliberate fallback"
)]
fn monotonic_nanos_since(started: Instant) -> u64 {
    match u64::try_from(started.elapsed().as_nanos()) {
        Ok(nanos) => nanos,
        Err(_) => u64::MAX,
    }
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

/// The scope authority of one streamed batch, checked window by window.
///
/// The header's clear surfaces and tombstones are known before the first
/// window and validated against each other up front; every replace scope
/// is then checked as it arrives against them and against every replace
/// scope before it, so a conflict is refused at the window that carries
/// it. The rules are the ones the all-at-once validation applied: a
/// surface is not cleared and replaced or tombstoned in one batch, an owner
/// scope is not replaced and tombstoned in one batch, and no
/// record id or owner scope is replaced twice.
struct StreamScopeAuthorityV1 {
    clear_surfaces: BTreeSet<SearchScopeSurface>,
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
        let mut tombstone_semantic_keys = BTreeSet::new();
        for tombstone in &header.mutations.tombstone_scopes {
            let scope = &tombstone.semantic_scope;
            if scope.owner_id.is_empty() {
                return Err(CoreError::InvalidContract(
                    "semantic: tombstone owner_id must not be empty".to_string(),
                ));
            }
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
        Ok(Self {
            clear_surfaces,
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

async fn delete_by_semantic_scope(
    table: &lancedb::Table,
    corpus_kind: &str,
    owner_kind: &str,
    owner_id: &str,
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    let predicate = format!(
        "{COLUMN_CORPUS_KIND} = {} AND {COLUMN_OWNER_KIND} = {} AND {COLUMN_OWNER_ID} = {}",
        crate::sql::quote_sql_string(corpus_kind),
        crate::sql::quote_sql_string(owner_kind),
        crate::sql::quote_sql_string(owner_id),
    );
    let delete_started = Instant::now();
    let _result = table
        .delete(predicate.as_str())
        .await
        .map_err(|err| lancedb_err(&format!("delete predicate `{predicate}`"), err))?;
    report.durations.semantic_delete = report
        .durations
        .semantic_delete
        .saturating_add(monotonic_nanos_since(delete_started));
    report.semantic_delete_commits = report.semantic_delete_commits.saturating_add(1);
    Ok(())
}

async fn delete_cluster_membership_by_owner(
    table: &lancedb::Table,
    owner_kind: &str,
    owner_id: &str,
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    let predicate = format!(
        "{COLUMN_MEMBERSHIP_OWNER_KIND} = {} AND {COLUMN_MEMBERSHIP_OWNER_ID} = {}",
        crate::sql::quote_sql_string(owner_kind),
        crate::sql::quote_sql_string(owner_id),
    );
    let delete_started = Instant::now();
    let _result = table
        .delete(predicate.as_str())
        .await
        .map_err(|err| lancedb_err(&format!("delete membership predicate `{predicate}`"), err))?;
    report.durations.membership_delete = report
        .durations
        .membership_delete
        .saturating_add(monotonic_nanos_since(delete_started));
    report.membership_delete_commits = report.membership_delete_commits.saturating_add(1);
    Ok(())
}

async fn delete_cluster_membership_for_surface(
    table: &lancedb::Table,
    surface: SearchScopeSurface,
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    report.membership_delete_calls = report.membership_delete_calls.saturating_add(1);
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
    let delete_started = Instant::now();
    let _result = table.delete(predicate.as_str()).await.map_err(|err| {
        lancedb_err(
            &format!("delete cluster membership surface predicate `{predicate}`"),
            err,
        )
    })?;
    report.durations.membership_delete = report
        .durations
        .membership_delete
        .saturating_add(monotonic_nanos_since(delete_started));
    report.membership_delete_commits = report.membership_delete_commits.saturating_add(1);
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
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    report.semantic_delete_calls = report.semantic_delete_calls.saturating_add(1);
    let predicate = semantic_surface_delete_predicate_v1(surface)?;
    let delete_started = Instant::now();
    let _result = table.delete(predicate.as_str()).await.map_err(|err| {
        lancedb_err(
            &format!("delete semantic surface {surface:?} predicate `{predicate}`"),
            err,
        )
    })?;
    report.durations.semantic_delete = report
        .durations
        .semantic_delete
        .saturating_add(monotonic_nanos_since(delete_started));
    report.semantic_delete_commits = report.semantic_delete_commits.saturating_add(1);
    Ok(())
}

async fn delete_replace_scope_rows(
    table: &lancedb::Table,
    scope: &SemanticReplaceScope,
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    report.semantic_delete_calls = report.semantic_delete_calls.saturating_add(1);
    for (corpus_kind, owner_kind, owner_id) in semantic_scopes_for_replace_scope(scope) {
        delete_by_semantic_scope(table, &corpus_kind, &owner_kind, &owner_id, report).await?;
    }
    Ok(())
}

async fn delete_tombstone_scope_rows(
    table: &lancedb::Table,
    scope: &SemanticTombstoneScope,
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    report.semantic_delete_calls = report.semantic_delete_calls.saturating_add(1);
    let semantic_scope = &scope.semantic_scope;
    delete_by_semantic_scope(
        table,
        semantic_scope.corpus_kind.as_code_str(),
        semantic_scope.owner_kind.as_code_str(),
        semantic_scope.owner_id.as_str(),
        report,
    )
    .await
}

async fn delete_replace_scope_memberships(
    table: &lancedb::Table,
    scope: &SemanticReplaceScope,
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    report.membership_delete_calls = report.membership_delete_calls.saturating_add(1);
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
        delete_cluster_membership_by_owner(table, owner_kind, owner_id, report).await?;
    }
    Ok(())
}

async fn delete_tombstone_memberships(
    table: &lancedb::Table,
    scope: &SemanticTombstoneScope,
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    report.membership_delete_calls = report.membership_delete_calls.saturating_add(1);
    let semantic_scope = &scope.semantic_scope;
    if semantic_scope.corpus_kind != SemanticCorpusKindV1::ClusterCard {
        return Ok(());
    }
    delete_cluster_membership_by_owner(
        table,
        semantic_scope.owner_kind.as_code_str(),
        semantic_scope.owner_id.as_str(),
        report,
    )
    .await
}

/// Append every row of `window` as one batch, after its owners' rows are
/// gone.
async fn append_window(
    table: &lancedb::Table,
    window: &SemanticScopeWindowV1,
    dimension: usize,
    report: &mut IngestStageReport,
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
    let append_started = Instant::now();
    let _result = table
        .add(batch)
        .execute()
        .await
        .map_err(|err| lancedb_err("table.add", err))?;
    report.durations.semantic_append = report
        .durations
        .semantic_append
        .saturating_add(monotonic_nanos_since(append_started));
    report.semantic_append_calls = report.semantic_append_calls.saturating_add(1);
    Ok(())
}

async fn append_cluster_membership_scope(
    table: &lancedb::Table,
    scope: &SemanticReplaceScope,
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    let Some(batch) = build_cluster_membership_record_batch(scope)? else {
        return Ok(());
    };
    let append_started = Instant::now();
    let _result = table
        .add(batch)
        .execute()
        .await
        .map_err(|err| lancedb_err("cluster membership table.add", err))?;
    report.durations.membership_append = report
        .durations
        .membership_append
        .saturating_add(monotonic_nanos_since(append_started));
    report.membership_append_calls = report.membership_append_calls.saturating_add(1);
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
    Ok(Some(manifest.vector_index))
}

/// The base generation directory a delta seal inherits digests from, if
/// the generation has a base.
fn delta_base_dir_v1(
    semantic_root: &Path,
    header: &SemanticIngestHeaderV1,
    generation_contract: &GenerationContract,
) -> Option<PathBuf> {
    generation_contract.base_generation.map(|base_generation| {
        layout::generation_dir(
            semantic_root,
            &header.pin.repo_id,
            &header.pin.revision_id,
            base_generation,
        )
    })
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
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    for surface in surfaces {
        delete_surface_rows(&tables.table, *surface, report).await?;
        delete_cluster_membership_for_surface(&tables.membership_table, *surface, report).await?;
    }
    Ok(())
}

/// Replace one window: every scope's owners are deleted, then the window's
/// rows are appended as one batch and its memberships per scope.
async fn apply_window(
    tables: &WorkingTables,
    window: &SemanticScopeWindowV1,
    dimension: usize,
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    for scope in window.scopes() {
        delete_replace_scope_rows(&tables.table, scope, report).await?;
        delete_replace_scope_memberships(&tables.membership_table, scope, report).await?;
    }
    append_window(&tables.table, window, dimension, report).await?;
    for scope in window.scopes() {
        append_cluster_membership_scope(&tables.membership_table, scope, report).await?;
    }
    Ok(())
}

async fn apply_tombstones(
    tables: &WorkingTables,
    tombstones: &[SemanticTombstoneScope],
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    for scope in tombstones {
        delete_tombstone_scope_rows(&tables.table, scope, report).await?;
        delete_tombstone_memberships(&tables.membership_table, scope, report).await?;
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
/// crate's async seam. Counts windows and owner scopes into `report` as
/// they are admitted (observation only).
fn apply_scope_stream(
    runtime: &tokio::runtime::Runtime,
    tables: &WorkingTables,
    policy: SemanticStreamWindowPolicy,
    header: &SemanticIngestHeaderV1,
    authority: &mut StreamScopeAuthorityV1,
    scopes: &mut dyn SemanticScopeSource,
    report: &mut IngestStageReport,
) -> Result<SemanticStreamTallyV1, CoreError> {
    let dimension = header.dimension()?;
    let normalization = header.contract.model_contract.normalization;
    let mut tally = SemanticStreamTallyV1::default();
    while let Some(window) = scopes.next_window()? {
        let _fill = policy.admit(&window)?;
        report.windows = report.windows.saturating_add(1);
        for scope in window.scopes() {
            validate_replace_scope(scope, dimension, normalization)?;
            authority.admit_replace_scope(scope)?;
            report.owner_scopes = report.owner_scopes.saturating_add(1);
        }
        let rows = window.rows()?;
        crate::run_blocking(runtime, apply_window(tables, &window, dimension, report))?;
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
#[cfg(test)]
pub(crate) fn build_stream(
    runtime: &tokio::runtime::Runtime,
    semantic_root: &Path,
    policy: SemanticStreamWindowPolicy,
    header: &SemanticIngestHeaderV1,
    scopes: &mut dyn SemanticScopeSource,
    seal_tallies: &SealTalliesV1,
) -> Result<SemanticStreamTallyV1, CoreError> {
    let (tally, _stage_report) =
        build_stream_reported(runtime, semantic_root, policy, header, scopes, seal_tallies)?;
    Ok(tally)
}

/// [`build_stream`] with the ingest-stage accounting handed back instead of
/// discarded (RBR-10 step 1).
///
/// The [`IngestStageReport`] is observation only: the durability sequence
/// below is exactly the one [`build_stream`] runs, un-reordered, with every
/// seal/promotion step intact. Callers that do not want the report keep
/// using [`build_stream`] unchanged.
pub(crate) fn build_stream_reported(
    runtime: &tokio::runtime::Runtime,
    semantic_root: &Path,
    policy: SemanticStreamWindowPolicy,
    header: &SemanticIngestHeaderV1,
    scopes: &mut dyn SemanticScopeSource,
    seal_tallies: &SealTalliesV1,
) -> Result<(SemanticStreamTallyV1, IngestStageReport), CoreError> {
    let build_started = Instant::now();
    let mut report = IngestStageReport::default();
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
        report.durations.prepare = monotonic_nanos_since(build_started);
        let clear_started = Instant::now();
        crate::run_blocking(
            runtime,
            apply_clear_surfaces(&tables, &header.mutations.clear_surfaces, &mut report),
        )?;
        report.durations.clear_surfaces = monotonic_nanos_since(clear_started);
        let stream_started = Instant::now();
        let tally = apply_scope_stream(
            runtime,
            &tables,
            policy,
            header,
            &mut authority,
            scopes,
            &mut report,
        )?;
        report.durations.stream = monotonic_nanos_since(stream_started);
        let tombstones_started = Instant::now();
        crate::run_blocking(
            runtime,
            apply_tombstones(&tables, &header.mutations.tombstone_scopes, &mut report),
        )?;
        report.durations.tombstones = monotonic_nanos_since(tombstones_started);
        let manifest_bytes = if header.batch.seal {
            let seal_started = Instant::now();
            let sealed = crate::run_blocking(
                runtime,
                seal_manifest_bytes(semantic_root, &tables, header, &generation_contract),
            )?;
            report.durations.seal = Some(monotonic_nanos_since(seal_started));
            Some(sealed)
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
    let promotion_started = Instant::now();
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
        // The file commitment every door checks the layout of and the scrub
        // proves the bytes of (QI-BB-017): measured after the dataset, the
        // contract and the scope manifest are durable, written before the
        // marker that makes the generation sealed. A delta inherits its
        // base's digests for every file it still shares by inode, so the
        // bytes hashed here are proportional to what this generation added
        // (QI-BB-006 #4); the measurement is tallied for the scrape.
        let base_dir = delta_base_dir_v1(semantic_root, header, &generation_contract);
        let (sealed_manifest_bytes, measurement) = build_sealed_manifest_bytes(
            &generation_dir,
            header.batch.manifest_digest.as_str(),
            base_dir.as_deref(),
        )?;
        seal_tallies.record(measurement);
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
    report.durations.promotion = monotonic_nanos_since(promotion_started);
    report.durations.total = monotonic_nanos_since(build_started);
    Ok((tally, report))
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
mod tests;
