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

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use arrow_array::{
    Array, BooleanArray, FixedSizeListArray, Float32Array, RecordBatch, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field, SchemaRef};
use futures::TryStreamExt as _;
use lance::Dataset;
use lance::dataset::{WriteParams, builder::DatasetBuilder};
use lance::session::Session;
use quanta_index_contract::{
    EmbeddingDistanceMetric, EmbeddingNormalization, EmbeddingRecord, GenerationPin,
    IngestStageReport, OwnerDocKind, SearchScopeSurface, SemanticCorpusKindV1,
    SemanticReplaceScope, SemanticTombstoneScope, SourceFileKey,
    canonical_order::first_canonical_order_break_v1, cluster_membership_content_digest_v1,
};
use quanta_index_core::domains::semantic::SemanticPolicy;
use quanta_index_core::publication_admission::{
    PublicationObjectLifetimePort, PublicationOutputAdmission,
};
use quanta_index_core::{
    CoreError, DiagnosticContextV1, DiagnosticCounterV1, DiagnosticSpanGuardV1, DiagnosticStageV1,
    SemanticGenerationContractV1, SemanticIngestHeaderV1, SemanticScopeSource,
    SemanticScopeWindowV1, SemanticStreamTallyV1, SemanticStreamWindowPolicy,
};

use crate::SemanticGenerationFileIo;
use crate::build_diagnostic::{begin_build_stage, finish_build_stage};
use crate::dataset_promotion::DatasetPromotion;
use crate::durable_write::{AtomicOutput, with_cleanup_context, write_atomic};
use crate::errors::{admitted_fs_err, arrow_err, fs_err, lancedb_err};
use crate::generation_contract::GenerationContract;
use crate::layout::{
    self, CLUSTER_MEMBERSHIP_TABLE_NAME, COLUMN_AUTHORITY_DIGEST, COLUMN_CORPUS_KIND,
    COLUMN_MEMBERSHIP_AUTHORITY_DIGEST, COLUMN_MEMBERSHIP_CLUSTER_RECORD_ID,
    COLUMN_MEMBERSHIP_CONTENT_DIGEST, COLUMN_MEMBERSHIP_MEMBER_COUNT,
    COLUMN_MEMBERSHIP_MEMBER_SYMBOL_ID, COLUMN_MEMBERSHIP_ORDINAL, COLUMN_MEMBERSHIP_OWNER_ID,
    COLUMN_MEMBERSHIP_OWNER_KIND, COLUMN_MEMBERSHIP_REPO_RELATIVE_PATH,
    COLUMN_MEMBERSHIP_SOURCE_REPO_ID, COLUMN_OWNER_ID, COLUMN_OWNER_KIND, COLUMN_VECTOR,
    TABLE_NAME, cluster_membership_schema, dimension_to_i32, semantic_schema,
};
use crate::manifest::{
    ClusterMembershipSealV1, SemanticCorpusCoverageV1, SemanticManifest, SemanticRowSealV1,
};
use crate::membership_integrity::{
    ClusterMembershipCommitmentV1, ClusterMembershipStoredRowV1, cluster_membership_commitment_v1,
};
use crate::seal_tallies::SealTalliesV1;
use crate::sealed_manifest::{build_sealed_manifest_bytes_with_lifetime, sealed_manifest_path};
use crate::semantic_row_integrity_v1::dataset_row_commitment_v1;
use crate::vector_index::{
    VectorIndexSealInputV1, inherited_index_contracted_v1, seal_vector_index_v1,
};

/// Sibling-of-`dataset/` staging dir used to make delta-base cloning
/// crash-atomic — see `prepare_generation_dir`.
const STAGING_DIR_NAME: &str = crate::legacy::layout::DATASET_STAGING_DIR_NAME;
#[cfg(test)]
const BACKUP_DIR_NAME: &str = "dataset.backup";

// Private backend SQL allocation ceiling, independent of vector residency.
// Split exact-owner deletes before either backend limit; a single oversized
// owner remains a typed refusal.
const MAX_DELETE_PREDICATE_BYTES: usize = 32 * 1024 * 1024;
// Lance 7 visits a left-associated OR expression recursively with MAX_DEPTH
// 500. Each exact-owner clause has five comparisons; 64 clauses leave a wide
// margin for their AND tree and the parser's own recursion.
const MAX_DELETE_OWNER_TUPLES: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkingDatasetOriginV1 {
    // This origin stays fixed for the build. Later windows may append rows,
    // but StreamScopeAuthorityV1 forbids replacing the same full owner key.
    FreshEmpty,
    TargetContinuation,
    SealedBaseClone,
}

impl WorkingDatasetOriginV1 {
    const fn is_inherited(self) -> bool {
        !matches!(self, Self::FreshEmpty)
    }
}

struct PreparedWorkingDatasetV1 {
    path: PathBuf,
    origin: WorkingDatasetOriginV1,
}

fn build_counter(
    diagnostic: Option<&DiagnosticContextV1>,
    span: Option<&DiagnosticSpanGuardV1>,
    key: DiagnosticCounterV1,
    value: u64,
) {
    if let (Some(context), Some(span)) = (diagnostic, span) {
        context.counter(span, key, value);
    }
}

/// Rows and vector bytes acknowledged by the semantic table's staging add.
/// Promotion may still fail.
#[derive(Default)]
struct AppendedVectors {
    rows: u64,
    bytes: u64,
}

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
fn inherit_dataset_tree(
    src: &Path,
    dst: &Path,
    generation_io: Option<&dyn SemanticGenerationFileIo>,
) -> Result<(), CoreError> {
    if let Some(io) = generation_io {
        io.prepare_staging_directory(dst)
            .map_err(|error| admitted_fs_err("prepare inherited dataset dir", error))?;
    } else {
        fs::create_dir_all(dst).map_err(|err| fs_err("create inherited dataset dir", dst, &err))?;
    }
    for entry in fs::read_dir(src).map_err(|err| fs_err("read source dataset dir", src, &err))? {
        let entry = entry.map_err(|err| fs_err("read source dataset entry", src, &err))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|err| fs_err("inherited dataset entry type", &from, &err))?;
        if file_type.is_dir() {
            inherit_dataset_tree(&from, &to, generation_io)?;
        } else if is_dataset_local_entry(&entry.file_name()) {
            if let Some(io) = generation_io {
                io.copy_inherited_file(&from, &to)
                    .map_err(|error| admitted_fs_err("copy generation-local entry", error))?;
            } else {
                let _copied = fs::copy(&from, &to)
                    .map_err(|error| fs_err("copy generation-local entry", &from, &error))?;
            }
        } else if let Some(io) = generation_io {
            io.link_inherited_file(&from, &to)
                .map_err(|error| admitted_fs_err("link inherited dataset entry", error))?;
        } else {
            fs::hard_link(&from, &to)
                .map_err(|error| fs_err("link inherited dataset entry", &from, &error))?;
        }
    }
    Ok(())
}

/// Remove `dir` and its contents if it exists, tagging IO failure with `action`.
fn remove_dir_if_exists(
    action: &str,
    dir: &Path,
    generation_io: Option<&dyn SemanticGenerationFileIo>,
) -> Result<(), CoreError> {
    if dir.exists() {
        match generation_io {
            Some(io) => io
                .remove_directory_and_sync(dir)
                .map_err(|error| admitted_fs_err("remove dataset tree", error))?,
            None => fs::remove_dir_all(dir).map_err(|err| fs_err(action, dir, &err))?,
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

async fn ensure_working_dataset(
    working_dataset: &Path,
    name: &str,
    schema: SchemaRef,
    session: Arc<Session>,
    mode: crate::qualified_fs::LanceUriMode,
) -> Result<Dataset, CoreError> {
    let path = working_dataset.join(format!("{name}.lance"));
    let uri = crate::qualified_fs::lance_connection_uri(&path, mode)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => DatasetBuilder::from_uri(&uri)
            .with_session(session)
            .load()
            .await
            .map_err(|error| lancedb_err(&format!("open_table {name}"), error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let empty = RecordBatch::new_empty(Arc::clone(&schema));
            let rows = arrow_array::RecordBatchIterator::new(std::iter::once(Ok(empty)), schema);
            let params = WriteParams {
                session: Some(session),
                ..WriteParams::default()
            };
            Dataset::write(rows, uri.as_str(), Some(params))
                .await
                .map_err(|error| lancedb_err(&format!("create_empty_table {name}"), error))
        }
        Ok(_) => Err(CoreError::Storage(format!(
            "semantic: {name} dataset path is not a directory: {}",
            path.display()
        ))),
        Err(error) => Err(fs_err("inspect working dataset", &path, &error)),
    }
}

fn verify_membership_schema(table: &Dataset) -> Result<(), CoreError> {
    let schema = arrow_schema::Schema::from(table.schema());
    for expected in cluster_membership_schema().fields() {
        let live = schema.field_with_name(expected.name()).map_err(|error| {
            CoreError::Storage(format!(
                "semantic: cluster membership table missing column `{}`: {error}",
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
    Ok(())
}

fn verify_table_dimension(table: &Dataset, expected_dimension: usize) -> Result<(), CoreError> {
    let schema = arrow_schema::Schema::from(table.schema());
    let expected = semantic_schema(dimension_to_i32(expected_dimension)?);
    for expected_field in expected.fields() {
        let live = schema.field_with_name(expected_field.name()).map_err(|_error| {
            CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
                message: format!(
                    "semantic: existing generation lacks required column `{}`; rebuild from producer",
                    expected_field.name()
                ),
            }
        })?;
        if live.data_type() != expected_field.data_type()
            || live.is_nullable() != expected_field.is_nullable()
        {
            return Err(CoreError::InvalidContract(format!(
                "semantic: existing generation column `{}` has incompatible schema",
                expected_field.name()
            )));
        }
    }
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
    scope.scope.source_file.validate().map_err(|error| {
        CoreError::InvalidContract(format!("semantic: invalid replace source file: {error}"))
    })?;
    for embedding in &scope.embeddings {
        if embedding.source_file != scope.scope.source_file {
            return Err(CoreError::InvalidContract(format!(
                "semantic: embedding {} source file differs from replace scope",
                embedding.embedding_id.as_str()
            )));
        }
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
            | CoreError::Storage(_)
            | CoreError::StorageIo { .. }) => other,
        })?;
    }
    let mut embeddings_by_record_id = BTreeMap::new();
    if !scope.cluster_memberships.is_empty() {
        for embedding in &scope.embeddings {
            let _existing = embeddings_by_record_id
                .entry(embedding.record_id.as_ref())
                .or_insert(embedding);
        }
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
        let embedding = embeddings_by_record_id
            .get(membership.cluster_record_id.as_str())
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

/// Proof-only, storage-free admission of all resident windows before creating
/// an external diagnostic state directory. Uses the production validators.
#[cfg(feature = "proof")]
pub(crate) fn proof_validate_resident_batch_v1(
    batch: &quanta_index_contract::SemanticIngestBatch,
    policy: SemanticStreamWindowPolicy,
) -> Result<(), CoreError> {
    let header = SemanticIngestHeaderV1::of_batch(batch);
    GenerationContract::validate_batch_shape(&header.contract)?;
    if header.contract.model_contract.distance_metric != EmbeddingDistanceMetric::Cosine {
        return Err(CoreError::InvalidContract(
            "proof requires cosine backend".to_owned(),
        ));
    }
    let dimension = header.dimension()?;
    preflight_tombstone_delete_predicates(&header.mutations.tombstone_scopes, policy)?;
    let mut authority = StreamScopeAuthorityV1::new(&header)?;
    let mut source = quanta_index_core::ResidentScopeSource::new(&batch.replace_scopes, policy)?;
    while let Some(window) = source.next_window()? {
        let _fill = policy.admit(&window)?;
        preflight_window_delete_predicates(&window)?;
        for scope in window.scopes() {
            validate_replace_scope(scope, dimension, batch.model_contract.normalization)?;
            authority.admit_replace_scope(scope)?;
        }
    }
    Ok(())
}

fn semantic_scope_key_v1(
    corpus_kind: &str,
    owner_kind: &str,
    owner_id: &str,
    source_file: &SourceFileKey,
) -> (String, String, String, SourceFileKey) {
    (
        corpus_kind.to_owned(),
        owner_kind.to_owned(),
        owner_id.to_owned(),
        source_file.clone(),
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
/// scope is not replaced and tombstoned in one batch, and no
/// record id or owner scope is replaced twice.
struct StreamScopeAuthorityV1 {
    clear_surfaces: BTreeSet<SearchScopeSurface>,
    tombstone_semantic_keys: BTreeSet<(String, String, String, SourceFileKey)>,
    replace_semantic_keys: BTreeSet<(String, String, String, SourceFileKey)>,
    record_ids: BTreeSet<String>,
    embedding_ids: BTreeSet<String>,
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
            scope.source_file.validate().map_err(|error| {
                CoreError::InvalidContract(format!(
                    "semantic: invalid tombstone source file: {error}"
                ))
            })?;
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
                &scope.source_file,
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
            embedding_ids: BTreeSet::new(),
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
            if !self
                .embedding_ids
                .insert(embedding.embedding_id.as_str().to_owned())
            {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: duplicate embedding_id {:?} in generation batch",
                    embedding.embedding_id
                )));
            }
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
            let _scope_key_was_new = scope_semantic_keys.insert(semantic_scope_key_v1(
                embedding.corpus_kind.as_code_str(),
                embedding.owner_kind.as_code_str(),
                embedding.owner_id.as_ref(),
                &embedding.source_file,
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

async fn validate_cluster_membership_coverage_v1(
    semantic_table: &Dataset,
    membership_table: &Dataset,
) -> Result<(), CoreError> {
    let cluster_predicate = format!(
        "{COLUMN_CORPUS_KIND} = {}",
        crate::sql::quote_sql_string(SemanticCorpusKindV1::ClusterCard.as_code_str())
    );
    let mut semantic_scan = semantic_table.scan();
    let mut semantic_stream = semantic_scan
        .filter(&cluster_predicate)
        .map_err(|err| lancedb_err("query ClusterCard coverage", err))?
        .batch_size(1024)
        .strict_batch_size(true)
        .try_into_stream()
        .await
        .map_err(|err| lancedb_err("query ClusterCard coverage", err))?;
    let mut expected = BTreeSet::new();
    while let Some(batch) = semantic_stream
        .try_next()
        .await
        .map_err(|err| lancedb_err("stream ClusterCard coverage", err))?
    {
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
    let mut membership_stream = membership_table
        .scan()
        .batch_size(1024)
        .strict_batch_size(true)
        .try_into_stream()
        .await
        .map_err(|err| lancedb_err("query cluster membership coverage", err))?;
    let mut observed = BTreeSet::new();
    while let Some(batch) = membership_stream
        .try_next()
        .await
        .map_err(|err| lancedb_err("stream cluster membership coverage", err))?
    {
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
    membership_table: &Dataset,
) -> Result<ClusterMembershipCommitmentV1, CoreError> {
    let row_count = membership_table
        .count_rows(None)
        .await
        .map_err(|error| lancedb_err("count cluster membership rows", error))?;
    let mut rows = Vec::with_capacity(row_count);
    let mut stream = membership_table
        .scan()
        .batch_size(1024)
        .strict_batch_size(true)
        .try_into_stream()
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
        let source_repos =
            column_as::<StringArray>(&batch, COLUMN_MEMBERSHIP_SOURCE_REPO_ID, "Utf8")?;
        let source_paths =
            column_as::<StringArray>(&batch, COLUMN_MEMBERSHIP_REPO_RELATIVE_PATH, "Utf8")?;
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
                source_repo_id: source_repos.value(row).to_owned(),
                repo_relative_path: source_paths.value(row).to_owned(),
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
            string_column(rows, |embedding| {
                embedding.source_file.source_repo_id.as_str()
            }),
            string_column(rows, |embedding| {
                embedding.source_file.repo_relative_path.as_str()
            }),
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
    let mut source_repos = Vec::with_capacity(row_count);
    let mut source_paths = Vec::with_capacity(row_count);
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
            source_repos.push(embedding.source_file.source_repo_id.as_str().to_string());
            source_paths.push(
                embedding
                    .source_file
                    .repo_relative_path
                    .as_str()
                    .to_string(),
            );
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
            Arc::new(StringArray::from(source_repos)),
            Arc::new(StringArray::from(source_paths)),
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
) -> SemanticScopeTuple<'_> {
    (
        embedding.source_file.source_repo_id.as_str(),
        embedding.source_file.repo_relative_path.as_str(),
        embedding.corpus_kind.as_code_str(),
        embedding.owner_kind.as_code_str(),
        embedding.owner_id.as_ref(),
    )
}

type SemanticScopeTuple<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);

fn predicate_budget_error() -> CoreError {
    CoreError::InvalidContract(format!(
        "semantic: exact-owner delete predicate exceeds {MAX_DELETE_PREDICATE_BYTES} UTF-8 bytes"
    ))
}

fn add_predicate_bytes(left: usize, right: usize) -> Result<usize, CoreError> {
    left.checked_add(right)
        .filter(|total| *total <= MAX_DELETE_PREDICATE_BYTES)
        .ok_or_else(predicate_budget_error)
}

fn literal_bytes(value: &str) -> Result<usize, CoreError> {
    crate::sql::quoted_sql_string_bytes(value).ok_or_else(predicate_budget_error)
}

/// Size both exact predicates before any escaping/String allocation or delete.
/// Owner IDs remain borrowed; the bounded group map holds only byte counts.
fn delete_predicate_bytes(
    scopes: &BTreeSet<SemanticScopeTuple<'_>>,
) -> Result<(usize, usize), CoreError> {
    let mut groups = BTreeMap::<(&str, &str, &str, &str), usize>::new();
    let mut membership_groups = BTreeMap::<(&str, &str, &str), usize>::new();
    for (repo, path, corpus, kind, id) in scopes {
        let bytes = literal_bytes(id)?;
        let group = groups.entry((*repo, *path, *corpus, *kind)).or_default();
        let separator = if *group == 0 { 0 } else { 2 };
        *group = add_predicate_bytes(*group, add_predicate_bytes(bytes, separator)?)?;
        if *corpus == SemanticCorpusKindV1::ClusterCard.as_code_str() {
            let group = membership_groups.entry((*repo, *path, *kind)).or_default();
            let separator = if *group == 0 { 0 } else { 2 };
            *group = add_predicate_bytes(*group, add_predicate_bytes(bytes, separator)?)?;
        }
    }
    let mut semantic = 0;
    for ((repo, path, corpus, kind), ids) in groups {
        let syntax = format!(
            "({} =  AND {} =  AND {COLUMN_CORPUS_KIND} =  AND {COLUMN_OWNER_KIND} =  AND {COLUMN_OWNER_ID} IN ())",
            crate::layout::COLUMN_SOURCE_REPO_ID,
            crate::layout::COLUMN_REPO_RELATIVE_PATH,
        )
        .len();
        let group = add_predicate_bytes(syntax, literal_bytes(repo)?)?;
        let group = add_predicate_bytes(group, literal_bytes(path)?)?;
        let group = add_predicate_bytes(group, literal_bytes(corpus)?)?;
        let group = add_predicate_bytes(group, literal_bytes(kind)?)?;
        let group = add_predicate_bytes(group, ids)?;
        semantic = add_predicate_bytes(semantic, if semantic == 0 { 0 } else { 4 })?;
        semantic = add_predicate_bytes(semantic, group)?;
    }
    let mut membership = 0;
    for ((repo, path, kind), ids) in membership_groups {
        let syntax = format!(
            "({COLUMN_MEMBERSHIP_SOURCE_REPO_ID} =  AND {COLUMN_MEMBERSHIP_REPO_RELATIVE_PATH} =  AND {COLUMN_MEMBERSHIP_OWNER_KIND} =  AND {COLUMN_MEMBERSHIP_OWNER_ID} IN ())"
        ).len();
        let group = add_predicate_bytes(syntax, literal_bytes(repo)?)?;
        let group = add_predicate_bytes(group, literal_bytes(path)?)?;
        let group = add_predicate_bytes(group, literal_bytes(kind)?)?;
        let group = add_predicate_bytes(group, ids)?;
        membership = add_predicate_bytes(membership, if membership == 0 { 0 } else { 4 })?;
        membership = add_predicate_bytes(membership, group)?;
    }
    Ok((semantic, membership))
}

/// Partition one logical delete union without changing any source/owner key.
/// The same planner runs before storage preparation and before native deletes.
fn plan_delete_scope_batches<'a>(
    scopes: &BTreeSet<SemanticScopeTuple<'a>>,
) -> Result<Vec<BTreeSet<SemanticScopeTuple<'a>>>, CoreError> {
    let mut batches = Vec::new();
    let mut current = BTreeSet::new();
    for tuple in scopes.iter().copied() {
        if current.len() == MAX_DELETE_OWNER_TUPLES {
            batches.push(std::mem::take(&mut current));
        }
        let _inserted = current.insert(tuple);
        if let Err(error) = delete_predicate_bytes(&current) {
            let _removed = current.remove(&tuple);
            if current.is_empty() {
                return Err(error);
            }
            batches.push(std::mem::take(&mut current));
            let _inserted = current.insert(tuple);
            let _sizes = delete_predicate_bytes(&current)?;
        }
    }
    if !current.is_empty() {
        batches.push(current);
    }
    Ok(batches)
}

fn preflight_window_delete_predicates(window: &SemanticScopeWindowV1) -> Result<(), CoreError> {
    let scopes = window
        .scopes()
        .iter()
        .flat_map(|scope| &scope.embeddings)
        .map(semantic_scope_tuple_from_embedding)
        .collect();
    let _batches = plan_delete_scope_batches(&scopes)?;
    Ok(())
}

fn tombstone_tuples(chunk: &[SemanticTombstoneScope]) -> BTreeSet<SemanticScopeTuple<'_>> {
    chunk
        .iter()
        .map(|scope| {
            let owner = &scope.semantic_scope;
            (
                owner.source_file.source_repo_id.as_str(),
                owner.source_file.repo_relative_path.as_str(),
                owner.corpus_kind.as_code_str(),
                owner.owner_kind.as_code_str(),
                owner.owner_id.as_str(),
            )
        })
        .collect()
}

fn preflight_tombstone_delete_predicates(
    tombstones: &[SemanticTombstoneScope],
    policy: SemanticStreamWindowPolicy,
) -> Result<(), CoreError> {
    for chunk in tombstones.chunks(policy.max_owner_scopes()) {
        let _batches = plan_delete_scope_batches(&tombstone_tuples(chunk))?;
    }
    Ok(())
}

/// Preserve exact tuples: independently combining corpus/kind/ID lists would
/// delete the cross product, including owners absent from this mutation.
fn semantic_scope_delete_predicate(scopes: &BTreeSet<SemanticScopeTuple<'_>>) -> Option<String> {
    let mut groups: BTreeMap<(&str, &str, &str, &str), Vec<String>> = BTreeMap::new();
    for (repo, path, corpus, kind, id) in scopes {
        groups
            .entry((*repo, *path, *corpus, *kind))
            .or_default()
            .push(crate::sql::quote_sql_string(id));
    }
    let clauses: Vec<String> = groups.into_iter().map(|((repo, path, corpus, kind), ids)| {
        format!("({} = {} AND {} = {} AND {COLUMN_CORPUS_KIND} = {} AND {COLUMN_OWNER_KIND} = {} AND {COLUMN_OWNER_ID} IN ({}))",
            crate::layout::COLUMN_SOURCE_REPO_ID,
            crate::sql::quote_sql_string(repo),
            crate::layout::COLUMN_REPO_RELATIVE_PATH,
            crate::sql::quote_sql_string(path),
            crate::sql::quote_sql_string(corpus), crate::sql::quote_sql_string(kind), ids.join(", "))
    }).collect();
    (!clauses.is_empty()).then(|| clauses.join(" OR "))
}

fn membership_scope_delete_predicate(scopes: &BTreeSet<SemanticScopeTuple<'_>>) -> Option<String> {
    let mut groups: BTreeMap<(&str, &str, &str), Vec<String>> = BTreeMap::new();
    for (repo, path, corpus, kind, id) in scopes {
        if *corpus == SemanticCorpusKindV1::ClusterCard.as_code_str() {
            groups
                .entry((*repo, *path, *kind))
                .or_default()
                .push(crate::sql::quote_sql_string(id));
        }
    }
    let clauses: Vec<String> = groups
        .into_iter()
        .map(|((repo, path, kind), ids)| {
            format!(
                "({COLUMN_MEMBERSHIP_SOURCE_REPO_ID} = {} AND {COLUMN_MEMBERSHIP_REPO_RELATIVE_PATH} = {} AND {COLUMN_MEMBERSHIP_OWNER_KIND} = {} AND {COLUMN_MEMBERSHIP_OWNER_ID} IN ({}))",
                crate::sql::quote_sql_string(repo),
                crate::sql::quote_sql_string(path),
                crate::sql::quote_sql_string(kind),
                ids.join(", ")
            )
        })
        .collect();
    (!clauses.is_empty()).then(|| clauses.join(" OR "))
}

/// Bounded native deletes per table for the exact set of mutation owners. The
/// logical scope counters are recorded by the callers, not by native calls.
async fn delete_scope_batch(
    tables: &mut WorkingTables,
    scopes: &BTreeSet<SemanticScopeTuple<'_>>,
    report: &mut IngestStageReport,
    deleted_rows: &mut u64,
) -> Result<(), CoreError> {
    let batches = plan_delete_scope_batches(scopes)?;
    if !tables.origin.is_inherited() {
        return Ok(());
    }
    for batch in batches {
        if let Some(predicate) = semantic_scope_delete_predicate(&batch) {
            delete_semantic_predicate(&mut tables.table, &predicate, report, deleted_rows).await?;
        }
        if let Some(predicate) = membership_scope_delete_predicate(&batch) {
            delete_membership_predicate(&mut tables.membership_table, &predicate, report).await?;
        }
    }
    Ok(())
}

async fn delete_semantic_predicate(
    table: &mut Dataset,
    predicate: &str,
    report: &mut IngestStageReport,
    deleted_rows: &mut u64,
) -> Result<(), CoreError> {
    let delete_started = Instant::now();
    let result = table
        .delete(predicate)
        .await
        .map_err(|err| lancedb_err(&format!("delete predicate `{predicate}`"), err))?;
    *deleted_rows = deleted_rows.saturating_add(result.num_deleted_rows);
    report.durations.semantic_delete = report
        .durations
        .semantic_delete
        .saturating_add(monotonic_nanos_since(delete_started));
    report.semantic_delete_commits = report.semantic_delete_commits.saturating_add(1);
    Ok(())
}

async fn delete_membership_predicate(
    table: &mut Dataset,
    predicate: &str,
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    let delete_started = Instant::now();
    let _result = table
        .delete(predicate)
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
    table: &mut Dataset,
    surface: SearchScopeSurface,
    origin: WorkingDatasetOriginV1,
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
    if !origin.is_inherited() {
        return Ok(());
    }
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
    table: &mut Dataset,
    surface: SearchScopeSurface,
    origin: WorkingDatasetOriginV1,
    report: &mut IngestStageReport,
    deleted_rows: &mut u64,
) -> Result<(), CoreError> {
    report.semantic_delete_calls = report.semantic_delete_calls.saturating_add(1);
    let predicate = semantic_surface_delete_predicate_v1(surface)?;
    if !origin.is_inherited() {
        return Ok(());
    }
    let delete_started = Instant::now();
    let result = table.delete(predicate.as_str()).await.map_err(|err| {
        lancedb_err(
            &format!("delete semantic surface {surface:?} predicate `{predicate}`"),
            err,
        )
    })?;
    *deleted_rows = deleted_rows.saturating_add(result.num_deleted_rows);
    report.durations.semantic_delete = report
        .durations
        .semantic_delete
        .saturating_add(monotonic_nanos_since(delete_started));
    report.semantic_delete_commits = report.semantic_delete_commits.saturating_add(1);
    Ok(())
}

/// Append every row of `window` as one batch, after its owners' rows are
/// gone.
async fn append_window(
    table: &mut Dataset,
    window: &SemanticScopeWindowV1,
    dimension: usize,
    report: &mut IngestStageReport,
    appended: &mut AppendedVectors,
) -> Result<(), CoreError> {
    let mut rows: Vec<&EmbeddingRecord> = Vec::new();
    for scope in window.scopes() {
        if failpoint::should_fail_append(scope.scope.source_file.repo_relative_path.as_str()) {
            return Err(CoreError::Storage(format!(
                "semantic: injected append failure for path {}",
                scope.scope.source_file.repo_relative_path.as_str()
            )));
        }
        rows.extend(scope.embeddings.iter());
    }
    if rows.is_empty() {
        return Ok(());
    }
    let row_count = u64::try_from(rows.len()).map_err(|error| {
        CoreError::InvalidContract(format!("semantic: appended row count overflow: {error}"))
    })?;
    let total_rows = appended.rows.checked_add(row_count).ok_or_else(|| {
        CoreError::InvalidContract("semantic: appended row count overflow".into())
    })?;
    let total_bytes = appended
        .bytes
        .checked_add(window.vector_bytes())
        .ok_or_else(|| {
            CoreError::InvalidContract("semantic: appended vector byte count overflow".into())
        })?;
    let batch = build_record_batch(&rows, dimension)?;
    let append_started = Instant::now();
    let schema = batch.schema();
    let rows = arrow_array::RecordBatchIterator::new(std::iter::once(Ok(batch)), schema);
    table
        .append(rows, None)
        .await
        .map_err(|err| lancedb_err("table.add", err))?;
    report.durations.semantic_append = report
        .durations
        .semantic_append
        .saturating_add(monotonic_nanos_since(append_started));
    report.semantic_append_calls = report.semantic_append_calls.saturating_add(1);
    appended.rows = total_rows;
    appended.bytes = total_bytes;
    Ok(())
}

async fn append_cluster_membership_scope(
    table: &mut Dataset,
    scope: &SemanticReplaceScope,
    report: &mut IngestStageReport,
) -> Result<(), CoreError> {
    let Some(batch) = build_cluster_membership_record_batch(scope)? else {
        return Ok(());
    };
    let append_started = Instant::now();
    let schema = batch.schema();
    let rows = arrow_array::RecordBatchIterator::new(std::iter::once(Ok(batch)), schema);
    table
        .append(rows, None)
        .await
        .map_err(|err| lancedb_err("cluster membership table.add", err))?;
    report.durations.membership_append = report
        .durations
        .membership_append
        .saturating_add(monotonic_nanos_since(append_started));
    report.membership_append_calls = report.membership_append_calls.saturating_add(1);
    Ok(())
}

fn ensure_generation_contract(
    generation_dir: &Path,
    header: &SemanticIngestHeaderV1,
    generation_io: Option<&dyn SemanticGenerationFileIo>,
    lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> Result<GenerationContract, CoreError> {
    GenerationContract::validate_batch_shape(&header.contract)?;
    let contract_path = layout::build_contract_path(generation_dir);
    if contract_path.exists() {
        let bytes = crate::control_file::read_bounded_for_generation(
            &contract_path,
            crate::control_file::MAX_GENERATION_CONTRACT_BYTES,
            lifetime,
        )
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
    if let Some(io) = generation_io {
        io.prepare_entry_parent(&contract_path)
            .map_err(|error| admitted_fs_err("prepare generation dir", error))?;
    } else {
        fs::create_dir_all(generation_dir)
            .map_err(|err| fs_err("create generation dir", generation_dir, &err))?;
    }
    Ok(GenerationContract::from_batch(&header.contract))
}

/// Observe the persisted target and sealed base without creating or changing
/// either generation. Callers hold the target/base and directory lifecycle
/// guards when this observation is used to admit a build.
pub(crate) fn preflight_contract(
    semantic_root: &Path,
    pin: &GenerationPin,
    contract: &SemanticGenerationContractV1,
    lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> Result<(), CoreError> {
    use quanta_index_contract::lex::LexicalErrorCode;

    GenerationContract::validate_batch_shape(contract)?;
    if contract.model_contract.model_id.is_empty()
        || contract.model_contract.policy_digest.is_empty()
        || contract
            .model_contract
            .view_policy_digest
            .as_deref()
            .is_some_and(str::is_empty)
    {
        return Err(CoreError::InvalidContract(
            "semantic: model identity and embedding policy must be nonempty".to_string(),
        ));
    }
    if contract.base_generation.is_some()
        && contract
            .model_contract
            .model_version
            .as_deref()
            .is_none_or(str::is_empty)
    {
        return Err(CoreError::Typed {
            code: LexicalErrorCode::SemModelMismatch.into(),
            message:
                "semantic: delta target has no immutable model revision; rebuild the generation"
                    .to_string(),
        });
    }

    let target_dir = layout::generation_dir(
        semantic_root,
        &pin.repo_id,
        &pin.revision_id,
        pin.manifest_generation,
    );
    let target_contract_path = layout::build_contract_path(&target_dir);
    if crate::control_file::regular_file_present(&target_contract_path).map_err(|error| {
        fs_err(
            "inspect target generation contract",
            &target_contract_path,
            &error,
        )
    })? {
        let bytes = crate::control_file::read_bounded_for_generation(
            &target_contract_path,
            crate::control_file::MAX_GENERATION_CONTRACT_BYTES,
            lifetime,
        )
        .map_err(|error| {
            fs_err(
                "read target generation contract",
                &target_contract_path,
                &error,
            )
        })?;
        let existing = GenerationContract::decode(&bytes)?;
        let _merged = existing.merge_batch(contract)?;
    } else if preflight_directory_present(&layout::dataset_dir(&target_dir))? {
        return Err(CoreError::Storage(format!(
            "semantic: target dataset exists without generation contract for {}",
            target_dir.display()
        )));
    }

    if let Some(base_generation) = contract.base_generation {
        let base_dir = layout::generation_dir(
            semantic_root,
            &pin.repo_id,
            &pin.revision_id,
            base_generation,
        );
        if !preflight_directory_present(&base_dir)? {
            return Err(CoreError::NotReady(format!(
                "semantic: delta base generation {} is absent for repo={} revision={}",
                base_generation.get(),
                pin.repo_id.as_str(),
                pin.revision_id.as_str()
            )));
        }
        let marker_path = layout::sealed_marker_path(&base_dir);
        if !crate::control_file::regular_file_present(&marker_path)
            .map_err(|error| fs_err("inspect delta base sealed marker", &marker_path, &error))?
        {
            return Err(CoreError::NotReady(format!(
                "semantic: delta base generation {} is not sealed",
                base_generation.get()
            )));
        }
        if !preflight_directory_present(&layout::dataset_dir(&base_dir))? {
            return Err(CoreError::NotReady(format!(
                "semantic: delta base generation {} has no dataset",
                base_generation.get()
            )));
        }
        let marker_digest = crate::control_file::read_string_for_generation(
            &marker_path,
            crate::control_file::MAX_SEALED_MARKER_BYTES,
            lifetime,
        )
        .map_err(|error| fs_err("read delta base sealed marker", &marker_path, &error))?;
        crate::sealed_manifest::verify_sealed_sidecars(&base_dir, &marker_digest, lifetime)?;
        let manifest_path = layout::manifest_path(&base_dir);
        let manifest = SemanticManifest::decode(
            &crate::control_file::read_bounded_for_generation(
                &manifest_path,
                crate::control_file::MAX_SCOPE_MANIFEST_BYTES,
                lifetime,
            )
            .map_err(|error| fs_err("read delta base manifest", &manifest_path, &error))?,
        )?;
        manifest.validate_scope(&pin.repo_id, &pin.revision_id, base_generation)?;
        if manifest.manifest_digest != marker_digest {
            return Err(CoreError::Storage(format!(
                "semantic: delta base generation {} manifest and sealed marker disagree",
                base_generation.get()
            )));
        }
        let base_contract_path = layout::build_contract_path(&base_dir);
        let base_contract = GenerationContract::decode(
            &crate::control_file::read_bounded_for_generation(
                &base_contract_path,
                crate::control_file::MAX_GENERATION_CONTRACT_BYTES,
                lifetime,
            )
            .map_err(|error| {
                fs_err(
                    "read delta base generation contract",
                    &base_contract_path,
                    &error,
                )
            })?,
        )?;
        manifest.validate_against(&base_contract)?;
        let target = GenerationContract::from_batch(contract);
        for (field, matches) in [
            ("model_id", base_contract.model_id == target.model_id),
            (
                "model_version",
                base_contract.model_version == target.model_version,
            ),
            ("dimension", base_contract.dimension == target.dimension),
            (
                "distance_metric",
                base_contract.distance_metric == target.distance_metric,
            ),
            (
                "normalization",
                base_contract.normalization == target.normalization,
            ),
            (
                "policy_digest",
                base_contract.policy_digest == target.policy_digest,
            ),
            (
                "view_policy_digest",
                base_contract.view_policy_digest == target.view_policy_digest,
            ),
            (
                "corpus_policy_digest",
                base_contract.corpus_policy_digest == target.corpus_policy_digest,
            ),
        ] {
            if !matches {
                return Err(CoreError::Typed {
                    code: LexicalErrorCode::SemModelMismatch.into(),
                    message: format!(
                        "semantic: delta base {field} differs from target; rebuild the generation"
                    ),
                });
            }
        }
        if base_contract
            .model_version
            .as_deref()
            .is_none_or(str::is_empty)
        {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::SemModelMismatch.into(),
                message:
                    "semantic: delta base has no immutable model revision; rebuild the generation"
                        .to_string(),
            });
        }
    }
    Ok(())
}

fn preflight_directory_present(path: &Path) -> Result<bool, CoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => Err(CoreError::Storage(format!(
            "semantic: path {} is not a directory",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(fs_err("inspect preflight dataset", path, &error)),
    }
}

#[cfg(test)]
fn persist_generation_contract(
    generation_dir: &Path,
    contract: &GenerationContract,
) -> Result<(), CoreError> {
    let bytes = contract.encode()?;
    let _output = write_atomic(
        &layout::build_contract_path(generation_dir),
        &bytes,
        "write generation contract",
        None,
    )?;
    Ok(())
}

fn stage_generation_contract(
    generation_dir: &Path,
    contract: &GenerationContract,
    publication: Option<PublicationOutputAdmission<'_>>,
) -> Result<AtomicOutput, CoreError> {
    let staged_path = layout::build_contract_path(generation_dir).with_extension("next");
    let bytes = contract.encode()?;
    write_atomic(
        &staged_path,
        &bytes,
        "stage generation contract for dataset promotion",
        publication,
    )
}

fn promote_staged_generation_contract(
    generation_dir: &Path,
    staged: &mut AtomicOutput,
) -> Result<(), CoreError> {
    if failpoint::should_fail_contract_promotion(generation_dir.to_string_lossy().as_ref()) {
        return Err(CoreError::Storage(
            "semantic: injected generation contract promotion failure".to_string(),
        ));
    }
    let contract_path = layout::build_contract_path(generation_dir);
    staged.rename_to(&contract_path, "promote generation contract")
}

fn prepare_staging_dataset(
    semantic_root: &Path,
    generation_dir: &Path,
    generation_contract: &GenerationContract,
    repo_id: &quanta_index_contract::RepoId,
    revision_id: &quanta_index_contract::RevisionId,
    generation_io: Option<&dyn SemanticGenerationFileIo>,
) -> Result<PreparedWorkingDatasetV1, CoreError> {
    let dataset = layout::dataset_dir(generation_dir);
    let staging = generation_dir.join(STAGING_DIR_NAME);
    remove_dir_if_exists("clean stale staging dir", &staging, generation_io)?;
    if dataset.exists() {
        inherit_dataset_tree(&dataset, &staging, generation_io)?;
        return Ok(PreparedWorkingDatasetV1 {
            path: staging,
            origin: WorkingDatasetOriginV1::TargetContinuation,
        });
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
        let base_marker = layout::sealed_marker_path(&base_dir);
        if !crate::control_file::regular_file_present(&base_marker)
            .map_err(|error| fs_err("inspect delta base sealed marker", &base_marker, &error))?
        {
            return Err(CoreError::NotReady(format!(
                "semantic: delta base generation {} is not sealed for repo={} revision={}; cannot clone an in-progress base",
                base_generation.get(),
                repo_id.as_str(),
                revision_id.as_str()
            )));
        }
        inherit_dataset_tree(&base_dataset, &staging, generation_io)?;
        return Ok(PreparedWorkingDatasetV1 {
            path: staging,
            origin: WorkingDatasetOriginV1::SealedBaseClone,
        });
    }
    if let Some(io) = generation_io {
        io.prepare_staging_directory(&staging)
            .map_err(|error| admitted_fs_err("prepare staging dataset dir", error))?;
    } else {
        fs::create_dir_all(&staging)
            .map_err(|err| fs_err("create staging dataset dir", &staging, &err))?;
    }
    Ok(PreparedWorkingDatasetV1 {
        path: staging,
        origin: WorkingDatasetOriginV1::FreshEmpty,
    })
}

/// The verified index contract of the sealed base a delta inherited.
///
/// The base's canonical open validates its committed sidecars, layout,
/// schema, row count and exact index identity. Its row commitment is then
/// recomputed against the sealed root. Every delta index decision uses this
/// same base authority, including append, budget retrain and contraction.
async fn inherited_vector_index_seal_v1(
    semantic_root: &Path,
    header: &SemanticIngestHeaderV1,
    generation_contract: &GenerationContract,
    session: Option<Arc<Session>>,
    control_lifetime: Option<&dyn PublicationObjectLifetimePort>,
    mode: crate::qualified_fs::LanceUriMode,
) -> Result<Option<crate::search::VerifiedInheritedVectorIndexV1>, CoreError> {
    let Some(base_generation) = generation_contract.base_generation else {
        return Ok(None);
    };
    let verified = crate::search::verified_base_vector_index_seal_v1(
        semantic_root,
        &header.pin.repo_id,
        &header.pin.revision_id,
        base_generation,
        session,
        control_lifetime,
        mode,
    )
    .await?;
    Ok(Some(verified))
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
    table: &mut Dataset,
    membership_table: &Dataset,
    header: &SemanticIngestHeaderV1,
    generation_contract: &GenerationContract,
    session: Option<Arc<Session>>,
    control_lifetime: Option<&dyn PublicationObjectLifetimePort>,
    mode: crate::qualified_fs::LanceUriMode,
) -> Result<Vec<u8>, CoreError> {
    let semantic_rows = dataset_row_commitment_v1(table).await?;
    let row_count_u64 = semantic_rows.row_count;
    let membership_commitment = collect_cluster_membership_commitment_v1(membership_table).await?;

    // The dense lane's index contract (QI-BB-027): built with every
    // parameter explicit, read back from the library, and recorded in the
    // manifest so the open can verify the dataset against the seal. A
    // delta appends to the index it inherited when the base's contract
    // admits it and trains otherwise. The index is approximate; what the
    // seal makes deterministic is its recipe, effort and lineage, not its
    // top-k.
    let inherited = inherited_vector_index_seal_v1(
        semantic_root,
        header,
        generation_contract,
        session,
        control_lifetime,
        mode,
    )
    .await?;
    let contracted = match inherited.as_ref() {
        Some(base) => {
            inherited_index_contracted_v1(
                table,
                &base.seal,
                row_count_u64,
                &base.row_fingerprints,
                &semantic_rows.row_fingerprints,
            )
            .await?
        }
        None => false,
    };
    // A complete segment deletion changes the physical ANN topology. Only a
    // proven subset of an intact base may retrain over the current live rows;
    // it must never inherit the old segment count or lineage.
    let inherited_for_seal = if contracted {
        None
    } else {
        inherited.as_ref().map(|base| &base.seal)
    };
    let vector_index = seal_vector_index_v1(
        table,
        VectorIndexSealInputV1 {
            generation: header.pin.manifest_generation.get(),
            row_count: row_count_u64,
            inherited: inherited_for_seal,
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
            present: semantic_rows.coverage.present_corpora,
            required: generation_contract.required_corpora.clone(),
            card_schema_versions: semantic_rows.coverage.card_schema_versions,
            render_policy_digests: semantic_rows.coverage.render_policy_digests,
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
    table: Dataset,
    membership_table: Dataset,
    origin: WorkingDatasetOriginV1,
}

#[cfg(test)]
async fn open_working_tables(
    working_dataset: &Path,
    dimension: usize,
) -> Result<WorkingTables, CoreError> {
    open_working_tables_with_session(
        working_dataset,
        dimension,
        None,
        WorkingDatasetOriginV1::TargetContinuation,
        crate::qualified_fs::LanceUriMode::File,
    )
    .await
}

async fn open_working_tables_with_session(
    working_dataset: &Path,
    dimension: usize,
    session: Option<Arc<Session>>,
    origin: WorkingDatasetOriginV1,
    mode: crate::qualified_fs::LanceUriMode,
) -> Result<WorkingTables, CoreError> {
    let session = session.unwrap_or_else(crate::qualified_fs::new_persisted_lance_session);
    let table = ensure_working_dataset(
        working_dataset,
        TABLE_NAME,
        semantic_schema(dimension_to_i32(dimension)?),
        Arc::clone(&session),
        mode,
    )
    .await?;
    verify_table_dimension(&table, dimension)?;
    let membership_table = ensure_working_dataset(
        working_dataset,
        CLUSTER_MEMBERSHIP_TABLE_NAME,
        cluster_membership_schema(),
        session,
        mode,
    )
    .await?;
    verify_membership_schema(&membership_table)?;
    Ok(WorkingTables {
        table,
        membership_table,
        origin,
    })
}

async fn apply_clear_surfaces(
    tables: &mut WorkingTables,
    surfaces: &[SearchScopeSurface],
    report: &mut IngestStageReport,
    deleted_rows: &mut u64,
) -> Result<(), CoreError> {
    for surface in surfaces {
        delete_surface_rows(
            &mut tables.table,
            *surface,
            tables.origin,
            report,
            deleted_rows,
        )
        .await?;
        delete_cluster_membership_for_surface(
            &mut tables.membership_table,
            *surface,
            tables.origin,
            report,
        )
        .await?;
    }
    Ok(())
}

/// Replace one admitted window: its exact owners are deleted in bounded batches, then the window's
/// rows are appended as one batch and its memberships per scope.
async fn apply_window(
    tables: &mut WorkingTables,
    window: &SemanticScopeWindowV1,
    dimension: usize,
    report: &mut IngestStageReport,
    appended: &mut AppendedVectors,
    diagnostic: Option<&DiagnosticContextV1>,
    deleted_rows: &mut u64,
) -> Result<(), CoreError> {
    let scopes = window
        .scopes()
        .iter()
        .flat_map(|scope| &scope.embeddings)
        .map(semantic_scope_tuple_from_embedding)
        .collect();
    let delete_before = report
        .semantic_delete_calls
        .saturating_add(report.membership_delete_calls);
    for _scope in window.scopes() {
        report.semantic_delete_calls = report.semantic_delete_calls.saturating_add(1);
        report.membership_delete_calls = report.membership_delete_calls.saturating_add(1);
    }
    let delete_span = begin_build_stage(diagnostic, DiagnosticStageV1::SemanticDelete);
    let rows_before = *deleted_rows;
    let deleted = delete_scope_batch(tables, &scopes, report, deleted_rows).await;
    build_counter(
        diagnostic,
        delete_span.as_ref(),
        DiagnosticCounterV1::SemanticDeleteCalls,
        report
            .semantic_delete_calls
            .saturating_add(report.membership_delete_calls)
            .saturating_sub(delete_before),
    );
    build_counter(
        diagnostic,
        delete_span.as_ref(),
        DiagnosticCounterV1::SemanticRowsDeleted,
        deleted_rows.saturating_sub(rows_before),
    );
    finish_build_stage(delete_span, deleted.is_ok());
    deleted?;

    let append_span = begin_build_stage(diagnostic, DiagnosticStageV1::SemanticAppend);
    let append_before = report
        .semantic_append_calls
        .saturating_add(report.membership_append_calls);
    let rows_before = appended.rows;
    let bytes_before = appended.bytes;
    let appended_result: Result<(), CoreError> = async {
        append_window(&mut tables.table, window, dimension, report, appended).await?;
        for scope in window.scopes() {
            append_cluster_membership_scope(&mut tables.membership_table, scope, report).await?;
        }
        Ok(())
    }
    .await;
    build_counter(
        diagnostic,
        append_span.as_ref(),
        DiagnosticCounterV1::SemanticAppendCalls,
        report
            .semantic_append_calls
            .saturating_add(report.membership_append_calls)
            .saturating_sub(append_before),
    );
    build_counter(
        diagnostic,
        append_span.as_ref(),
        DiagnosticCounterV1::SemanticRowsAppended,
        appended.rows.saturating_sub(rows_before),
    );
    build_counter(
        diagnostic,
        append_span.as_ref(),
        DiagnosticCounterV1::SemanticVectorBytes,
        appended.bytes.saturating_sub(bytes_before),
    );
    finish_build_stage(append_span, appended_result.is_ok());
    appended_result?;
    Ok(())
}

async fn apply_tombstones(
    tables: &mut WorkingTables,
    tombstones: &[SemanticTombstoneScope],
    policy: SemanticStreamWindowPolicy,
    report: &mut IngestStageReport,
    deleted_rows: &mut u64,
) -> Result<(), CoreError> {
    for chunk in tombstones.chunks(policy.max_owner_scopes()) {
        let scopes = tombstone_tuples(chunk);
        for _scope in chunk {
            report.semantic_delete_calls = report.semantic_delete_calls.saturating_add(1);
            report.membership_delete_calls = report.membership_delete_calls.saturating_add(1);
        }
        delete_scope_batch(tables, &scopes, report, deleted_rows).await?;
    }
    Ok(())
}

async fn seal_manifest_bytes(
    semantic_root: &Path,
    tables: &mut WorkingTables,
    header: &SemanticIngestHeaderV1,
    generation_contract: &GenerationContract,
    session: Option<Arc<Session>>,
    control_lifetime: Option<&dyn PublicationObjectLifetimePort>,
    mode: crate::qualified_fs::LanceUriMode,
) -> Result<Vec<u8>, CoreError> {
    validate_cluster_membership_coverage_v1(&tables.table, &tables.membership_table).await?;
    build_manifest_bytes(
        semantic_root,
        &mut tables.table,
        &tables.membership_table,
        header,
        generation_contract,
        session,
        control_lifetime,
        mode,
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
#[expect(
    clippy::too_many_arguments,
    reason = "Streaming owner carries distinct authority, storage, progress and diagnostic state without changing admission."
)]
fn apply_scope_stream(
    runtime: &tokio::runtime::Runtime,
    tables: &mut WorkingTables,
    policy: SemanticStreamWindowPolicy,
    header: &SemanticIngestHeaderV1,
    authority: &mut StreamScopeAuthorityV1,
    source: (Option<SemanticScopeWindowV1>, &mut dyn SemanticScopeSource),
    report: &mut IngestStageReport,
    appended: &mut AppendedVectors,
    diagnostic: Option<&DiagnosticContextV1>,
    deleted_rows: &mut u64,
) -> Result<SemanticStreamTallyV1, CoreError> {
    let dimension = header.dimension()?;
    let normalization = header.contract.model_contract.normalization;
    let mut tally = SemanticStreamTallyV1::default();
    let (mut next_window, scopes) = source;
    while let Some(window) = next_window {
        let _fill = policy.admit(&window)?;
        preflight_window_delete_predicates(&window)?;
        report.windows = report.windows.saturating_add(1);
        for scope in window.scopes() {
            validate_replace_scope(scope, dimension, normalization)?;
            authority.admit_replace_scope(scope)?;
            report.owner_scopes = report.owner_scopes.saturating_add(1);
        }
        let rows = window.rows()?;
        crate::run_blocking(
            runtime,
            apply_window(
                tables,
                &window,
                dimension,
                report,
                appended,
                diagnostic,
                deleted_rows,
            ),
        )?;
        tally.count_window(window.scopes().len(), rows, window.vector_bytes())?;
        drop(window);
        next_window = scopes.next_window()?;
    }
    Ok(tally)
}

/// Test-fixture projection of the canonical reported build. Production
/// adapters retain the report through `SemanticScopeStreamBuildPort`.
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

/// The canonical storage build, returning tally and transient accounting.
///
/// The [`IngestStageReport`] is observation only: the durability sequence
/// below is un-reordered, with every seal/promotion step intact. The test
/// helper `build_stream` projects the tally without creating another build.
#[cfg(test)]
pub(crate) fn build_stream_reported(
    runtime: &tokio::runtime::Runtime,
    semantic_root: &Path,
    policy: SemanticStreamWindowPolicy,
    header: &SemanticIngestHeaderV1,
    scopes: &mut dyn SemanticScopeSource,
    seal_tallies: &SealTalliesV1,
) -> Result<(SemanticStreamTallyV1, IngestStageReport), CoreError> {
    build_stream_reported_with_diagnostic(
        runtime,
        semantic_root,
        policy,
        header,
        scopes,
        seal_tallies,
        None,
        None,
    )
}

#[cfg(test)]
pub(crate) fn build_stream_reported_with_diagnostic(
    runtime: &tokio::runtime::Runtime,
    semantic_root: &Path,
    policy: SemanticStreamWindowPolicy,
    header: &SemanticIngestHeaderV1,
    scopes: &mut dyn SemanticScopeSource,
    seal_tallies: &SealTalliesV1,
    publication: Option<PublicationOutputAdmission<'_>>,
    diagnostic: Option<&DiagnosticContextV1>,
) -> Result<(SemanticStreamTallyV1, IngestStageReport), CoreError> {
    build_stream_reported_with_diagnostic_and_lance(
        runtime,
        semantic_root,
        policy,
        header,
        scopes,
        &BuildFileContext {
            seal_tallies,
            work_files: None,
            dataset_read_lifetime: None,
            lance_uri_mode: crate::qualified_fs::LanceUriMode::File,
        },
        publication,
        diagnostic,
    )
}

pub(crate) struct BuildFileContext<'a> {
    pub(crate) seal_tallies: &'a SealTalliesV1,
    pub(crate) work_files: Option<&'a SemanticWorkFiles>,
    pub(crate) dataset_read_lifetime: Option<&'a dyn PublicationObjectLifetimePort>,
    pub(crate) lance_uri_mode: crate::qualified_fs::LanceUriMode,
}

pub(crate) struct SemanticWorkFiles {
    pub(crate) generation: Arc<dyn SemanticGenerationFileIo>,
    pub(crate) session: Arc<Session>,
}

pub(crate) fn build_stream_reported_with_diagnostic_and_lance(
    runtime: &tokio::runtime::Runtime,
    semantic_root: &Path,
    policy: SemanticStreamWindowPolicy,
    header: &SemanticIngestHeaderV1,
    scopes: &mut dyn SemanticScopeSource,
    files: &BuildFileContext<'_>,
    publication: Option<PublicationOutputAdmission<'_>>,
    diagnostic: Option<&DiagnosticContextV1>,
) -> Result<(SemanticStreamTallyV1, IngestStageReport), CoreError> {
    let seal_tallies = files.seal_tallies;
    let generation_io = files.work_files.map(|work| &work.generation);
    let dataset_read_lifetime = files.dataset_read_lifetime;
    if files.work_files.is_some() != dataset_read_lifetime.is_some() {
        return Err(CoreError::InvalidContract(
            "qualified semantic build requires both work files and generation reader lifetime"
                .into(),
        ));
    }
    let lance_session = files.work_files.map(|work| Arc::clone(&work.session));
    let build_started = Instant::now();
    let mut report = IngestStageReport::default();
    let mut appended = AppendedVectors::default();
    let mut deleted_rows = 0_u64;
    let generation_dir = layout::generation_dir(
        semantic_root,
        &header.pin.repo_id,
        &header.pin.revision_id,
        header.pin.manifest_generation,
    );
    let sealed_marker = layout::sealed_marker_path(&generation_dir);
    if crate::control_file::regular_file_present(&sealed_marker)
        .map_err(|error| fs_err("inspect sealed marker", &sealed_marker, &error))?
    {
        return Err(CoreError::Storage(format!(
            "semantic: generation {} is already sealed; refusing in-place mutation",
            header.pin.manifest_generation.get()
        )));
    }
    preflight_contract(
        semantic_root,
        &header.pin,
        &header.contract,
        dataset_read_lifetime,
    )?;
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
    GenerationContract::validate_batch_shape(&header.contract)?;
    preflight_tombstone_delete_predicates(&header.mutations.tombstone_scopes, policy)?;
    let mut authority = StreamScopeAuthorityV1::new(header)?;
    // Lease only the first window before preparing storage. Subsequent windows
    // are admitted before their writes; failures never promote earlier staging.
    let prefetch_started = Instant::now();
    let prefetch_span = begin_build_stage(diagnostic, DiagnosticStageV1::SemanticStream);
    let first_window = (|| {
        let first_window = scopes.next_window()?;
        if let Some(window) = &first_window {
            let _fill = policy.admit(window)?;
            preflight_window_delete_predicates(window)?;
        }
        Ok::<_, CoreError>(first_window)
    })();
    finish_build_stage(prefetch_span, first_window.is_ok());
    let first_window = first_window?;
    let prefetch_elapsed = monotonic_nanos_since(prefetch_started);

    let prepare_started = Instant::now();
    let prepare_span = begin_build_stage(diagnostic, DiagnosticStageV1::SemanticPrepare);
    let prepared = (|| {
        DatasetPromotion::recover_with_admission(&generation_dir, generation_io.cloned())?;
        let generation_contract = ensure_generation_contract(
            &generation_dir,
            header,
            generation_io.map(AsRef::as_ref),
            dataset_read_lifetime,
        )?;
        let working_dataset = prepare_staging_dataset(
            semantic_root,
            &generation_dir,
            &generation_contract,
            &header.pin.repo_id,
            &header.pin.revision_id,
            generation_io.map(AsRef::as_ref),
        )?;
        let tables = crate::run_blocking(
            runtime,
            open_working_tables_with_session(
                &working_dataset.path,
                dimension,
                lance_session.clone(),
                working_dataset.origin,
                files.lance_uri_mode,
            ),
        )?;
        Ok::<_, CoreError>((generation_contract, tables))
    })();
    finish_build_stage(prepare_span, prepared.is_ok());
    let (generation_contract, mut tables) = prepared?;
    report.durations.prepare = monotonic_nanos_since(prepare_started);

    let (tally, manifest_bytes) = {
        let clear_started = Instant::now();
        let clear_span = begin_build_stage(diagnostic, DiagnosticStageV1::SemanticDelete);
        let clear_before = report
            .semantic_delete_calls
            .saturating_add(report.membership_delete_calls);
        let cleared = crate::run_blocking(
            runtime,
            apply_clear_surfaces(
                &mut tables,
                &header.mutations.clear_surfaces,
                &mut report,
                &mut deleted_rows,
            ),
        );
        build_counter(
            diagnostic,
            clear_span.as_ref(),
            DiagnosticCounterV1::SemanticDeleteCalls,
            report
                .semantic_delete_calls
                .saturating_add(report.membership_delete_calls)
                .saturating_sub(clear_before),
        );
        build_counter(
            diagnostic,
            clear_span.as_ref(),
            DiagnosticCounterV1::SemanticRowsDeleted,
            deleted_rows,
        );
        finish_build_stage(clear_span, cleared.is_ok());
        cleared?;
        report.durations.clear_surfaces = monotonic_nanos_since(clear_started);
        let stream_started = Instant::now();
        let stream_span = begin_build_stage(diagnostic, DiagnosticStageV1::SemanticStream);
        let stream_context = diagnostic
            .zip(stream_span.as_ref())
            .map(|(context, span)| context.child_context(span));
        let windows_before = report.windows;
        let scopes_before = report.owner_scopes;
        let streamed = apply_scope_stream(
            runtime,
            &mut tables,
            policy,
            header,
            &mut authority,
            (first_window, scopes),
            &mut report,
            &mut appended,
            stream_context.as_ref(),
            &mut deleted_rows,
        );
        build_counter(
            diagnostic,
            stream_span.as_ref(),
            DiagnosticCounterV1::SemanticWindows,
            report.windows.saturating_sub(windows_before),
        );
        build_counter(
            diagnostic,
            stream_span.as_ref(),
            DiagnosticCounterV1::SemanticOwnerScopes,
            report.owner_scopes.saturating_sub(scopes_before),
        );
        finish_build_stage(stream_span, streamed.is_ok());
        let tally = streamed?;
        report.durations.stream =
            prefetch_elapsed.saturating_add(monotonic_nanos_since(stream_started));
        let tombstones_started = Instant::now();
        let tombstones_span = begin_build_stage(diagnostic, DiagnosticStageV1::SemanticDelete);
        let tombstones_before = report
            .semantic_delete_calls
            .saturating_add(report.membership_delete_calls);
        let tombstone_rows_before = deleted_rows;
        let deleted = crate::run_blocking(
            runtime,
            apply_tombstones(
                &mut tables,
                &header.mutations.tombstone_scopes,
                policy,
                &mut report,
                &mut deleted_rows,
            ),
        );
        build_counter(
            diagnostic,
            tombstones_span.as_ref(),
            DiagnosticCounterV1::SemanticDeleteCalls,
            report
                .semantic_delete_calls
                .saturating_add(report.membership_delete_calls)
                .saturating_sub(tombstones_before),
        );
        build_counter(
            diagnostic,
            tombstones_span.as_ref(),
            DiagnosticCounterV1::SemanticRowsDeleted,
            deleted_rows.saturating_sub(tombstone_rows_before),
        );
        finish_build_stage(tombstones_span, deleted.is_ok());
        deleted?;
        report.durations.tombstones = monotonic_nanos_since(tombstones_started);
        let manifest_bytes = if header.batch.seal {
            let seal_started = Instant::now();
            let seal_span = begin_build_stage(diagnostic, DiagnosticStageV1::SemanticSeal);
            let sealed = crate::run_blocking(
                runtime,
                seal_manifest_bytes(
                    semantic_root,
                    &mut tables,
                    header,
                    &generation_contract,
                    lance_session,
                    dataset_read_lifetime,
                    files.lance_uri_mode,
                ),
            );
            finish_build_stage(seal_span, sealed.is_ok());
            let sealed = sealed?;
            report.durations.seal = Some(monotonic_nanos_since(seal_started));
            Some(sealed)
        } else {
            None
        };
        drop(tables);
        (tally, manifest_bytes)
    };

    // Stage the generation-wide policy only after every table mutation and
    // manifest validation has succeeded. The final rename follows dataset
    // promotion; recovery promotes this staged sidecar only when the dataset is
    // already durable, so a rejected append/tombstone/seal cannot advance policy
    // independently of its rows.
    let promotion_started = Instant::now();
    let promotion_span = begin_build_stage(diagnostic, DiagnosticStageV1::SemanticPromote);
    let promotion_result = (|| -> Result<(), CoreError> {
        let mut staged_contract =
            stage_generation_contract(&generation_dir, &generation_contract, publication)?;
        let promotion = DatasetPromotion::begin_with_admission(
            &generation_dir,
            &staged_contract,
            publication,
            generation_io.cloned(),
        )?;
        #[cfg(test)]
        exit_for_promotion_crash_boundary(PRE_DATASET_PROMOTION);
        // Any rename/sync failure retains the authenticated intent, staging,
        // contract and backup for recovery. Cleanup cannot guess its phase.
        promotion.promote_dataset()?;
        #[cfg(test)]
        exit_for_promotion_crash_boundary(POST_DATASET_PRE_CONTRACT_PROMOTION);
        if let Err(contract_err) =
            promote_staged_generation_contract(&generation_dir, &mut staged_contract)
        {
            // Once rename has succeeded, the dataset and contract are both
            // advanced. A sync or ledger error keeps their recovery state;
            // rolling back only the dataset would make the pair inconsistent.
            if staged_contract.path() == layout::build_contract_path(&generation_dir) {
                return Err(contract_err);
            }
            return Err(match promotion.rollback(staged_contract) {
                Ok(()) => contract_err,
                Err(error) => {
                    with_cleanup_context(contract_err, format!("dataset rollback: {error}"))
                }
            });
        }
        promotion.finish()?;

        let _ready = write_atomic(
            &layout::ready_marker_path(&generation_dir),
            b"ready",
            "write ready marker",
            publication,
        )?;
        if let Some(manifest_bytes) = manifest_bytes {
            let _manifest = write_atomic(
                &layout::manifest_path(&generation_dir),
                &manifest_bytes,
                "write manifest",
                publication,
            )?;
            // The file commitment every door checks the layout of and the scrub
            // proves the bytes of (QI-BB-017): measured after the dataset, the
            // contract and the scope manifest are durable, written before the
            // marker that makes the generation sealed. A delta inherits its
            // base's digests for every file it still shares by inode, so the
            // bytes hashed here are proportional to what this generation added
            // (QI-BB-006 #4); the measurement is tallied for the scrape.
            let base_dir = delta_base_dir_v1(semantic_root, header, &generation_contract);
            let (sealed_manifest_bytes, measurement) = build_sealed_manifest_bytes_with_lifetime(
                &generation_dir,
                header.batch.manifest_digest.as_str(),
                base_dir.as_deref(),
                dataset_read_lifetime,
            )?;
            seal_tallies.record(measurement);
            let _sealed_manifest = write_atomic(
                &sealed_manifest_path(&generation_dir),
                &sealed_manifest_bytes,
                "write sealed generation manifest",
                publication,
            )?;
            crate::control_file::ensure_bounded(
                header.batch.manifest_digest.as_bytes(),
                crate::control_file::MAX_SEALED_MARKER_BYTES,
                "sealed marker",
            )?;
            let _sealed_marker = write_atomic(
                &layout::sealed_marker_path(&generation_dir),
                header.batch.manifest_digest.as_bytes(),
                "write sealed marker",
                publication,
            )?;
        }
        Ok(())
    })();
    finish_build_stage(promotion_span, promotion_result.is_ok());
    promotion_result?;
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
