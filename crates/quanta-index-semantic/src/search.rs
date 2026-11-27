//! Lancedb-backed durable open + search path.
//!
//! `open_generation` opens exactly one sealed generation's Lance dataset
//! (manifest validated against requested scope, row count cross-checked
//! against the manifest) and returns a `LoadedGeneration` ready to serve
//! `vector_search`. There is no cross-generation replay; the open cost is
//! bounded by reopening a single Lance dataset.
//!
//! The searcher returns scores as cosine similarity in `[-1, 1]` (Lance
//! reports cosine *distance* in the `_distance` column; we convert
//! `similarity = 1 - distance` so the historical query-time contract is
//! preserved).
//!
//! Every search runs under the request budget (W5 phase 3): the vector
//! query is refused before issue, dropped in flight or stopped between
//! rows once the budget interrupts, and the typed answer names the lane
//! that looked — see [`crate::budget`].

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, StringArray, UInt32Array, UInt64Array,
};
use arrow_schema::DataType;
use futures::TryStreamExt as _;
use lance::Dataset;
use lance::dataset::{builder::DatasetBuilder, scanner::Scanner};
use lance::session::Session;
use lance_linalg::distance::DistanceType as LanceDistanceType;
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    ClusterMembershipBatchReadRequestV1, ClusterMembershipBatchReadResponseV1,
    ClusterMembershipCompletenessV1, ClusterMembershipReadFailureV1,
    ClusterMembershipReadOutcomeV1, ClusterMembershipReadRejectionV1,
    ClusterMembershipReadRequestV1, ClusterMembershipSnapshotV1, EmbeddingNormalization,
    GenerationPin, LexicalCandidate, ManifestGeneration, OwnerDocKind, QueryConstraintSetV1,
    RepoId, RepoRelativePath, RevisionId, SemanticCorpusKindV1, SourceFileKey, SymbolId,
    canonical_order::{CanonicalOrderBreakV1, first_canonical_order_break_v1},
    cluster_membership_content_digest_v1,
};
use quanta_index_core::domains::semantic::{
    DenseLaneContractV1, SemanticPolicy, SemanticSearchHitV1, SemanticSearcher,
};
use quanta_index_core::{CoreError, RequestBudgetV1};

use crate::budget::{
    DenseLaneBudgetV1, DenseLaneKindV1, DenseLaneTalliesV1, RowBudgetProbe, failpoint,
    race_with_budget,
};
use crate::control_file::receipt::refuse_if_quarantined;
use crate::errors::lancedb_err;
use crate::generation_contract::GenerationContract;
use crate::layout::{
    self, CLUSTER_MEMBERSHIP_TABLE_NAME, COLUMN_AUTHORITY_DIGEST, COLUMN_CORPUS_KIND,
    COLUMN_EMBEDDING_ID, COLUMN_END_LINE, COLUMN_LANGUAGE, COLUMN_MEMBERSHIP_AUTHORITY_DIGEST,
    COLUMN_MEMBERSHIP_CLUSTER_RECORD_ID, COLUMN_MEMBERSHIP_CONTENT_DIGEST,
    COLUMN_MEMBERSHIP_MEMBER_COUNT, COLUMN_MEMBERSHIP_MEMBER_SYMBOL_ID, COLUMN_MEMBERSHIP_ORDINAL,
    COLUMN_OWNER_ID, COLUMN_OWNER_KIND, COLUMN_RECORD_ID, COLUMN_REPO_RELATIVE_PATH,
    COLUMN_SNIPPET, COLUMN_SOURCE_REPO_ID, COLUMN_START_LINE, COLUMN_VECTOR, TABLE_NAME,
};
use crate::manifest::{SemanticManifest, VectorIndexSealV1};
use crate::qualified_fs::ReadSession;
use crate::sealed_manifest::verify_sealed_manifest;
use crate::semantic_row_integrity_v1::{SemanticRowFingerprintV1, dataset_row_commitment_v1};
use crate::sql::build_id_in_filter;
use crate::vector_index::{LoadedVectorIndexV1, verify_vector_index_dataset_v1};
use quanta_index_core::publication_admission::PublicationObjectLifetimePort;

const COLUMN_DISTANCE: &str = "_distance";

mod exact_tie_heap;
use exact_tie_heap::{ExactTieHeap, ExactTieKey};

const EXACT_SCAN_BATCH_ROWS: usize = 32;
const DATASET_READ_BATCH_ROWS: usize = 1024;
const COLUMN_ROW_ID: &str = "_rowid";

fn load_generation_contract(
    generation_dir: &Path,
    control_lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> Result<GenerationContract, CoreError> {
    let contract_path = layout::build_contract_path(generation_dir);
    let bytes = crate::control_file::read_bounded_for_generation(
        &contract_path,
        crate::control_file::MAX_GENERATION_CONTRACT_BYTES,
        control_lifetime,
    )
    .map_err(|err| crate::errors::fs_err("read generation contract", &contract_path, &err))?;
    GenerationContract::decode(&bytes)
}

fn read_scope_manifest(
    generation_dir: &Path,
    control_lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> Result<SemanticManifest, CoreError> {
    let manifest_path = layout::manifest_path(generation_dir);
    let manifest_bytes = crate::control_file::read_bounded_for_generation(
        &manifest_path,
        crate::control_file::MAX_SCOPE_MANIFEST_BYTES,
        control_lifetime,
    )
    .map_err(|err| crate::errors::fs_err("read manifest", &manifest_path, &err))?;
    SemanticManifest::decode(&manifest_bytes)
}

/// The two immutable Lance datasets of one verified sealed generation.
pub(crate) struct LoadedGeneration {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    dimension: usize,
    sealed_row_count: u64,
    /// The normalization the generation was sealed under: what every query
    /// vector is held to, as every ingested row was (QI-BB-031).
    normalization: EmbeddingNormalization,
    model_id: String,
    model_version: Option<String>,
    table: Dataset,
    /// The cluster-membership sidecar table every sealed generation carries.
    cluster_membership: Dataset,
    /// The dense lane's index, verified against the seal at open.
    vector_index: LoadedVectorIndexV1,
    /// On-disk bytes of the dataset this handle maps, as the seal committed
    /// them.
    resident_bytes_estimate: u64,
    /// The sealed manifest digest the open proved.
    manifest_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClusterMembershipReadRowV1 {
    ordinal: u32,
    cluster_record_id: String,
    authority_digest: String,
    member_symbol_id: String,
    member_count: u32,
    membership_digest: String,
}

fn validate_cluster_membership_rows_v1(
    mut rows: Vec<ClusterMembershipReadRowV1>,
    request: &ClusterMembershipReadRequestV1,
) -> Result<Vec<SymbolId>, ClusterMembershipReadFailureV1> {
    if rows.is_empty() {
        return Err(ClusterMembershipReadFailureV1::CurrentGenerationMissing);
    }
    let Ok(row_count) = u32::try_from(rows.len()) else {
        return Err(ClusterMembershipReadFailureV1::MemberLimitExceeded);
    };
    if row_count > quanta_index_contract::MAX_CLUSTER_MEMBERSHIP_READ_V1 {
        return Err(ClusterMembershipReadFailureV1::MemberLimitExceeded);
    }
    rows.sort_by_key(|row| row.ordinal);
    let mut members = Vec::with_capacity(rows.len());
    let mut committed_count = None;
    let mut committed_digest = None;
    for (index, row) in rows.into_iter().enumerate() {
        if row.cluster_record_id != request.cluster_record_id {
            return Err(ClusterMembershipReadFailureV1::ClusterIdentityMismatch);
        }
        if row.authority_digest != request.expected_authority_digest {
            return Err(ClusterMembershipReadFailureV1::AuthorityDigestMismatch);
        }
        let Ok(expected_ordinal) = u32::try_from(index) else {
            return Err(ClusterMembershipReadFailureV1::NonCanonicalMemberOrder);
        };
        if row.ordinal != expected_ordinal {
            return Err(ClusterMembershipReadFailureV1::NonCanonicalMemberOrder);
        }
        if committed_count
            .replace(row.member_count)
            .is_some_and(|prior| prior != row.member_count)
            || committed_digest
                .replace(row.membership_digest.clone())
                .is_some_and(|prior| prior != row.membership_digest)
        {
            return Err(ClusterMembershipReadFailureV1::CorruptSidecar);
        }
        members.push(SymbolId::new(row.member_symbol_id));
    }
    let computed_digest = cluster_membership_content_digest_v1(&members);
    let Ok(member_count) = u32::try_from(members.len()) else {
        return Err(ClusterMembershipReadFailureV1::CorruptSidecar);
    };
    if committed_count != Some(member_count)
        || committed_digest.as_deref() != Some(computed_digest.as_str())
    {
        return Err(ClusterMembershipReadFailureV1::CorruptSidecar);
    }
    match first_canonical_order_break_v1(&members, |member| member.as_str()) {
        Some(CanonicalOrderBreakV1::Duplicate) => {
            Err(ClusterMembershipReadFailureV1::DuplicateMemberIdentity)
        }
        Some(CanonicalOrderBreakV1::OutOfOrder) => {
            Err(ClusterMembershipReadFailureV1::NonCanonicalMemberOrder)
        }
        None => Ok(members),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SemanticSearchHit {
    pub(crate) candidate: LexicalCandidate,
    pub(crate) record_id: String,
    pub(crate) owner_id: String,
    pub(crate) owner_kind: String,
    pub(crate) corpus_kind: String,
    pub(crate) authority_digest: String,
}

/// Open a sealed generation directly from durable state, failing closed on any
/// absent marker, quarantine, scope mismatch, or shape mismatch.
///
/// This is the cheap door (QI-BB-017): it reads the sealed marker, refuses
/// a generation the scrub quarantined, proves the sealed manifest's layout
/// (every dataset file present at its committed length, the two sidecars
/// hashed), decodes the scope manifest and the build contract, opens the
/// tables, and checks their schemas, row counts and the dense lane's index
/// against the seal. It reads no dataset payload byte; the scrub does.
pub(crate) async fn open_generation(
    semantic_root: &Path,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    session: Option<Arc<Session>>,
) -> Result<LoadedGeneration, CoreError> {
    let (loaded, _manifest) = open_generation_with_manifest(
        semantic_root,
        repo,
        revision,
        generation,
        ReadSession::Existing(session),
        None,
        crate::qualified_fs::LanceUriMode::File,
    )
    .await?;
    Ok(loaded)
}

#[cfg(feature = "qualified-memory-uri-candidate")]
pub(crate) async fn open_generation_admitted(
    semantic_root: &Path,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    session: ReadSession,
    control_lifetime: &dyn PublicationObjectLifetimePort,
) -> Result<LoadedGeneration, CoreError> {
    let (loaded, _manifest) = open_generation_with_manifest(
        semantic_root,
        repo,
        revision,
        generation,
        session,
        Some(control_lifetime),
        crate::qualified_fs::LanceUriMode::AdmittedMemoryCandidate,
    )
    .await?;
    Ok(loaded)
}

/// Open the generation and retain the exact manifest validated by that open.
async fn open_generation_with_manifest(
    semantic_root: &Path,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    session: ReadSession,
    control_lifetime: Option<&dyn PublicationObjectLifetimePort>,
    mode: crate::qualified_fs::LanceUriMode,
) -> Result<(LoadedGeneration, SemanticManifest), CoreError> {
    let generation_dir = layout::generation_dir(semantic_root, repo, revision, generation);
    let marker_path = layout::sealed_marker_path(&generation_dir);
    if !crate::control_file::regular_file_present(&marker_path).map_err(|error| {
        CoreError::Storage(format!(
            "semantic: inspect sealed marker {}: {error}",
            marker_path.display()
        ))
    })? {
        return Err(CoreError::NotReady(format!(
            "semantic: generation {} for repo={} revision={} is not sealed (or absent)",
            generation.get(),
            repo.as_str(),
            revision.as_str()
        )));
    }
    refuse_if_quarantined(&generation_dir, control_lifetime)?;
    // The marker carries the identity digest, which is all the sealed
    // manifest needs: every dataset file, the scope manifest and the build
    // contract are proven against the seal before the scope manifest is
    // even decoded, so a forged manifest is refused as a corrupt sidecar
    // rather than interpreted.
    let sealed_digest = crate::control_file::read_string_for_generation(
        &marker_path,
        crate::control_file::MAX_SEALED_MARKER_BYTES,
        control_lifetime,
    )
    .map_err(|err| crate::errors::fs_err("read sealed marker", &marker_path, &err))?;
    let sealed_manifest =
        verify_sealed_manifest(&generation_dir, &sealed_digest, control_lifetime)?;
    let manifest = read_scope_manifest(&generation_dir, control_lifetime)?;
    manifest.validate_scope(repo, revision, generation)?;
    if manifest.manifest_digest != sealed_digest {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
            message: format!(
                "semantic: sealed marker says {sealed_digest} but the manifest says {} for generation {}",
                manifest.manifest_digest,
                generation.get()
            ),
        });
    }
    manifest.validate_against(&load_generation_contract(
        &generation_dir,
        control_lifetime,
    )?)?;
    let normalization = manifest.normalization_contract()?;

    let dimension = usize::try_from(manifest.dimension).map_err(|err| {
        CoreError::Storage(format!("semantic: manifest dimension overflow: {err}"))
    })?;
    let manifest_dimension_i32 = i32::try_from(manifest.dimension).map_err(|err| {
        CoreError::Storage(format!(
            "semantic: manifest dimension {} does not fit i32 for schema validation: {err}",
            manifest.dimension
        ))
    })?;
    let expected_schema = layout::semantic_schema(manifest_dimension_i32);

    // Verify the sealed directory without following a replacement link. The
    // qualified session binds later opens to the committed dataset artifacts.
    let dataset_dir = layout::dataset_dir(&generation_dir);
    let dataset_kind = std::fs::symlink_metadata(&dataset_dir).map_err(|error| {
        CoreError::Storage(format!(
            "semantic: inspect sealed dataset directory {}: {error}",
            dataset_dir.display()
        ))
    })?;
    if !dataset_kind.file_type().is_dir() {
        return Err(CoreError::Storage(format!(
            "semantic: sealed dataset path is not a directory: {}",
            dataset_dir.display()
        )));
    }
    let resident_bytes_estimate = sealed_manifest.dataset_bytes();
    let session = session.resolve(&dataset_dir, sealed_manifest.artifacts)?;
    let table_path = dataset_dir.join(format!("{TABLE_NAME}.lance"));
    let table = open_sealed_dataset(
        &table_path,
        mode,
        Arc::clone(&session),
        "open_table semantic",
    )
    .await?;
    let live_schema = arrow_schema::Schema::from(table.schema());
    for expected_field in expected_schema.fields() {
        let live_field = live_schema
            .field_with_name(expected_field.name())
            .map_err(|err| {
                CoreError::Storage(format!(
                    "semantic: table missing expected column `{}`: {err}",
                    expected_field.name(),
                ))
            })?;
        if live_field.data_type() != expected_field.data_type() {
            return Err(CoreError::Storage(format!(
                "semantic: table column `{}` type {:?} does not match expected {:?}",
                expected_field.name(),
                live_field.data_type(),
                expected_field.data_type(),
            )));
        }
        if *expected_field.data_type() == DataType::Boolean && live_field.is_nullable() {
            return Err(CoreError::Storage(format!(
                "semantic: table boolean column `{}` unexpectedly nullable",
                expected_field.name(),
            )));
        }
    }

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

    let vector_index =
        verify_vector_index_dataset_v1(&table, &manifest.vector_index, manifest.row_count).await?;

    let membership_path = dataset_dir.join(format!("{CLUSTER_MEMBERSHIP_TABLE_NAME}.lance"));
    let membership_kind = match std::fs::symlink_metadata(&membership_path) {
        Ok(metadata) => Some(metadata.file_type()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(crate::errors::fs_err(
                "inspect sealed cluster membership table",
                &membership_path,
                &error,
            ));
        }
    };
    if !membership_kind.is_some_and(|kind| kind.is_dir()) {
        return Err(CoreError::Storage(
            "semantic: the sealed generation carries no cluster membership table".to_string(),
        ));
    }
    let membership_table = open_sealed_dataset(
        &membership_path,
        mode,
        session,
        "open cluster membership table",
    )
    .await?;
    let membership_schema = arrow_schema::Schema::from(membership_table.schema());
    if !layout::cluster_membership_schema()
        .fields()
        .iter()
        .all(|expected| {
            membership_schema
                .field_with_name(expected.name())
                .is_ok_and(|live| live.data_type() == expected.data_type() && !live.is_nullable())
        })
    {
        return Err(CoreError::Storage(
            "semantic: the cluster membership table has an incompatible schema".to_string(),
        ));
    }
    // With the file layout verified against the seal, the membership rows
    // are the sealed rows; only the cheap count is re-checked here and the
    // root is never re-derived on the serving path.
    verify_cluster_membership_row_count_v1(&membership_table, &manifest).await?;

    Ok((
        LoadedGeneration {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
            dimension,
            sealed_row_count: manifest.row_count,
            normalization,
            model_id: manifest.model_id.clone(),
            model_version: manifest.model_version.clone(),
            table,
            cluster_membership: membership_table,
            vector_index,
            resident_bytes_estimate,
            manifest_digest: sealed_digest,
        },
        manifest,
    ))
}

async fn open_sealed_dataset(
    path: &Path,
    mode: crate::qualified_fs::LanceUriMode,
    session: Arc<Session>,
    action: &str,
) -> Result<Dataset, CoreError> {
    let uri = crate::qualified_fs::lance_connection_uri(path, mode)?;
    DatasetBuilder::from_uri(&uri)
        .with_session(session)
        .load()
        .await
        .map_err(|error| lancedb_err(action, error))
}

/// The inherited ANN contract and canonical row identities from one
/// fully verified sealed base generation.
pub(crate) struct VerifiedInheritedVectorIndexV1 {
    pub(crate) seal: VectorIndexSealV1,
    pub(crate) row_fingerprints: Vec<SemanticRowFingerprintV1>,
}

/// Read the verified vector seal of a delta's immutable base.
///
/// The normal open checks sealed sidecars, dataset layout, schema, row count
/// and index identity. Recomputing the canonical row commitment also binds
/// the payload to the sealed row root before any append or retrain decision.
/// The caller holds the adapter's generation and lifecycle guards, so this
/// private function must not acquire them again.
pub(crate) async fn verified_base_vector_index_seal_v1(
    semantic_root: &Path,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    session: Option<Arc<Session>>,
    control_lifetime: Option<&dyn PublicationObjectLifetimePort>,
    mode: crate::qualified_fs::LanceUriMode,
) -> Result<VerifiedInheritedVectorIndexV1, CoreError> {
    let (loaded, manifest) = open_generation_with_manifest(
        semantic_root,
        repo,
        revision,
        generation,
        ReadSession::Existing(session),
        control_lifetime,
        mode,
    )
    .await?;
    let actual = dataset_row_commitment_v1(&loaded.table).await?;
    let row_count_matches = actual.row_count == manifest.row_count;
    let row_root_matches = actual.root_digest == manifest.semantic_row_root_digest;
    if !row_count_matches || !row_root_matches {
        return Err(CoreError::Storage(format!(
            "semantic: sealed base row commitment differs before successor seal: rows {} versus {}, root {} versus {}",
            actual.row_count,
            manifest.row_count,
            actual.root_digest,
            manifest.semantic_row_root_digest
        )));
    }
    Ok(VerifiedInheritedVectorIndexV1 {
        seal: manifest.vector_index,
        row_fingerprints: actual.row_fingerprints,
    })
}

/// Sum of regular-file sizes under `root`, recursively.
///
/// `LanceDB` maps the dataset's files on demand, so their on-disk size is the
/// honest upper bound on what one open handle can make resident.
pub(crate) fn dataset_tree_bytes(track_root: &Path, root: &Path) -> Result<u64, CoreError> {
    dataset_tree_bytes_observed(track_root, root, None)
}

pub(crate) fn dataset_tree_bytes_observed(
    track_root: &Path,
    root: &Path,
    diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
) -> Result<u64, CoreError> {
    quanta_index_core::unique_inode_tree_bytes_below_track(
        track_root,
        root,
        &|_name| false,
        diagnostic,
    )
    .map_err(|error| {
        CoreError::Storage(format!(
            "semantic: measure dataset dir {}: {error}",
            root.display()
        ))
    })
}

/// Downcast a named column to a concrete Arrow array type.
///
/// Fails closed with the same message shape for both the missing-column and
/// wrong-type cases. `arrow_type` is the human-readable expected type name used
/// in the error.
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

async fn verify_cluster_membership_row_count_v1(
    table: &Dataset,
    manifest: &SemanticManifest,
) -> Result<(), CoreError> {
    let counted_rows = table
        .count_rows(None)
        .await
        .map_err(|error| lancedb_err("count cluster membership rows at open", error))?;
    let counted_rows_u64 = u64::try_from(counted_rows).map_err(|error| {
        CoreError::Storage(format!(
            "semantic: cluster membership row count overflow at open: {error}"
        ))
    })?;
    if counted_rows_u64 != manifest.cluster_membership_member_row_count {
        return Err(CoreError::Storage(format!(
            "semantic: cluster membership row count {counted_rows_u64} != sealed count {}",
            manifest.cluster_membership_member_row_count
        )));
    }
    Ok(())
}

/// Convert a lancedb cosine *distance* to the query contract's cosine
/// *similarity* (`1 - distance`), failing closed on a non-finite distance.
///
/// A NaN/Inf distance signals a corrupt index or a degenerate *stored* vector
/// (not a bad query vector), so it is reported as `CoreError::Storage` — the same
/// channel as the sibling lancedb integrity checks (missing column, row-count
/// mismatch) — rather than `SEM_INVALID_VECTOR`, which means a bad *query* vector
/// everywhere else. Letting it propagate would seed a NaN score into ranked
/// output and `semantic_score_raw`. A non-finite distance aborts the entire query
/// (not just the row), by design: a corrupt generation must not serve partial
/// ranked output (R-SAFE-03 fail-closed).
fn cosine_distance_to_score_v1(distance: f32, candidate_id: &str) -> Result<f32, CoreError> {
    if !distance.is_finite() {
        return Err(CoreError::Storage(format!(
            "semantic: non-finite cosine distance {distance} from lancedb for candidate {candidate_id} (corrupt index or degenerate stored vector)"
        )));
    }
    // Lancedb returns cosine *distance* in [0, 2]; the historical query contract
    // is cosine *similarity* in [-1, 1] (higher = better).
    Ok(1.0_f32 - distance)
}

fn parse_owner_kind_v1(value: &str, record_id: &str) -> Result<OwnerDocKind, CoreError> {
    OwnerDocKind::from_code_str(value).ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: unsupported owner_kind {value:?} on record {record_id}"
        ))
    })
}

/// Read one result batch into hits, asking `probe` before every row so
/// the request budget is observed while rows are read back, not only
/// around the query.
fn extract_hits(
    batch: &RecordBatch,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    generation: ManifestGeneration,
    probe: &mut RowBudgetProbe<'_>,
    out: &mut Vec<SemanticSearchHit>,
) -> Result<(), CoreError> {
    let id_col = column_as::<StringArray>(batch, COLUMN_EMBEDDING_ID, "Utf8")?;
    let source_repo_col = column_as::<StringArray>(batch, COLUMN_SOURCE_REPO_ID, "Utf8")?;
    let path_col = column_as::<StringArray>(batch, COLUMN_REPO_RELATIVE_PATH, "Utf8")?;
    let start_col = column_as::<UInt32Array>(batch, COLUMN_START_LINE, "UInt32")?;
    let end_col = column_as::<UInt32Array>(batch, COLUMN_END_LINE, "UInt32")?;
    let snippet_col = column_as::<StringArray>(batch, COLUMN_SNIPPET, "Utf8")?;
    let distance_col = column_as::<Float32Array>(batch, COLUMN_DISTANCE, "Float32")?;
    let record_id_col = column_as::<StringArray>(batch, COLUMN_RECORD_ID, "Utf8")?;
    let owner_id_col = column_as::<StringArray>(batch, COLUMN_OWNER_ID, "Utf8")?;
    let owner_kind_col = column_as::<StringArray>(batch, COLUMN_OWNER_KIND, "Utf8")?;
    let corpus_kind_col = column_as::<StringArray>(batch, COLUMN_CORPUS_KIND, "Utf8")?;
    let authority_digest_col = column_as::<StringArray>(batch, COLUMN_AUTHORITY_DIGEST, "Utf8")?;
    for row in 0..batch.num_rows() {
        probe.tick()?;
        let id = id_col.value(row).to_owned();
        let path = path_col.value(row).to_owned();
        let snippet = snippet_col.value(row).to_owned();
        let start_line = start_col.value(row);
        let end_line = end_col.value(row);
        let distance = distance_col.value(row);
        let score = cosine_distance_to_score_v1(distance, &id)?;
        let source_file = SourceFileKey {
            source_repo_id: RepoId::new(source_repo_col.value(row)).map_err(|error| {
                CoreError::Storage(format!(
                    "semantic: invalid source_repo_id on record {id}: {error}"
                ))
            })?,
            repo_relative_path: RepoRelativePath::new(path),
        };
        source_file.validate().map_err(|error| {
            CoreError::Storage(format!(
                "semantic: invalid source path on record {id}: {error}"
            ))
        })?;
        let candidate = LexicalCandidate {
            source_repo_id: source_file.source_repo_id,
            source: None,
            preview: None,
            candidate_id: id.clone(),
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            manifest_generation: generation,
            repo_relative_path: source_file.repo_relative_path,
            lines: Some(quanta_index_contract::LexicalLineRange {
                start_line,
                end_line,
            }),
            excerpt: Some(quanta_index_contract::LexicalMatchExcerpt {
                snippet,
                // Semantic results carry no single lexical hit anchor.
                snippet_hit_offset: None,
                highlights: Vec::new(),
            }),
            score,
            native_source: None,
        };
        out.push(SemanticSearchHit {
            candidate,
            record_id: record_id_col.value(row).to_owned(),
            owner_id: owner_id_col.value(row).to_owned(),
            owner_kind: owner_kind_col.value(row).to_owned(),
            corpus_kind: corpus_kind_col.value(row).to_owned(),
            authority_digest: authority_digest_col.value(row).to_owned(),
        });
    }
    Ok(())
}

fn extract_hit_at(
    batch: &RecordBatch,
    row: usize,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    generation: ManifestGeneration,
    score: f32,
) -> Result<SemanticSearchHit, CoreError> {
    let id = column_as::<StringArray>(batch, COLUMN_EMBEDDING_ID, "Utf8")?
        .value(row)
        .to_owned();
    let path = column_as::<StringArray>(batch, COLUMN_REPO_RELATIVE_PATH, "Utf8")?
        .value(row)
        .to_owned();
    let source_repo = column_as::<StringArray>(batch, COLUMN_SOURCE_REPO_ID, "Utf8")?.value(row);
    let source_file = SourceFileKey {
        source_repo_id: RepoId::new(source_repo).map_err(|error| {
            CoreError::Storage(format!(
                "semantic: invalid source_repo_id on record {id}: {error}"
            ))
        })?,
        repo_relative_path: RepoRelativePath::new(path),
    };
    source_file.validate().map_err(|error| {
        CoreError::Storage(format!(
            "semantic: invalid source path on record {id}: {error}"
        ))
    })?;
    Ok(SemanticSearchHit {
        candidate: LexicalCandidate {
            source_repo_id: source_file.source_repo_id,
            source: None,
            preview: None,
            candidate_id: id,
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            manifest_generation: generation,
            repo_relative_path: source_file.repo_relative_path,
            lines: Some(quanta_index_contract::LexicalLineRange {
                start_line: column_as::<UInt32Array>(batch, COLUMN_START_LINE, "UInt32")?
                    .value(row),
                end_line: column_as::<UInt32Array>(batch, COLUMN_END_LINE, "UInt32")?.value(row),
            }),
            excerpt: Some(quanta_index_contract::LexicalMatchExcerpt {
                snippet: column_as::<StringArray>(batch, COLUMN_SNIPPET, "Utf8")?
                    .value(row)
                    .to_owned(),
                snippet_hit_offset: None,
                highlights: Vec::new(),
            }),
            score,
            native_source: None,
        },
        record_id: column_as::<StringArray>(batch, COLUMN_RECORD_ID, "Utf8")?
            .value(row)
            .to_owned(),
        owner_id: column_as::<StringArray>(batch, COLUMN_OWNER_ID, "Utf8")?
            .value(row)
            .to_owned(),
        owner_kind: column_as::<StringArray>(batch, COLUMN_OWNER_KIND, "Utf8")?
            .value(row)
            .to_owned(),
        corpus_kind: column_as::<StringArray>(batch, COLUMN_CORPUS_KIND, "Utf8")?
            .value(row)
            .to_owned(),
        authority_digest: column_as::<StringArray>(batch, COLUMN_AUTHORITY_DIGEST, "Utf8")?
            .value(row)
            .to_owned(),
    })
}

impl LoadedGeneration {
    fn cluster_membership_rejection_v1(
        &self,
        request: &ClusterMembershipReadRequestV1,
        failure: ClusterMembershipReadFailureV1,
    ) -> ClusterMembershipReadOutcomeV1 {
        ClusterMembershipReadOutcomeV1::Rejected(ClusterMembershipReadRejectionV1 {
            cluster_record_id: request.cluster_record_id.clone(),
            generation: request.generation.clone(),
            expected_authority_digest: request.expected_authority_digest.clone(),
            failure,
        })
    }

    fn cluster_membership_outcome_from_rows_v1(
        &self,
        request: &ClusterMembershipReadRequestV1,
        expected_pin: &GenerationPin,
        rows: Vec<ClusterMembershipReadRowV1>,
    ) -> ClusterMembershipReadOutcomeV1 {
        let mut members = match validate_cluster_membership_rows_v1(rows, request) {
            Ok(members) => members,
            Err(failure) => return self.cluster_membership_rejection_v1(request, failure),
        };
        let limit = match usize::try_from(request.limit) {
            Ok(limit) => limit,
            // A `u32` limit wider than this platform's `usize` cannot truncate
            // a result set that already fits in memory.
            Err(_too_wide_for_platform) => usize::MAX,
        };
        let completeness = if members.len() > limit {
            members.truncate(limit);
            ClusterMembershipCompletenessV1::Truncated
        } else {
            ClusterMembershipCompletenessV1::Complete
        };
        let snapshot = ClusterMembershipSnapshotV1 {
            cluster_record_id: request.cluster_record_id.clone(),
            generation: expected_pin.clone(),
            authority_digest: request.expected_authority_digest.clone(),
            members,
            completeness,
        };
        match snapshot.validate_against_v1(request) {
            Ok(()) => ClusterMembershipReadOutcomeV1::Available(snapshot),
            Err(failure) => self.cluster_membership_rejection_v1(request, failure),
        }
    }

    fn cluster_membership_batch_rejection_v1(
        &self,
        request: &ClusterMembershipBatchReadRequestV1,
        failure: ClusterMembershipReadFailureV1,
    ) -> ClusterMembershipBatchReadResponseV1 {
        ClusterMembershipBatchReadResponseV1 {
            outcomes: request
                .items
                .iter()
                .map(|item| {
                    self.cluster_membership_rejection_v1(
                        &item.as_single_request_v1(&request.generation),
                        failure,
                    )
                })
                .collect(),
        }
    }

    async fn cluster_membership_batch_read_async(
        &self,
        request: &ClusterMembershipBatchReadRequestV1,
    ) -> Result<ClusterMembershipBatchReadResponseV1, CoreError> {
        request
            .validate_v1()
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        let expected_pin = GenerationPin::new(
            self.repo_id.clone(),
            self.revision_id.clone(),
            self.generation,
        );
        if request.generation != expected_pin {
            return Ok(self.cluster_membership_batch_rejection_v1(
                request,
                ClusterMembershipReadFailureV1::GenerationMismatch,
            ));
        }
        let table = &self.cluster_membership;
        let mut predicate = format!("{COLUMN_MEMBERSHIP_CLUSTER_RECORD_ID} IN (");
        for (index, item) in request.items.iter().enumerate() {
            if index > 0 {
                predicate.push_str(", ");
            }
            predicate.push_str(&crate::sql::quote_sql_string(
                item.cluster_record_id.as_str(),
            ));
        }
        predicate.push(')');
        let maximum_rows = usize::try_from(quanta_index_contract::MAX_CLUSTER_MEMBERSHIP_READ_V1)
            .map_err(|error| {
                CoreError::InvalidContract(format!(
                    "semantic: cluster membership read cap does not fit this platform: {error}"
                ))
            })?
            .checked_mul(request.items.len())
            .and_then(|rows| rows.checked_add(1))
            .ok_or_else(|| {
                CoreError::InvalidContract(
                    "semantic: cluster membership batch row budget overflow".to_string(),
                )
            })?;
        let scan_limit = i64::try_from(maximum_rows).map_err(|error| {
            CoreError::InvalidContract(format!(
                "semantic: cluster membership batch row budget overflow: {error}"
            ))
        })?;
        let opened = async {
            let mut scan = table.scan();
            scan.filter(&predicate)?
                .limit(Some(scan_limit), None)?
                .batch_size(DATASET_READ_BATCH_ROWS)
                .strict_batch_size(true)
                .try_into_stream()
                .await
        }
        .await;
        let mut stream = match opened {
            Ok(stream) => stream,
            Err(_error) => {
                return Ok(self.cluster_membership_batch_rejection_v1(
                    request,
                    ClusterMembershipReadFailureV1::CorruptSidecar,
                ));
            }
        };
        let mut observed_rows = 0_usize;
        let mut rows_by_cluster: BTreeMap<String, Vec<ClusterMembershipReadRowV1>> =
            BTreeMap::new();
        loop {
            let batch = match stream.try_next().await {
                Ok(Some(batch)) => batch,
                Ok(None) => break,
                Err(_error) => {
                    return Ok(self.cluster_membership_batch_rejection_v1(
                        request,
                        ClusterMembershipReadFailureV1::CorruptSidecar,
                    ));
                }
            };
            observed_rows = match observed_rows.checked_add(batch.num_rows()) {
                Some(rows) if rows <= maximum_rows => rows,
                _ => {
                    return Ok(self.cluster_membership_batch_rejection_v1(
                        request,
                        ClusterMembershipReadFailureV1::MemberLimitExceeded,
                    ));
                }
            };
            if batch
                .columns()
                .iter()
                .any(|column| column.null_count() != 0)
            {
                return Ok(self.cluster_membership_batch_rejection_v1(
                    request,
                    ClusterMembershipReadFailureV1::CorruptSidecar,
                ));
            }
            let cluster_id =
                match column_as::<StringArray>(&batch, COLUMN_MEMBERSHIP_CLUSTER_RECORD_ID, "Utf8")
                {
                    Ok(column) => column,
                    Err(_error) => {
                        return Ok(self.cluster_membership_batch_rejection_v1(
                            request,
                            ClusterMembershipReadFailureV1::CorruptSidecar,
                        ));
                    }
                };
            let authority_digest = match column_as::<StringArray>(
                &batch,
                COLUMN_MEMBERSHIP_AUTHORITY_DIGEST,
                "Utf8",
            ) {
                Ok(column) => column,
                Err(_error) => {
                    return Ok(self.cluster_membership_batch_rejection_v1(
                        request,
                        ClusterMembershipReadFailureV1::CorruptSidecar,
                    ));
                }
            };
            let member_symbol_id = match column_as::<StringArray>(
                &batch,
                COLUMN_MEMBERSHIP_MEMBER_SYMBOL_ID,
                "Utf8",
            ) {
                Ok(column) => column,
                Err(_error) => {
                    return Ok(self.cluster_membership_batch_rejection_v1(
                        request,
                        ClusterMembershipReadFailureV1::CorruptSidecar,
                    ));
                }
            };
            let ordinal =
                match column_as::<UInt32Array>(&batch, COLUMN_MEMBERSHIP_ORDINAL, "UInt32") {
                    Ok(column) => column,
                    Err(_error) => {
                        return Ok(self.cluster_membership_batch_rejection_v1(
                            request,
                            ClusterMembershipReadFailureV1::CorruptSidecar,
                        ));
                    }
                };
            let member_count =
                match column_as::<UInt32Array>(&batch, COLUMN_MEMBERSHIP_MEMBER_COUNT, "UInt32") {
                    Ok(column) => column,
                    Err(_error) => {
                        return Ok(self.cluster_membership_batch_rejection_v1(
                            request,
                            ClusterMembershipReadFailureV1::CorruptSidecar,
                        ));
                    }
                };
            let membership_digest =
                match column_as::<StringArray>(&batch, COLUMN_MEMBERSHIP_CONTENT_DIGEST, "Utf8") {
                    Ok(column) => column,
                    Err(_error) => {
                        return Ok(self.cluster_membership_batch_rejection_v1(
                            request,
                            ClusterMembershipReadFailureV1::CorruptSidecar,
                        ));
                    }
                };
            for row in 0..batch.num_rows() {
                let cluster_record_id = cluster_id.value(row).to_owned();
                rows_by_cluster
                    .entry(cluster_record_id.clone())
                    .or_default()
                    .push(ClusterMembershipReadRowV1 {
                        ordinal: ordinal.value(row),
                        cluster_record_id,
                        authority_digest: authority_digest.value(row).to_owned(),
                        member_symbol_id: member_symbol_id.value(row).to_owned(),
                        member_count: member_count.value(row),
                        membership_digest: membership_digest.value(row).to_owned(),
                    });
            }
        }

        if rows_by_cluster.keys().any(|record_id| {
            request
                .items
                .binary_search_by(|item| item.cluster_record_id.as_str().cmp(record_id.as_str()))
                .is_err()
        }) {
            return Ok(self.cluster_membership_batch_rejection_v1(
                request,
                ClusterMembershipReadFailureV1::CorruptSidecar,
            ));
        }

        let outcomes = request
            .items
            .iter()
            .map(|item| {
                let single = item.as_single_request_v1(&request.generation);
                let rows = rows_by_cluster
                    .remove(item.cluster_record_id.as_str())
                    .unwrap_or_default();
                self.cluster_membership_outcome_from_rows_v1(&single, &expected_pin, rows)
            })
            .collect();
        let response = ClusterMembershipBatchReadResponseV1 { outcomes };
        response.validate_against_v1(request).map_err(|failure| {
            CoreError::InvalidContract(format!(
                "semantic: invalid cluster membership batch response: {failure}"
            ))
        })?;
        Ok(response)
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn model_version(&self) -> Option<&str> {
        self.model_version.as_deref()
    }

    fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    /// Hold a query vector to the contract the generation's rows were held
    /// to at ingest (QI-BB-031): its dimension (`SEM_DIM_MISMATCH`), then
    /// finite components, a non-zero norm and, under `L2Unit`, a unit norm
    /// (`SEM_INVALID_VECTOR`) — the one validator every ingested row passed.
    fn validate_query_vector(&self, query_vector: &[f32]) -> Result<(), CoreError> {
        if query_vector.len() != self.dimension {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::SemDimMismatch.into(),
                message: format!(
                    "semantic: query vector dim {} does not match index dim {} for generation {}",
                    query_vector.len(),
                    self.dimension,
                    self.generation.get()
                ),
            });
        }
        SemanticPolicy::validate_embedding_vector_v1(
            query_vector,
            self.dimension,
            self.normalization,
        )
    }

    /// Build the lane's vector query and run it under the request budget
    /// (W5 phase 3): refused before issue, dropped in flight, or stopped
    /// between rows, whichever the budget reaches first — see
    /// [`crate::budget`].
    /// The top `top_k` rows of the scope `filter` names, nearest first:
    /// always `min(top_k, rows in scope)` of them.
    ///
    /// An approximate pass that returns fewer rows than asked proves
    /// nothing about the rows it did not return — a graph walk can leave
    /// rows unreached, however large its effort. The scope's own count
    /// decides: if it holds more than the pass returned, the exact lane,
    /// the answer the index approximates, answers the same query
    /// (QI-BB-025: a page at `top_k = 10_000` over 10,001 rows came back
    /// 9,695 rows long and was read as the whole scope).
    async fn run_vector_query(
        &self,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<String>,
        watch: DenseLaneBudgetV1<'_>,
    ) -> Result<Vec<SemanticSearchHit>, CoreError> {
        let lane = self.vector_index.lane_kind();
        let hits = self
            .run_lane(query_vector, top_k, filter.clone(), watch, lane)
            .await?;
        if lane == DenseLaneKindV1::Exact || hits.len() >= top_k {
            return Ok(hits);
        }
        let in_scope = race_with_budget(watch, lane, async {
            self.table
                .count_rows(filter.clone())
                .await
                .map_err(|err| lancedb_err("count rows in scope", err))
        })
        .await?;
        if hits.len() >= in_scope.min(top_k) {
            return Ok(hits);
        }
        watch.tallies.count_exact_completion();
        self.run_lane(query_vector, top_k, filter, watch, DenseLaneKindV1::Exact)
            .await
    }

    /// One pass through `lane`: the sealed effort pinned for the approximate
    /// lane — it probes, walks and refines exactly as its seal recorded —
    /// or every index bypassed for the exact lane (QI-BB-027).
    async fn run_lane(
        &self,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<String>,
        watch: DenseLaneBudgetV1<'_>,
        lane: DenseLaneKindV1,
    ) -> Result<Vec<SemanticSearchHit>, CoreError> {
        race_with_budget(watch, lane, async {
            let limit = i64::try_from(top_k).map_err(|error| {
                CoreError::InvalidContract(format!(
                    "semantic: vector search limit overflow: {error}"
                ))
            })?;
            let key = Float32Array::from(query_vector.to_vec());
            let mut scan = self.table.scan();
            let scan = scan
                .nearest(COLUMN_VECTOR, &key, top_k)
                .map_err(|error| lancedb_err("vector_search build", error))?
                .limit(Some(limit), None)
                .map_err(|error| lancedb_err("vector_search execute", error))?
                .distance_metric(LanceDistanceType::Cosine)
                .prefilter(true)
                .batch_size(DATASET_READ_BATCH_ROWS)
                .strict_batch_size(true);
            match lane {
                DenseLaneKindV1::Approximate => self.vector_index.apply_scanner(scan, top_k)?,
                DenseLaneKindV1::Exact => {
                    let _ = scan.use_index(false);
                }
            }
            if let Some(predicate) = filter {
                let _filtered = scan
                    .filter(&predicate)
                    .map_err(|error| lancedb_err("vector_search execute", error))?;
            }
            self.issue_and_read_back(scan, top_k, watch, lane).await
        })
        .await
    }

    /// Scan every eligible row from one pinned Lance dataset before the Top-K
    /// cut. The retained heap never holds more than K hits.
    async fn run_canonical_exact_scan(
        &self,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<String>,
        watch: DenseLaneBudgetV1<'_>,
    ) -> Result<Vec<SemanticSearchHit>, CoreError> {
        let lane = DenseLaneKindV1::Exact;
        race_with_budget(watch, lane, async {
            failpoint::hold_dense_lane_if_armed().await;
            watch.tallies.count_query(lane);
            let mut scan = self.table.scan();
            let scan = scan
                .project(&[
                    COLUMN_VECTOR,
                    COLUMN_EMBEDDING_ID,
                    COLUMN_SOURCE_REPO_ID,
                    COLUMN_REPO_RELATIVE_PATH,
                    COLUMN_START_LINE,
                    COLUMN_END_LINE,
                    COLUMN_SNIPPET,
                    COLUMN_RECORD_ID,
                    COLUMN_OWNER_ID,
                    COLUMN_OWNER_KIND,
                    COLUMN_CORPUS_KIND,
                    COLUMN_AUTHORITY_DIGEST,
                ])
                .map_err(|error| lancedb_err("canonical exact scan execute", error))?
                .with_row_id()
                .batch_size(EXACT_SCAN_BATCH_ROWS)
                .strict_batch_size(true);
            if let Some(predicate) = filter {
                let _filtered = scan
                    .filter(&predicate)
                    .map_err(|error| lancedb_err("canonical exact scan execute", error))?;
            }
            let mut stream = scan
                .try_into_stream()
                .await
                .map_err(|error| lancedb_err("canonical exact scan execute", error))?;
            let key = Float32Array::from(query_vector.to_vec());
            let mut probe = RowBudgetProbe::new(watch, lane);
            let mut heap = ExactTieHeap::new(top_k)?;
            while let Some(batch) = stream
                .try_next()
                .await
                .map_err(|error| lancedb_err("canonical exact scan stream", error))?
            {
                if batch.num_rows() > EXACT_SCAN_BATCH_ROWS {
                    return Err(CoreError::Storage(
                        "semantic: canonical exact scan exceeded output batch limit".into(),
                    ));
                }
                let vectors =
                    column_as::<FixedSizeListArray>(&batch, COLUMN_VECTOR, "FixedSizeList")?;
                if usize::try_from(vectors.value_length()).ok() != Some(query_vector.len()) {
                    return Err(CoreError::Storage(
                        "semantic: canonical exact scan vector dimension mismatch".into(),
                    ));
                }
                let distances = LanceDistanceType::Cosine.arrow_batch_func()(&key, vectors)
                    .map_err(|error| {
                        CoreError::Storage(format!(
                            "semantic: canonical exact scan distance: {error}"
                        ))
                    })?;
                let ids = column_as::<StringArray>(&batch, COLUMN_EMBEDDING_ID, "Utf8")?;
                let repos = column_as::<StringArray>(&batch, COLUMN_SOURCE_REPO_ID, "Utf8")?;
                let paths = column_as::<StringArray>(&batch, COLUMN_REPO_RELATIVE_PATH, "Utf8")?;
                let starts = column_as::<UInt32Array>(&batch, COLUMN_START_LINE, "UInt32")?;
                let ends = column_as::<UInt32Array>(&batch, COLUMN_END_LINE, "UInt32")?;
                let row_ids = column_as::<UInt64Array>(&batch, COLUMN_ROW_ID, "UInt64")?;
                for row in 0..batch.num_rows() {
                    probe.tick()?;
                    if vectors.is_null(row)
                        || distances.is_null(row)
                        || ids.is_null(row)
                        || repos.is_null(row)
                        || paths.is_null(row)
                        || starts.is_null(row)
                        || ends.is_null(row)
                        || row_ids.is_null(row)
                    {
                        return Err(CoreError::Storage(
                            "semantic: canonical exact scan required column is null".into(),
                        ));
                    }
                    let score = cosine_distance_to_score_v1(distances.value(row), ids.value(row))?;
                    let candidate_key = ExactTieKey {
                        score,
                        source_repo_id: repos.value(row).to_owned(),
                        repo_relative_path: paths.value(row).to_owned(),
                        start_line: starts.value(row),
                        end_line: ends.value(row),
                        embedding_id: ids.value(row).to_owned(),
                        row_id: row_ids.value(row),
                    };
                    let _selected = heap.consider_try_with(candidate_key, || {
                        extract_hit_at(
                            &batch,
                            row,
                            &self.repo_id,
                            &self.revision_id,
                            self.generation,
                            score,
                        )
                    })?;
                }
            }
            Ok(heap.into_best().into_iter().map(|(_, hit)| hit).collect())
        })
        .await
    }

    /// Issue the query and read its rows back one batch at a time, ticking
    /// the row probe; the first `top_k` rows end the read and drop the
    /// stream there.
    async fn issue_and_read_back(
        &self,
        scan: &Scanner,
        top_k: usize,
        watch: DenseLaneBudgetV1<'_>,
        lane: DenseLaneKindV1,
    ) -> Result<Vec<SemanticSearchHit>, CoreError> {
        failpoint::hold_dense_lane_if_armed().await;
        watch.tallies.count_query(lane);
        let mut stream = scan
            .try_into_stream()
            .await
            .map_err(|err| lancedb_err("vector_search execute", err))?;
        let mut probe = RowBudgetProbe::new(watch, lane);
        let mut out: Vec<SemanticSearchHit> = Vec::with_capacity(top_k);
        while let Some(batch) = stream
            .try_next()
            .await
            .map_err(|err| lancedb_err("vector_search stream", err))?
        {
            extract_hits(
                &batch,
                &self.repo_id,
                &self.revision_id,
                self.generation,
                &mut probe,
                &mut out,
            )?;
            if out.len() >= top_k {
                break;
            }
        }
        out.truncate(top_k);
        Ok(out)
    }

    fn map_hits_to_candidates(hits: Vec<SemanticSearchHit>) -> Vec<LexicalCandidate> {
        hits.into_iter().map(|hit| hit.candidate).collect()
    }

    fn map_hits_to_core_v1(
        hits: Vec<SemanticSearchHit>,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        hits.into_iter()
            .map(|hit| {
                let owner_kind = parse_owner_kind_v1(&hit.owner_kind, &hit.record_id)?;
                let corpus_kind = SemanticCorpusKindV1::from_code_str(&hit.corpus_kind)
                    .ok_or_else(|| {
                        CoreError::Storage(format!(
                            "semantic: unsupported corpus_kind {:?} on record {}",
                            hit.corpus_kind, hit.record_id
                        ))
                    })?;
                Ok(SemanticSearchHitV1 {
                    candidate: hit.candidate,
                    record_id: hit.record_id,
                    owner_id: hit.owner_id,
                    owner_kind,
                    corpus_kind: Some(corpus_kind),
                    authority_digest: hit.authority_digest,
                })
            })
            .collect()
    }

    fn combine_filters(filters: impl IntoIterator<Item = String>) -> Option<String> {
        let parts: Vec<String> = filters
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" AND "))
        }
    }

    fn language_any_of_filter(constraints: &QueryConstraintSetV1) -> Option<String> {
        if constraints.language_any_of.is_empty() {
            return None;
        }
        let clauses = constraints
            .language_any_of
            .iter()
            .map(|language| {
                format!(
                    "{COLUMN_LANGUAGE} = {}",
                    crate::sql::quote_sql_string(language.as_str())
                )
            })
            .collect::<Vec<_>>();
        Some(format!("({})", clauses.join(" OR ")))
    }

    fn exact_repo_relative_path_filter(constraints: &QueryConstraintSetV1) -> Option<String> {
        constraints.repo_relative_path_exact.as_ref().map(|path| {
            format!(
                "{COLUMN_REPO_RELATIVE_PATH} = {}",
                crate::sql::quote_sql_string(path.as_str())
            )
        })
    }

    async fn search_hits_constrained_async(
        &self,
        query_vector: &[f32],
        top_k: usize,
        allowed_ids: Option<&BTreeSet<String>>,
        corpus_kind: Option<&str>,
        constraints: &QueryConstraintSetV1,
        watch: DenseLaneBudgetV1<'_>,
    ) -> Result<Vec<SemanticSearchHit>, CoreError> {
        self.validate_query_vector(query_vector)?;
        if allowed_ids.is_some_and(BTreeSet::is_empty) {
            return Ok(Vec::new());
        }
        let filter = Self::combine_filters(
            allowed_ids
                .into_iter()
                .map(build_id_in_filter)
                .chain(corpus_kind.into_iter().map(|kind| {
                    format!(
                        "{COLUMN_CORPUS_KIND} = {}",
                        crate::sql::quote_sql_string(kind)
                    )
                }))
                .chain(Self::language_any_of_filter(constraints))
                .chain(Self::exact_repo_relative_path_filter(constraints)),
        );
        self.run_vector_query(query_vector, top_k, filter, watch)
            .await
    }

    pub(crate) async fn search_async(
        &self,
        query_vector: &[f32],
        top_k: usize,
        watch: DenseLaneBudgetV1<'_>,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        self.search_hits_constrained_async(
            query_vector,
            top_k,
            None,
            None,
            &QueryConstraintSetV1::unconstrained(),
            watch,
        )
        .await
        .map(Self::map_hits_to_candidates)
    }

    /// One stored row scored exactly against `query_vector` (QI-BB-022).
    ///
    /// An exact-lane vector query filtered to the candidate's own id: the
    /// sealed approximate index, if any, is bypassed, so the distance is the
    /// stored vector's own cosine and never a neighbour search's recall.
    /// `None` when the generation stores no row under that id.
    pub(crate) async fn score_candidate_async(
        &self,
        candidate_id: &str,
        query_vector: &[f32],
        watch: DenseLaneBudgetV1<'_>,
    ) -> Result<Option<f32>, CoreError> {
        self.validate_query_vector(query_vector)?;
        let mut allowed_ids = BTreeSet::new();
        let _inserted = allowed_ids.insert(candidate_id.to_owned());
        let hits = self
            .run_lane(
                query_vector,
                1,
                Some(build_id_in_filter(&allowed_ids)),
                watch,
                DenseLaneKindV1::Exact,
            )
            .await?;
        let Some(hit) = hits.into_iter().next() else {
            return Ok(None);
        };
        if hit.candidate.candidate_id != candidate_id {
            return Err(CoreError::Storage(format!(
                "semantic: exact lookup of {candidate_id} answered row {}",
                hit.candidate.candidate_id
            )));
        }
        Ok(Some(hit.candidate.score))
    }

    pub(crate) async fn search_scoped_async(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        top_k: usize,
        watch: DenseLaneBudgetV1<'_>,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        self.search_hits_constrained_async(
            query_vector,
            top_k,
            Some(allowed_ids),
            None,
            &QueryConstraintSetV1::unconstrained(),
            watch,
        )
        .await
        .map(Self::map_hits_to_candidates)
    }

    pub(crate) async fn search_hits_filtered_async(
        &self,
        query_vector: &[f32],
        top_k: usize,
        corpus_kind: Option<&str>,
        watch: DenseLaneBudgetV1<'_>,
    ) -> Result<Vec<SemanticSearchHit>, CoreError> {
        self.search_hits_constrained_async(
            query_vector,
            top_k,
            None,
            corpus_kind,
            &QueryConstraintSetV1::unconstrained(),
            watch,
        )
        .await
    }
}

/// Searcher over a single loaded sealed generation.
///
/// Bridges the sync `SemanticSearcher` port surface to the async Lance API
/// via the adapter's shared tokio runtime — see [`crate::run_blocking`].
/// Every search runs under the request's budget and reports what its lane
/// observed to the adapter-wide tallies (W5 phase 3).
pub(crate) struct PersistedSemanticSearcher {
    loaded: Arc<LoadedGeneration>,
    runtime: Arc<tokio::runtime::Runtime>,
    tallies: Arc<DenseLaneTalliesV1>,
    #[cfg(feature = "qualified-memory-uri-candidate")]
    _candidate_view: Option<crate::qualified_fs::CandidateView>,
}

impl PersistedSemanticSearcher {
    pub(crate) fn new(
        loaded: Arc<LoadedGeneration>,
        runtime: Arc<tokio::runtime::Runtime>,
        tallies: Arc<DenseLaneTalliesV1>,
        #[cfg(feature = "qualified-memory-uri-candidate")] candidate_view: Option<
            crate::qualified_fs::CandidateView,
        >,
    ) -> Self {
        Self {
            loaded,
            runtime,
            tallies,
            #[cfg(feature = "qualified-memory-uri-candidate")]
            _candidate_view: candidate_view,
        }
    }

    /// The request's budget paired with this adapter's tallies, as every
    /// query path below takes it.
    fn watch<'a>(&'a self, budget: &'a RequestBudgetV1) -> DenseLaneBudgetV1<'a> {
        DenseLaneBudgetV1 {
            budget,
            tallies: &self.tallies,
        }
    }

    fn charge_exact_scan_work(&self, budget: &RequestBudgetV1) -> Result<(), CoreError> {
        // The seal bounds the physical scan, including rows excluded by a
        // pushed-down predicate. Charge before Lance starts work.
        let dimensions = u64::try_from(self.loaded.dimension).map_err(|error| {
            CoreError::InvalidContract(format!("semantic dimension overflow: {error}"))
        })?;
        let work = self
            .loaded
            .sealed_row_count
            .checked_mul(dimensions)
            .ok_or_else(|| CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SemanticWorkBudgetExceeded,
                message: "sealed semantic generation exceeds work representation".into(),
            })?;
        budget.consume_semantic_work_v1(work)
    }
}

fn top_k_limit(top_k: u32) -> Result<usize, CoreError> {
    usize::try_from(top_k)
        .map_err(|err| CoreError::InvalidContract(format!("semantic: top_k overflow: {err}")))
}

impl SemanticSearcher for PersistedSemanticSearcher {
    fn validate_query_vector(&self, query_vector: &[f32]) -> Result<(), CoreError> {
        self.loaded.validate_query_vector(query_vector)
    }

    fn resident_bytes_estimate(&self) -> u64 {
        self.loaded.resident_bytes_estimate
    }

    fn cluster_membership_batch_read(
        &self,
        request: &ClusterMembershipBatchReadRequestV1,
    ) -> Result<ClusterMembershipBatchReadResponseV1, CoreError> {
        crate::run_blocking(
            &self.runtime,
            self.loaded.cluster_membership_batch_read_async(request),
        )
    }

    fn search(
        &self,
        query_vector: &[f32],
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded
                .search_async(query_vector, limit, self.watch(budget)),
        )
    }

    fn search_constrained(
        &self,
        query_vector: &[f32],
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded.search_hits_constrained_async(
                query_vector,
                limit,
                None,
                None,
                constraints,
                self.watch(budget),
            ),
        )
        .map(LoadedGeneration::map_hits_to_candidates)
    }

    fn search_constrained_canonical_ties(
        &self,
        query_vector: &[f32],
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        self.loaded.validate_query_vector(query_vector)?;
        if budget.semantic_work_consumed_v1()?.is_some() {
            self.charge_exact_scan_work(budget)?;
        }
        let limit = top_k_limit(top_k)?;
        let filter = LoadedGeneration::combine_filters(
            LoadedGeneration::language_any_of_filter(constraints)
                .into_iter()
                .chain(LoadedGeneration::exact_repo_relative_path_filter(
                    constraints,
                )),
        );
        crate::run_blocking(
            &self.runtime,
            self.loaded
                .run_canonical_exact_scan(query_vector, limit, filter, self.watch(budget)),
        )
        .map(LoadedGeneration::map_hits_to_candidates)
    }

    fn search_work_bounded_v1(
        &self,
        query_vector: &[f32],
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        if budget.semantic_work_consumed_v1()?.is_none() {
            return Err(CoreError::InvalidContract(
                "bounded semantic search requires a work ledger".into(),
            ));
        }
        SemanticPolicy::validate_fetch_size(top_k)?;
        let limit = top_k_limit(top_k)?;
        // The sealed physical row count is checked when the generation opens.
        // Admit every possible vector component score before the exact lane.
        self.loaded.validate_query_vector(query_vector)?;
        self.charge_exact_scan_work(budget)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded.run_lane(
                query_vector,
                limit,
                LoadedGeneration::combine_filters(
                    LoadedGeneration::language_any_of_filter(constraints)
                        .into_iter()
                        .chain(LoadedGeneration::exact_repo_relative_path_filter(
                            constraints,
                        )),
                ),
                self.watch(budget),
                DenseLaneKindV1::Exact,
            ),
        )
        .map(LoadedGeneration::map_hits_to_candidates)
    }

    fn search_hits(
        &self,
        query_vector: &[f32],
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded
                .search_hits_filtered_async(query_vector, limit, None, self.watch(budget)),
        )
        .and_then(LoadedGeneration::map_hits_to_core_v1)
    }

    fn search_hits_constrained(
        &self,
        query_vector: &[f32],
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded.search_hits_constrained_async(
                query_vector,
                limit,
                None,
                None,
                constraints,
                self.watch(budget),
            ),
        )
        .and_then(LoadedGeneration::map_hits_to_core_v1)
    }

    fn search_hits_for_corpus(
        &self,
        query_vector: &[f32],
        corpus_kind: SemanticCorpusKindV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded.search_hits_filtered_async(
                query_vector,
                limit,
                Some(corpus_kind.as_code_str()),
                self.watch(budget),
            ),
        )
        .and_then(LoadedGeneration::map_hits_to_core_v1)
    }

    fn search_hits_for_corpus_constrained(
        &self,
        query_vector: &[f32],
        corpus_kind: SemanticCorpusKindV1,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded.search_hits_constrained_async(
                query_vector,
                limit,
                None,
                Some(corpus_kind.as_code_str()),
                constraints,
                self.watch(budget),
            ),
        )
        .and_then(LoadedGeneration::map_hits_to_core_v1)
    }

    fn search_scoped(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded
                .search_scoped_async(query_vector, allowed_ids, limit, self.watch(budget)),
        )
    }

    fn search_scoped_constrained(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded.search_hits_constrained_async(
                query_vector,
                limit,
                Some(allowed_ids),
                None,
                constraints,
                self.watch(budget),
            ),
        )
        .map(LoadedGeneration::map_hits_to_candidates)
    }

    fn score_candidate(
        &self,
        candidate_id: &str,
        query_vector: &[f32],
        budget: &RequestBudgetV1,
    ) -> Result<Option<f32>, CoreError> {
        crate::run_blocking(
            &self.runtime,
            self.loaded
                .score_candidate_async(candidate_id, query_vector, self.watch(budget)),
        )
    }

    fn index_model_id(&self) -> &str {
        self.loaded.model_id()
    }

    fn index_model_revision(&self) -> Option<&str> {
        self.loaded.model_version()
    }

    fn manifest_digest(&self) -> &str {
        self.loaded.manifest_digest()
    }

    fn dense_lane(&self) -> DenseLaneContractV1 {
        self.loaded.vector_index.contract()
    }
}

/// Exhaustive logical state export for the opt-in local proof tool.
#[cfg(feature = "proof")]
pub(crate) async fn proof_rows_v1(
    semantic_root: &Path,
    batch: &quanta_index_contract::SemanticIngestBatch,
) -> Result<serde_json::Value, CoreError> {
    let loaded = open_generation(
        semantic_root,
        &batch.repo_id,
        &batch.revision_id,
        batch.generation,
        None,
    )
    .await?;
    let mut tables = serde_json::Map::new();
    for (name, table) in [
        ("semantic", &loaded.table),
        ("membership", &loaded.cluster_membership),
    ] {
        let count = table
            .count_rows(None)
            .await
            .map_err(|error| lancedb_err("proof count", error))?;
        if count > 65_536 {
            return Err(CoreError::InvalidContract(
                "proof table exceeds row limit".to_string(),
            ));
        }
        let mut stream = table
            .scan()
            .batch_size(DATASET_READ_BATCH_ROWS)
            .strict_batch_size(true)
            .try_into_stream()
            .await
            .map_err(|error| lancedb_err("proof query", error))?;
        let mut rows = Vec::new();
        while let Some(batch) = stream
            .try_next()
            .await
            .map_err(|error| lancedb_err("proof stream", error))?
        {
            for row in 0..batch.num_rows() {
                let mut values = serde_json::Map::new();
                for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
                    let value = proof_cell_v1(column.as_ref(), row)?;
                    if value.is_null() && !field.is_nullable() {
                        return Err(CoreError::Storage(
                            "proof nonnullable cell is null".to_string(),
                        ));
                    }
                    let _previous = values.insert(field.name().clone(), value);
                }
                rows.push(serde_json::Value::Object(values));
                if rows.len() > count {
                    return Err(CoreError::Storage(
                        "proof stream exceeds counted rows".to_string(),
                    ));
                }
            }
        }
        if rows.len() != count {
            return Err(CoreError::Storage("proof stream is partial".to_string()));
        }
        rows.sort_unstable_by_key(serde_json::Value::to_string);
        let _previous = tables.insert(
            name.to_owned(),
            serde_json::json!({"count": count, "rows": rows}),
        );
    }
    Ok(serde_json::Value::Object(tables))
}

#[cfg(feature = "proof")]
fn proof_cell_v1(column: &dyn Array, row: usize) -> Result<serde_json::Value, CoreError> {
    if column.is_null(row) {
        return Ok(serde_json::Value::Null);
    }
    if let Some(values) = column.as_any().downcast_ref::<StringArray>() {
        return Ok(serde_json::json!(values.value(row)));
    }
    if let Some(values) = column.as_any().downcast_ref::<UInt32Array>() {
        return Ok(serde_json::json!(values.value(row)));
    }
    if let Some(values) = column.as_any().downcast_ref::<arrow_array::BooleanArray>() {
        return Ok(serde_json::json!(values.value(row)));
    }
    if let Some(values) = column.as_any().downcast_ref::<Float32Array>() {
        let value = values.value(row);
        if !value.is_finite() {
            return Err(CoreError::Storage("proof nonfinite vector".to_string()));
        }
        return Ok(serde_json::json!(value));
    }
    if let Some(values) = column
        .as_any()
        .downcast_ref::<arrow_array::FixedSizeListArray>()
    {
        let child = values.value(row);
        let values = (0..child.len())
            .map(|offset| proof_cell_v1(child.as_ref(), offset))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(serde_json::Value::Array(values));
    }
    Err(CoreError::Storage(format!(
        "unsupported proof column type {}",
        column.data_type()
    )))
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "fixtures in this module are built with known fixed lengths; an out-of-range index is a test authoring bug that should fail loudly"
)]
mod tests {
    use super::{
        ClusterMembershipReadRowV1, cosine_distance_to_score_v1, parse_owner_kind_v1,
        validate_cluster_membership_rows_v1,
    };
    use quanta_index_contract::{
        ClusterMembershipReadFailureV1, ClusterMembershipReadRequestV1, GenerationPin,
        ManifestGeneration, OwnerDocKind, RepoId, RevisionId, SymbolId,
        cluster_membership_content_digest_v1,
    };
    use quanta_index_core::CoreError;

    // CASE-COVERS: non-finite cosine distance must fail closed, not seed a NaN score.
    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "the conversion is exact arithmetic (1.0 - distance) on values chosen to be representable, so the test pins the exact result rather than a tolerance that would hide a drifted formula"
    )]
    fn cosine_distance_to_score_rejects_non_finite_v1() {
        // Finite distances convert to similarity = 1 - distance.
        assert_eq!(
            cosine_distance_to_score_v1(0.0, "c").expect("finite ok"),
            1.0
        );
        assert_eq!(
            cosine_distance_to_score_v1(2.0, "c").expect("finite ok"),
            -1.0
        );
        assert_eq!(
            cosine_distance_to_score_v1(0.5, "c").expect("finite ok"),
            0.5
        );

        // NaN / +Inf / -Inf each fail closed as a storage/index-corruption error
        // (not a query-vector error, not a silent NaN score).
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            match cosine_distance_to_score_v1(bad, "candidate-x") {
                Err(CoreError::Storage(message)) => assert!(
                    message.contains("non-finite cosine distance"),
                    "storage error must name the corruption, got {message}"
                ),
                other => {
                    panic!("non-finite distance {bad} must fail closed as Storage, got {other:?}")
                }
            }
        }
    }

    #[test]
    fn owner_kind_parser_preserves_valid_value_and_rejects_malformed_v1() {
        assert_eq!(
            parse_owner_kind_v1("Test", "record-test").expect("valid owner kind"),
            OwnerDocKind::Test
        );
        match parse_owner_kind_v1("test", "record-bad") {
            Err(CoreError::Storage(message)) => {
                assert!(message.contains("unsupported owner_kind \"test\""));
                assert!(message.contains("record-bad"));
            }
            other => panic!("malformed v4 owner kind must fail closed, got {other:?}"),
        }
    }

    fn membership_request_v1() -> ClusterMembershipReadRequestV1 {
        ClusterMembershipReadRequestV1 {
            cluster_record_id: "cluster:a".to_string(),
            generation: GenerationPin::new(
                RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(7),
            ),
            expected_authority_digest: "authority:a".to_string(),
            limit: 8,
        }
    }

    fn membership_rows_v1() -> Vec<ClusterMembershipReadRowV1> {
        let members = vec![SymbolId::new("symbol:a"), SymbolId::new("symbol:b")];
        let digest = cluster_membership_content_digest_v1(&members);
        members
            .into_iter()
            .enumerate()
            .map(|(ordinal, member)| ClusterMembershipReadRowV1 {
                ordinal: u32::try_from(ordinal).expect("two-row fixture ordinal fits u32"),
                cluster_record_id: "cluster:a".to_string(),
                authority_digest: "authority:a".to_string(),
                member_symbol_id: member.as_str().to_string(),
                member_count: 2,
                membership_digest: digest.clone(),
            })
            .collect()
    }

    #[test]
    fn cluster_membership_committed_count_and_digest_reject_row_loss_substitution_and_reorder_v1() {
        let request = membership_request_v1();
        assert_eq!(
            validate_cluster_membership_rows_v1(membership_rows_v1(), &request)
                .expect("valid committed rows"),
            [SymbolId::new("symbol:a"), SymbolId::new("symbol:b")]
        );

        let mut trailing_row_deleted = membership_rows_v1();
        let _deleted = trailing_row_deleted.pop();
        assert_eq!(
            validate_cluster_membership_rows_v1(trailing_row_deleted, &request),
            Err(ClusterMembershipReadFailureV1::CorruptSidecar)
        );

        let mut same_count_substitution = membership_rows_v1();
        same_count_substitution[1].member_symbol_id = "symbol:c".to_string();
        assert_eq!(
            validate_cluster_membership_rows_v1(same_count_substitution, &request),
            Err(ClusterMembershipReadFailureV1::CorruptSidecar)
        );

        let mut reordered = membership_rows_v1();
        reordered[0].ordinal = 1;
        reordered[1].ordinal = 0;
        assert_eq!(
            validate_cluster_membership_rows_v1(reordered, &request),
            Err(ClusterMembershipReadFailureV1::CorruptSidecar)
        );

        let oversized = (0..=quanta_index_contract::MAX_CLUSTER_MEMBERSHIP_READ_V1)
            .map(|ordinal| ClusterMembershipReadRowV1 {
                ordinal,
                cluster_record_id: "cluster:a".to_string(),
                authority_digest: "authority:a".to_string(),
                member_symbol_id: format!("symbol:{ordinal:04}"),
                member_count: quanta_index_contract::MAX_CLUSTER_MEMBERSHIP_READ_V1 + 1,
                membership_digest: "unreachable-oversized-digest".to_string(),
            })
            .collect();
        assert_eq!(
            validate_cluster_membership_rows_v1(oversized, &request),
            Err(ClusterMembershipReadFailureV1::MemberLimitExceeded)
        );
    }
}
