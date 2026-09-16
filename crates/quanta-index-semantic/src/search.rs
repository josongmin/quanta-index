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

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use arrow_array::{Array, Float32Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::DataType;
use futures::TryStreamExt as _;
use lancedb::DistanceType;
use lancedb::connect;
use lancedb::query::{ExecutableQuery as _, QueryBase as _};
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    ClusterMembershipBatchReadRequestV1, ClusterMembershipBatchReadResponseV1,
    ClusterMembershipCompletenessV1, ClusterMembershipReadFailureV1,
    ClusterMembershipReadOutcomeV1, ClusterMembershipReadRejectionV1,
    ClusterMembershipReadRequestV1, ClusterMembershipSnapshotV1, ClusterMembershipUnavailableV1,
    GenerationPin, LexicalCandidate, ManifestGeneration, OwnerDocKind, QueryConstraintSetV1,
    RepoId, RepoRelativePath, RevisionId, SemanticCorpusKindV1, SymbolId,
    canonical_order::{CanonicalOrderBreakV1, first_canonical_order_break_v1},
    cluster_membership_content_digest_v1,
};
use quanta_index_core::CoreError;
use quanta_index_core::domains::semantic::{SemanticPolicy, SemanticSearchHitV1, SemanticSearcher};

use crate::errors::lancedb_err;
use crate::generation_contract::GenerationContract;
use crate::layout::{
    self, CLUSTER_MEMBERSHIP_TABLE_NAME, COLUMN_AUTHORITY_DIGEST, COLUMN_CORPUS_KIND,
    COLUMN_EMBEDDING_ID, COLUMN_END_LINE, COLUMN_LANGUAGE, COLUMN_MEMBERSHIP_AUTHORITY_DIGEST,
    COLUMN_MEMBERSHIP_CLUSTER_RECORD_ID, COLUMN_MEMBERSHIP_CONTENT_DIGEST,
    COLUMN_MEMBERSHIP_MEMBER_COUNT, COLUMN_MEMBERSHIP_MEMBER_SYMBOL_ID, COLUMN_MEMBERSHIP_ORDINAL,
    COLUMN_MEMBERSHIP_OWNER_ID, COLUMN_MEMBERSHIP_OWNER_KIND, COLUMN_OWNER_ID, COLUMN_OWNER_KIND,
    COLUMN_RECORD_ID, COLUMN_REPO_RELATIVE_PATH, COLUMN_SNIPPET, COLUMN_START_LINE, TABLE_NAME,
    dataset_uri,
};
use crate::manifest::{
    FORMAT_VERSION, LEGACY_BUILD_CONTRACT_FORMAT_VERSION, LEGACY_LANCEDB_FORMAT_VERSION,
    LEGACY_SEMANTIC_CORPUS_FORMAT_VERSION, LEGACY_UNCOMMITTED_MEMBERSHIP_FORMAT_VERSION,
    LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION, SemanticManifest,
};
use crate::membership_integrity::{
    ClusterMembershipStoredRowV1 as ClusterMembershipIntegrityRowV1,
    cluster_membership_commitment_v1,
};
use crate::semantic_row_integrity_v1::semantic_row_commitment_v1;
use crate::sql::build_id_in_filter;

const COLUMN_DISTANCE: &str = "_distance";

fn has_semantic_corpus_metadata_v1(format_version: u32) -> bool {
    matches!(
        format_version,
        FORMAT_VERSION
            | LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION
            | LEGACY_UNCOMMITTED_MEMBERSHIP_FORMAT_VERSION
            | LEGACY_SEMANTIC_CORPUS_FORMAT_VERSION
    )
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
        FORMAT_VERSION
        | LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION
        | LEGACY_UNCOMMITTED_MEMBERSHIP_FORMAT_VERSION
        | LEGACY_SEMANTIC_CORPUS_FORMAT_VERSION
        | LEGACY_BUILD_CONTRACT_FORMAT_VERSION => Err(CoreError::Storage(format!(
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
    format_version: u32,
    dimension: usize,
    model_id: String,
    model_version: Option<String>,
    table: lancedb::Table,
    cluster_membership: LoadedClusterMembershipV1,
}

enum LoadedClusterMembershipV1 {
    Available(lancedb::Table),
    LegacyUnavailable,
}

fn unavailable_or_rejected_membership_v1(
    state: &LoadedClusterMembershipV1,
    request: &ClusterMembershipReadRequestV1,
) -> Option<ClusterMembershipReadOutcomeV1> {
    match state {
        LoadedClusterMembershipV1::Available(_) => None,
        LoadedClusterMembershipV1::LegacyUnavailable => Some(
            ClusterMembershipReadOutcomeV1::Unavailable(ClusterMembershipUnavailableV1 {
                cluster_record_id: request.cluster_record_id.clone(),
                generation: request.generation.clone(),
                expected_authority_digest: request.expected_authority_digest.clone(),
            }),
        ),
    }
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
    pub(crate) corpus_kind: Option<String>,
    pub(crate) authority_digest: String,
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
    let manifest_dimension_i32 = i32::try_from(manifest.dimension).map_err(|err| {
        CoreError::Storage(format!(
            "semantic: manifest dimension {} does not fit i32 for schema validation: {err}",
            manifest.dimension
        ))
    })?;
    let expected_schema = layout::semantic_schema_for_manifest_version(
        manifest.format_version,
        manifest_dimension_i32,
    )?;

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
    let live_schema = table
        .schema()
        .await
        .map_err(|err| lancedb_err("read table schema", err))?;
    for expected_field in expected_schema.fields() {
        let live_field = live_schema
            .field_with_name(expected_field.name())
            .map_err(|err| {
                CoreError::Storage(format!(
                    "semantic: table missing expected column `{}` for format version {}: {err}",
                    expected_field.name(),
                    manifest.format_version,
                ))
            })?;
        if live_field.data_type() != expected_field.data_type() {
            return Err(CoreError::Storage(format!(
                "semantic: table column `{}` type {:?} does not match expected {:?} for format version {}",
                expected_field.name(),
                live_field.data_type(),
                expected_field.data_type(),
                manifest.format_version,
            )));
        }
        if *expected_field.data_type() == DataType::Boolean && live_field.is_nullable() {
            return Err(CoreError::Storage(format!(
                "semantic: table boolean column `{}` unexpectedly nullable for format version {}",
                expected_field.name(),
                manifest.format_version,
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
    if manifest.format_version == FORMAT_VERSION {
        let commitment = semantic_row_commitment_v1(&table).await?;
        // The commitment's `root_digest` is compared against the manifest's
        // `semantic_row_root_digest`: the manifest carries a second, unrelated
        // root for the membership sidecar, so the field names differ on purpose.
        #[expect(
            clippy::suspicious_operation_groupings,
            reason = "the manifest has no `root_digest` field; `semantic_row_root_digest` is the main table's root and the comparison is deliberate"
        )]
        let row_root_mismatch = commitment.row_count != manifest.row_count
            || commitment.root_digest != manifest.semantic_row_root_digest;
        if row_root_mismatch {
            return Err(CoreError::Typed {
                code: "SEMANTIC_ROW_ROOT_MISMATCH".to_string(),
                message: format!(
                    "semantic: main table does not match sealed row root for repo={} revision={} generation={}",
                    repo.as_str(),
                    revision.as_str(),
                    generation.get()
                ),
            });
        }
    }

    let cluster_membership = if matches!(
        manifest.format_version,
        FORMAT_VERSION | LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION
    ) {
        let names = connection
            .table_names()
            .execute()
            .await
            .map_err(|err| lancedb_err("table_names", err))?;
        if !names
            .iter()
            .any(|name| name == CLUSTER_MEMBERSHIP_TABLE_NAME)
        {
            return Err(CoreError::Storage(
                "semantic: current manifest requires the cluster membership table".to_string(),
            ));
        }
        let membership_table = connection
            .open_table(CLUSTER_MEMBERSHIP_TABLE_NAME)
            .execute()
            .await
            .map_err(|error| lancedb_err("open cluster membership table", error))?;
        let membership_schema = membership_table
            .schema()
            .await
            .map_err(|error| lancedb_err("read cluster membership schema", error))?;
        if !layout::cluster_membership_schema()
            .fields()
            .iter()
            .all(|expected| {
                membership_schema
                    .field_with_name(expected.name())
                    .is_ok_and(|live| {
                        live.data_type() == expected.data_type() && !live.is_nullable()
                    })
            })
        {
            return Err(CoreError::Storage(
                "semantic: current cluster membership table has an incompatible schema".to_string(),
            ));
        }
        verify_cluster_membership_commitment_v1(&membership_table, &manifest).await?;
        LoadedClusterMembershipV1::Available(membership_table)
    } else {
        LoadedClusterMembershipV1::LegacyUnavailable
    };

    drop(connection);
    Ok(LoadedGeneration {
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        generation,
        format_version: manifest.format_version,
        dimension,
        model_id: manifest.model_id.clone(),
        model_version: manifest.model_version.clone(),
        table,
        cluster_membership,
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

async fn verify_cluster_membership_commitment_v1(
    table: &lancedb::Table,
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

    let committed_capacity = usize::try_from(manifest.cluster_membership_member_row_count)
        .map_err(|error| {
            CoreError::Storage(format!(
                "semantic: sealed cluster membership row count does not fit memory bound: {error}"
            ))
        })?;
    let mut rows = Vec::with_capacity(committed_capacity);
    let mut stream = table
        .query()
        .execute()
        .await
        .map_err(|error| lancedb_err("query cluster membership commitment at open", error))?;
    while let Some(batch) = stream
        .try_next()
        .await
        .map_err(|error| lancedb_err("stream cluster membership commitment at open", error))?
    {
        if rows
            .len()
            .checked_add(batch.num_rows())
            .is_none_or(|next| next > committed_capacity)
        {
            return Err(CoreError::Storage(
                "semantic: cluster membership stream exceeded sealed row bound".to_string(),
            ));
        }
        if batch
            .columns()
            .iter()
            .any(|column| column.null_count() != 0)
        {
            return Err(CoreError::Storage(
                "semantic: cluster membership stream contains null values".to_string(),
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
            rows.push(ClusterMembershipIntegrityRowV1 {
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
    let observed = cluster_membership_commitment_v1(rows)
        .map_err(|error| CoreError::Storage(format!("semantic: {error}")))?;
    if observed.root_digest != manifest.cluster_membership_root_digest
        || observed.cluster_count != manifest.cluster_membership_cluster_count
        || observed.member_row_count != manifest.cluster_membership_member_row_count
    {
        return Err(CoreError::Storage(
            "semantic: cluster membership sidecar does not match sealed manifest commitment"
                .to_string(),
        ));
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

fn extract_hits(
    batch: &RecordBatch,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    generation: ManifestGeneration,
    format_version: u32,
    out: &mut Vec<SemanticSearchHit>,
) -> Result<(), CoreError> {
    let id_col = column_as::<StringArray>(batch, COLUMN_EMBEDDING_ID, "Utf8")?;
    let path_col = column_as::<StringArray>(batch, COLUMN_REPO_RELATIVE_PATH, "Utf8")?;
    let start_col = column_as::<UInt32Array>(batch, COLUMN_START_LINE, "UInt32")?;
    let end_col = column_as::<UInt32Array>(batch, COLUMN_END_LINE, "UInt32")?;
    let snippet_col = column_as::<StringArray>(batch, COLUMN_SNIPPET, "Utf8")?;
    let distance_col = column_as::<Float32Array>(batch, COLUMN_DISTANCE, "Float32")?;
    let record_id_col = if has_semantic_corpus_metadata_v1(format_version) {
        Some(column_as::<StringArray>(batch, COLUMN_RECORD_ID, "Utf8")?)
    } else {
        None
    };
    let owner_id_col = if has_semantic_corpus_metadata_v1(format_version) {
        Some(column_as::<StringArray>(batch, COLUMN_OWNER_ID, "Utf8")?)
    } else {
        None
    };
    let owner_kind_col = if has_semantic_corpus_metadata_v1(format_version) {
        Some(column_as::<StringArray>(batch, COLUMN_OWNER_KIND, "Utf8")?)
    } else {
        None
    };
    let corpus_kind_col = if has_semantic_corpus_metadata_v1(format_version) {
        Some(column_as::<StringArray>(batch, COLUMN_CORPUS_KIND, "Utf8")?)
    } else {
        None
    };
    let authority_digest_col = column_as::<StringArray>(batch, COLUMN_AUTHORITY_DIGEST, "Utf8")?;
    for row in 0..batch.num_rows() {
        let id = id_col.value(row).to_owned();
        let path = path_col.value(row).to_owned();
        let snippet = snippet_col.value(row).to_owned();
        let start_line = start_col.value(row);
        let end_line = end_col.value(row);
        let distance = distance_col.value(row);
        let score = cosine_distance_to_score_v1(distance, &id)?;
        let candidate = LexicalCandidate {
            candidate_id: id.clone(),
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
            highlights: Vec::new(),
        };
        out.push(SemanticSearchHit {
            candidate,
            record_id: record_id_col.map_or_else(|| id.clone(), |col| col.value(row).to_owned()),
            owner_id: owner_id_col.map_or_else(|| id.clone(), |col| col.value(row).to_owned()),
            owner_kind: owner_kind_col.map_or_else(
                || OwnerDocKind::Chunk.as_code_str().to_owned(),
                |col| col.value(row).to_owned(),
            ),
            corpus_kind: corpus_kind_col.map(|col| col.value(row).to_owned()),
            authority_digest: authority_digest_col.value(row).to_owned(),
        });
    }
    Ok(())
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
        if !matches!(
            self.cluster_membership,
            LoadedClusterMembershipV1::Available(_)
        ) {
            let outcomes = request
                .items
                .iter()
                .map(|item| item.as_single_request_v1(&request.generation))
                .map(|single| {
                    unavailable_or_rejected_membership_v1(&self.cluster_membership, &single)
                        .unwrap_or_else(|| {
                            self.cluster_membership_rejection_v1(
                                &single,
                                ClusterMembershipReadFailureV1::CorruptSidecar,
                            )
                        })
                })
                .collect();
            return Ok(ClusterMembershipBatchReadResponseV1 { outcomes });
        }
        let LoadedClusterMembershipV1::Available(table) = &self.cluster_membership else {
            return Err(CoreError::Storage(
                "semantic: cluster membership state changed after terminal classification"
                    .to_string(),
            ));
        };
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
        let mut stream = match table
            .query()
            .only_if(predicate)
            .limit(maximum_rows)
            .execute()
            .await
        {
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
    ) -> Result<Vec<SemanticSearchHit>, CoreError> {
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

        let mut out: Vec<SemanticSearchHit> = Vec::with_capacity(top_k);
        for batch in batches {
            extract_hits(
                &batch,
                &self.repo_id,
                &self.revision_id,
                self.generation,
                self.format_version,
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
                let corpus_kind = hit
                    .corpus_kind
                    .as_deref()
                    .map(|value| {
                        SemanticCorpusKindV1::from_code_str(value).ok_or_else(|| {
                            CoreError::Storage(format!(
                                "semantic: unsupported corpus_kind {:?} on record {}",
                                value, hit.record_id
                            ))
                        })
                    })
                    .transpose()?;
                Ok(SemanticSearchHitV1 {
                    candidate: hit.candidate,
                    record_id: hit.record_id,
                    owner_id: hit.owner_id,
                    owner_kind,
                    corpus_kind,
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
    ) -> Result<Vec<SemanticSearchHit>, CoreError> {
        self.check_query_dim(query_vector)?;
        if allowed_ids.is_some_and(BTreeSet::is_empty) {
            return Ok(Vec::new());
        }
        if (corpus_kind.is_some() || !constraints.language_any_of.is_empty())
            && !has_semantic_corpus_metadata_v1(self.format_version)
        {
            return Err(CoreError::Storage(format!(
                "semantic: corpus/language filters require format version {LEGACY_SEMANTIC_CORPUS_FORMAT_VERSION} or newer, found {}",
                self.format_version
            )));
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
        self.run_vector_query(query_vector, top_k, filter).await
    }

    pub(crate) async fn search_async(
        &self,
        query_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        self.search_hits_constrained_async(
            query_vector,
            top_k,
            None,
            None,
            &QueryConstraintSetV1::unconstrained(),
        )
        .await
        .map(Self::map_hits_to_candidates)
    }

    pub(crate) async fn search_scoped_async(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        top_k: usize,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        self.search_hits_constrained_async(
            query_vector,
            top_k,
            Some(allowed_ids),
            None,
            &QueryConstraintSetV1::unconstrained(),
        )
        .await
        .map(Self::map_hits_to_candidates)
    }

    pub(crate) async fn search_hits_filtered_async(
        &self,
        query_vector: &[f32],
        top_k: usize,
        corpus_kind: Option<&str>,
    ) -> Result<Vec<SemanticSearchHit>, CoreError> {
        self.search_hits_constrained_async(
            query_vector,
            top_k,
            None,
            corpus_kind,
            &QueryConstraintSetV1::unconstrained(),
        )
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
    fn cluster_membership_batch_read(
        &self,
        request: &ClusterMembershipBatchReadRequestV1,
    ) -> Result<ClusterMembershipBatchReadResponseV1, CoreError> {
        crate::run_blocking(
            &self.runtime,
            self.loaded.cluster_membership_batch_read_async(request),
        )
    }

    fn search(&self, query_vector: &[f32], top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        SemanticPolicy::validate_query_vector(query_vector)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(&self.runtime, self.loaded.search_async(query_vector, limit))
    }

    fn search_constrained(
        &self,
        query_vector: &[f32],
        constraints: &QueryConstraintSetV1,
        top_k: u32,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        SemanticPolicy::validate_query_vector(query_vector)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded
                .search_hits_constrained_async(query_vector, limit, None, None, constraints),
        )
        .map(LoadedGeneration::map_hits_to_candidates)
    }

    fn search_hits(
        &self,
        query_vector: &[f32],
        top_k: u32,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        SemanticPolicy::validate_query_vector(query_vector)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded
                .search_hits_filtered_async(query_vector, limit, None),
        )
        .and_then(LoadedGeneration::map_hits_to_core_v1)
    }

    fn search_hits_constrained(
        &self,
        query_vector: &[f32],
        constraints: &QueryConstraintSetV1,
        top_k: u32,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        SemanticPolicy::validate_query_vector(query_vector)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded
                .search_hits_constrained_async(query_vector, limit, None, None, constraints),
        )
        .and_then(LoadedGeneration::map_hits_to_core_v1)
    }

    fn search_hits_for_corpus(
        &self,
        query_vector: &[f32],
        corpus_kind: SemanticCorpusKindV1,
        top_k: u32,
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        SemanticPolicy::validate_query_vector(query_vector)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded.search_hits_filtered_async(
                query_vector,
                limit,
                Some(corpus_kind.as_code_str()),
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
    ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        SemanticPolicy::validate_query_vector(query_vector)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded.search_hits_constrained_async(
                query_vector,
                limit,
                None,
                Some(corpus_kind.as_code_str()),
                constraints,
            ),
        )
        .and_then(LoadedGeneration::map_hits_to_core_v1)
    }

    fn search_scoped(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        top_k: u32,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        SemanticPolicy::validate_query_vector(query_vector)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded
                .search_scoped_async(query_vector, allowed_ids, limit),
        )
    }

    fn search_scoped_constrained(
        &self,
        query_vector: &[f32],
        allowed_ids: &BTreeSet<String>,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        SemanticPolicy::validate_fetch_size(top_k)?;
        SemanticPolicy::validate_query_vector(query_vector)?;
        let limit = top_k_limit(top_k)?;
        crate::run_blocking(
            &self.runtime,
            self.loaded.search_hits_constrained_async(
                query_vector,
                limit,
                Some(allowed_ids),
                None,
                constraints,
            ),
        )
        .map(LoadedGeneration::map_hits_to_candidates)
    }

    fn index_model_id(&self) -> &str {
        self.loaded.model_id()
    }

    fn index_model_version(&self) -> Option<&str> {
        self.loaded.model_version()
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "fixtures in this module are built with known fixed lengths; an out-of-range index is a test authoring bug that should fail loudly"
)]
mod tests {
    use super::{
        ClusterMembershipReadRowV1, LoadedClusterMembershipV1, cosine_distance_to_score_v1,
        parse_owner_kind_v1, unavailable_or_rejected_membership_v1,
        validate_cluster_membership_rows_v1,
    };
    use quanta_index_contract::{
        ClusterMembershipReadFailureV1, ClusterMembershipReadOutcomeV1,
        ClusterMembershipReadRequestV1, GenerationPin, ManifestGeneration, OwnerDocKind, RepoId,
        RevisionId, SymbolId, cluster_membership_content_digest_v1,
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
                RepoId::new("repo"),
                RevisionId::new("rev"),
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
    fn cluster_membership_legacy_generation_is_typed_unavailable_v1() {
        let request = membership_request_v1();
        assert!(matches!(
            unavailable_or_rejected_membership_v1(
                &LoadedClusterMembershipV1::LegacyUnavailable,
                &request
            ),
            Some(ClusterMembershipReadOutcomeV1::Unavailable(_))
        ));
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
