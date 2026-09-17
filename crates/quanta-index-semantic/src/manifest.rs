//! Persisted semantic generation manifest (LDB-01 manifest contract).
//!
//! Internal to `quanta-index`, but correctness-critical: the open path fails
//! closed on a format, scope, or integrity mismatch rather than guessing around
//! missing or inconsistent state.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::collections::BTreeSet;

use quanta_index_contract::{
    EmbeddingDistanceMetric, EmbeddingNormalization, GenerationPin, ManifestGeneration, RepoId,
    RevisionId,
};
use quanta_index_core::CoreError;

use crate::codec::{self, cbor_serde};
use crate::generation_contract::GenerationContract;

/// Distance metric the lancedb adapter actually serves at query time.
///
/// The adapter pins `DistanceType::Cosine` everywhere; a manifest that records
/// a different metric is rejected at open so a future producer that switches
/// metric mid-flight cannot serve cosine-quantized data through a `dot` /
/// `euclidean` contract.
const SUPPORTED_DISTANCE_METRIC: &str = "cosine";

fn is_canonical_sha256_v1(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Current manifest format version. Bumped on any durable shape change.
///
/// `8` = v7 plus the dense lane's index contract (QI-BB-027).
pub(crate) const FORMAT_VERSION: u32 = 8;

/// Legacy manifest with the semantic-row root but no vector index seal.
pub(crate) const LEGACY_UNSEALED_VECTOR_INDEX_FORMAT_VERSION: u32 = 7;
pub(crate) const LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION: u32 = 6;

/// Legacy structured-membership manifest without a sealed sidecar commitment.
///
/// Not a capability threshold: `5` carries the same columns as `4` and, like
/// it, no membership commitment. It is named for the decode fixtures.
#[cfg(test)]
pub(crate) const LEGACY_UNCOMMITTED_MEMBERSHIP_FORMAT_VERSION: u32 = 5;

/// Legacy semantic-corpus manifest without structured membership capability.
pub(crate) const LEGACY_SEMANTIC_CORPUS_FORMAT_VERSION: u32 = 4;

/// Legacy manifest format with build-contract sidecar but without v4 corpus
/// coverage fields.
pub(crate) const LEGACY_BUILD_CONTRACT_FORMAT_VERSION: u32 = 3;

/// Legacy lancedb manifest format that predates `semantic-build-contract.cbor`.
///
/// Sealed generations written at `2` remain openable for compatibility, but
/// they do not get the stronger sidecar cross-check that `FORMAT_VERSION=3`
/// provides.
pub(crate) const LEGACY_LANCEDB_FORMAT_VERSION: u32 = 2;

/// What a manifest format version carries and proves.
///
/// Every door that used to match on the format-version constants asks this
/// instead, so adding a format is one threshold here rather than one arm in
/// each of them. Each capability, once gained by a format, is kept by every
/// later one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FormatCapabilitiesV1 {
    format_version: u32,
}

impl FormatCapabilitiesV1 {
    /// `semantic-build-contract.cbor` is required beside the manifest.
    pub(crate) const fn build_contract(self) -> bool {
        self.format_version >= LEGACY_BUILD_CONTRACT_FORMAT_VERSION
    }

    /// The table carries the corpus, owner and language metadata columns.
    pub(crate) const fn corpus_metadata(self) -> bool {
        self.format_version >= LEGACY_SEMANTIC_CORPUS_FORMAT_VERSION
    }

    /// The manifest commits to the cluster-membership sidecar.
    pub(crate) const fn membership_commitment(self) -> bool {
        self.format_version >= LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION
    }

    /// The manifest commits to the semantic row root.
    pub(crate) const fn semantic_row_root(self) -> bool {
        self.format_version >= LEGACY_UNSEALED_VECTOR_INDEX_FORMAT_VERSION
    }

    /// The seal commits to every file, so an open re-measures files, not
    /// rows, and a missing commitment is a refusal rather than a scan.
    pub(crate) const fn file_commitment(self) -> bool {
        self.format_version >= LEGACY_UNSEALED_VECTOR_INDEX_FORMAT_VERSION
    }

    /// The manifest records the dense lane's index contract.
    pub(crate) const fn vector_index_seal(self) -> bool {
        self.format_version >= FORMAT_VERSION
    }
}

/// The capabilities of `format_version`, or `None` when this adapter does
/// not serve that format.
#[must_use]
pub(crate) fn format_capabilities_v1(format_version: u32) -> Option<FormatCapabilitiesV1> {
    (LEGACY_LANCEDB_FORMAT_VERSION..=FORMAT_VERSION)
        .contains(&format_version)
        .then_some(FormatCapabilitiesV1 { format_version })
}

fn unsupported_format(format_version: u32) -> CoreError {
    CoreError::Storage(format!(
        "semantic: manifest format version {format_version} unsupported (this adapter serves {LEGACY_LANCEDB_FORMAT_VERSION} through {FORMAT_VERSION})"
    ))
}

/// The dense lane's index contract, sealed with the generation (QI-BB-027).
///
/// `mode` is `exact` or `ivf_hnsw_sq`; `ann` is present exactly when the
/// mode is an approximate index. The library that built the index is
/// recorded as provenance: an open verifies what the dataset reports against
/// `ann`, not the version string, so a library upgrade is an explicit A/B
/// over these fields rather than a refusal to serve every sealed generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VectorIndexSealV1 {
    pub(crate) mode: String,
    pub(crate) library: String,
    pub(crate) library_version: String,
    /// The seal policy's row floor: an index is built at or above it.
    pub(crate) index_min_rows: u64,
    pub(crate) ann: Option<AnnIndexSealV1>,
}

cbor_serde!(VectorIndexSealV1 {
    mode: String,
    library: String,
    library_version: String,
    index_min_rows: u64,
    ann: Option<AnnIndexSealV1>,
});

/// The approximate index a seal built: its full recipe, what the library
/// reported about it, and the query effort the lane spends in it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AnnIndexSealV1 {
    pub(crate) index_name: String,
    pub(crate) distance: String,
    pub(crate) num_partitions: u32,
    pub(crate) sample_rate: u32,
    pub(crate) max_iterations: u32,
    pub(crate) hnsw_m: u32,
    pub(crate) hnsw_ef_construction: u32,
    pub(crate) indexed_rows: u64,
    pub(crate) index_segments: u32,
    pub(crate) nprobes: u32,
    pub(crate) ef_floor: u32,
    pub(crate) ef_per_candidate: u32,
    pub(crate) refine_factor: u32,
}

cbor_serde!(AnnIndexSealV1 {
    index_name: String,
    distance: String,
    num_partitions: u32,
    sample_rate: u32,
    max_iterations: u32,
    hnsw_m: u32,
    hnsw_ef_construction: u32,
    indexed_rows: u64,
    index_segments: u32,
    nprobes: u32,
    ef_floor: u32,
    ef_per_candidate: u32,
    refine_factor: u32,
});

pub(crate) const VECTOR_INDEX_MODE_EXACT: &str = "exact";
pub(crate) const VECTOR_INDEX_MODE_IVF_HNSW_SQ: &str = "ivf_hnsw_sq";

impl VectorIndexSealV1 {
    /// Refuse a seal whose fields contradict each other or the row count.
    pub(crate) fn validate(&self, row_count: u64) -> Result<(), CoreError> {
        let invalid = |detail: &str| {
            CoreError::Storage(format!(
                "semantic: manifest vector index seal is inconsistent: {detail}"
            ))
        };
        match (self.mode.as_str(), self.ann.as_ref()) {
            (VECTOR_INDEX_MODE_EXACT, None) => {
                if row_count >= self.index_min_rows {
                    return Err(invalid(&format!(
                        "mode exact with {row_count} rows at or above the index floor {}",
                        self.index_min_rows
                    )));
                }
            }
            (VECTOR_INDEX_MODE_EXACT, Some(_)) => {
                return Err(invalid("mode exact carries an index record"));
            }
            (VECTOR_INDEX_MODE_IVF_HNSW_SQ, None) => {
                return Err(invalid("mode ivf_hnsw_sq carries no index record"));
            }
            (VECTOR_INDEX_MODE_IVF_HNSW_SQ, Some(ann)) => {
                if row_count < self.index_min_rows {
                    return Err(invalid(&format!(
                        "mode ivf_hnsw_sq with {row_count} rows below the index floor {}",
                        self.index_min_rows
                    )));
                }
                if ann.index_name.is_empty() {
                    return Err(invalid("index name is empty"));
                }
                if ann.distance != SUPPORTED_DISTANCE_METRIC {
                    return Err(invalid(&format!(
                        "index distance `{}` is not `{SUPPORTED_DISTANCE_METRIC}`",
                        ann.distance
                    )));
                }
                if ann.indexed_rows != row_count {
                    return Err(invalid(&format!(
                        "index covers {} rows but the manifest has {row_count}",
                        ann.indexed_rows
                    )));
                }
                if ann.num_partitions == 0
                    || ann.sample_rate == 0
                    || ann.max_iterations == 0
                    || ann.hnsw_m == 0
                    || ann.hnsw_ef_construction == 0
                    || ann.index_segments == 0
                {
                    return Err(invalid("a build parameter or the segment count is zero"));
                }
                if ann.nprobes == 0 || ann.nprobes > ann.num_partitions {
                    return Err(invalid(&format!(
                        "nprobes {} is outside 1..={}",
                        ann.nprobes, ann.num_partitions
                    )));
                }
                if ann.ef_floor == 0 || ann.ef_per_candidate == 0 || ann.refine_factor == 0 {
                    return Err(invalid("a query effort parameter is zero"));
                }
            }
            (other, _) => {
                return Err(invalid(&format!("unknown mode `{other}`")));
            }
        }
        if self.library.is_empty() || self.library_version.is_empty() {
            return Err(invalid("library provenance is empty"));
        }
        Ok(())
    }
}

pub(crate) struct SemanticManifest {
    pub(crate) format_version: u32,
    pub(crate) repo_id: String,
    pub(crate) revision_id: String,
    pub(crate) generation: u64,
    pub(crate) manifest_digest: String,
    pub(crate) model_id: String,
    pub(crate) model_version: Option<String>,
    pub(crate) dimension: u32,
    pub(crate) distance_metric: String,
    pub(crate) normalization: String,
    pub(crate) row_count: u64,
    pub(crate) semantic_row_root_digest: String,
    pub(crate) built_at_unix_nanos: u64,
    pub(crate) present_corpora: Vec<String>,
    pub(crate) required_corpora: Vec<String>,
    pub(crate) card_schema_versions: Vec<u32>,
    pub(crate) render_policy_digests: Vec<String>,
    pub(crate) corpus_policy_digest: Option<String>,
    pub(crate) cluster_membership_root_digest: String,
    pub(crate) cluster_membership_cluster_count: u64,
    pub(crate) cluster_membership_member_row_count: u64,
    /// `None` only for formats before `8`, which recorded no index contract.
    pub(crate) vector_index: Option<VectorIndexSealV1>,
}

cbor_serde!(SemanticManifest {
    format_version: u32,
    repo_id: String,
    revision_id: String,
    generation: u64,
    manifest_digest: String,
    model_id: String,
    model_version: Option<String>,
    dimension: u32,
    distance_metric: String,
    normalization: String,
    row_count: u64,
    semantic_row_root_digest: String,
    built_at_unix_nanos: u64,
    present_corpora: Vec<String>,
    required_corpora: Vec<String>,
    card_schema_versions: Vec<u32>,
    render_policy_digests: Vec<String>,
    corpus_policy_digest: Option<String>,
    cluster_membership_root_digest: String,
    cluster_membership_cluster_count: u64,
    cluster_membership_member_row_count: u64,
    vector_index: Option<VectorIndexSealV1>,
});

struct SemanticManifestV7 {
    format_version: u32,
    repo_id: String,
    revision_id: String,
    generation: u64,
    manifest_digest: String,
    model_id: String,
    model_version: Option<String>,
    dimension: u32,
    distance_metric: String,
    normalization: String,
    row_count: u64,
    semantic_row_root_digest: String,
    built_at_unix_nanos: u64,
    present_corpora: Vec<String>,
    required_corpora: Vec<String>,
    card_schema_versions: Vec<u32>,
    render_policy_digests: Vec<String>,
    corpus_policy_digest: Option<String>,
    cluster_membership_root_digest: String,
    cluster_membership_cluster_count: u64,
    cluster_membership_member_row_count: u64,
}

cbor_serde!(SemanticManifestV7 {
    format_version: u32,
    repo_id: String,
    revision_id: String,
    generation: u64,
    manifest_digest: String,
    model_id: String,
    model_version: Option<String>,
    dimension: u32,
    distance_metric: String,
    normalization: String,
    row_count: u64,
    semantic_row_root_digest: String,
    built_at_unix_nanos: u64,
    present_corpora: Vec<String>,
    required_corpora: Vec<String>,
    card_schema_versions: Vec<u32>,
    render_policy_digests: Vec<String>,
    corpus_policy_digest: Option<String>,
    cluster_membership_root_digest: String,
    cluster_membership_cluster_count: u64,
    cluster_membership_member_row_count: u64,
});

struct SemanticManifestV6 {
    format_version: u32,
    repo_id: String,
    revision_id: String,
    generation: u64,
    manifest_digest: String,
    model_id: String,
    model_version: Option<String>,
    dimension: u32,
    distance_metric: String,
    normalization: String,
    row_count: u64,
    built_at_unix_nanos: u64,
    present_corpora: Vec<String>,
    required_corpora: Vec<String>,
    card_schema_versions: Vec<u32>,
    render_policy_digests: Vec<String>,
    corpus_policy_digest: Option<String>,
    cluster_membership_root_digest: String,
    cluster_membership_cluster_count: u64,
    cluster_membership_member_row_count: u64,
}

cbor_serde!(SemanticManifestV6 {
    format_version: u32,
    repo_id: String,
    revision_id: String,
    generation: u64,
    manifest_digest: String,
    model_id: String,
    model_version: Option<String>,
    dimension: u32,
    distance_metric: String,
    normalization: String,
    row_count: u64,
    built_at_unix_nanos: u64,
    present_corpora: Vec<String>,
    required_corpora: Vec<String>,
    card_schema_versions: Vec<u32>,
    render_policy_digests: Vec<String>,
    corpus_policy_digest: Option<String>,
    cluster_membership_root_digest: String,
    cluster_membership_cluster_count: u64,
    cluster_membership_member_row_count: u64,
});

struct SemanticManifestV5 {
    format_version: u32,
    repo_id: String,
    revision_id: String,
    generation: u64,
    manifest_digest: String,
    model_id: String,
    model_version: Option<String>,
    dimension: u32,
    distance_metric: String,
    normalization: String,
    row_count: u64,
    built_at_unix_nanos: u64,
    present_corpora: Vec<String>,
    required_corpora: Vec<String>,
    card_schema_versions: Vec<u32>,
    render_policy_digests: Vec<String>,
    corpus_policy_digest: Option<String>,
}

cbor_serde!(SemanticManifestV5 {
    format_version: u32,
    repo_id: String,
    revision_id: String,
    generation: u64,
    manifest_digest: String,
    model_id: String,
    model_version: Option<String>,
    dimension: u32,
    distance_metric: String,
    normalization: String,
    row_count: u64,
    built_at_unix_nanos: u64,
    present_corpora: Vec<String>,
    required_corpora: Vec<String>,
    card_schema_versions: Vec<u32>,
    render_policy_digests: Vec<String>,
    corpus_policy_digest: Option<String>,
});

struct SemanticManifestV3 {
    format_version: u32,
    repo_id: String,
    revision_id: String,
    generation: u64,
    manifest_digest: String,
    model_id: String,
    model_version: Option<String>,
    dimension: u32,
    distance_metric: String,
    normalization: String,
    row_count: u64,
    built_at_unix_nanos: u64,
}

cbor_serde!(SemanticManifestV3 {
    format_version: u32,
    repo_id: String,
    revision_id: String,
    generation: u64,
    manifest_digest: String,
    model_id: String,
    model_version: Option<String>,
    dimension: u32,
    distance_metric: String,
    normalization: String,
    row_count: u64,
    built_at_unix_nanos: u64,
});

#[must_use]
pub(crate) fn distance_metric_token(metric: EmbeddingDistanceMetric) -> &'static str {
    match metric {
        EmbeddingDistanceMetric::Cosine => "cosine",
        EmbeddingDistanceMetric::Dot => "dot",
        EmbeddingDistanceMetric::Euclidean => "euclidean",
    }
}

#[must_use]
pub(crate) fn normalization_token(normalization: EmbeddingNormalization) -> &'static str {
    match normalization {
        EmbeddingNormalization::None => "none",
        EmbeddingNormalization::L2Unit => "l2_unit",
    }
}

/// What the main semantic table committed to at seal time.
pub(crate) struct SemanticRowSealV1 {
    pub(crate) row_count: u64,
    pub(crate) root_digest: String,
    pub(crate) built_at_unix_nanos: u64,
}

/// Which corpora the sealed generation carries, and under which policies.
pub(crate) struct SemanticCorpusCoverageV1 {
    pub(crate) present: Vec<String>,
    pub(crate) required: Vec<String>,
    pub(crate) card_schema_versions: Vec<u32>,
    pub(crate) render_policy_digests: Vec<String>,
    pub(crate) policy_digest: Option<String>,
}

/// What the cluster-membership sidecar committed to at seal time.
pub(crate) struct ClusterMembershipSealV1 {
    pub(crate) root_digest: String,
    pub(crate) cluster_count: u64,
    pub(crate) member_row_count: u64,
}

impl SemanticManifest {
    /// Assemble a manifest from the generation contract plus the four
    /// commitment groups sealed alongside it.
    ///
    /// The groups are separate parameters rather than twenty positional
    /// arguments so a caller cannot transpose, say, the semantic row root and
    /// the membership root: they have different types now, not just different
    /// positions in a long list of `String`s and `u64`s.
    pub(crate) fn from_generation_contract(
        scope: &GenerationPin,
        generation_contract: &GenerationContract,
        manifest_digest: &str,
        rows: SemanticRowSealV1,
        corpora: SemanticCorpusCoverageV1,
        cluster_membership: ClusterMembershipSealV1,
        vector_index: VectorIndexSealV1,
    ) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            repo_id: scope.repo_id.as_str().to_owned(),
            revision_id: scope.revision_id.as_str().to_owned(),
            generation: scope.manifest_generation.get(),
            manifest_digest: manifest_digest.to_owned(),
            model_id: generation_contract.model_id.clone(),
            model_version: generation_contract.model_version.clone(),
            dimension: generation_contract.dimension,
            distance_metric: generation_contract.distance_metric.clone(),
            normalization: generation_contract.normalization.clone(),
            row_count: rows.row_count,
            semantic_row_root_digest: rows.root_digest,
            built_at_unix_nanos: rows.built_at_unix_nanos,
            present_corpora: corpora.present,
            required_corpora: corpora.required,
            card_schema_versions: corpora.card_schema_versions,
            render_policy_digests: corpora.render_policy_digests,
            corpus_policy_digest: corpora.policy_digest,
            cluster_membership_root_digest: cluster_membership.root_digest,
            cluster_membership_cluster_count: cluster_membership.cluster_count,
            cluster_membership_member_row_count: cluster_membership.member_row_count,
            vector_index: Some(vector_index),
        }
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        codec::encode(self, "semantic manifest")
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        match codec::decode(bytes, "semantic manifest") {
            Ok(current) => Ok(current),
            Err(current_err) => {
                if let Ok(legacy) = codec::decode::<SemanticManifestV7>(bytes, "semantic manifest")
                {
                    return Ok(Self {
                        format_version: legacy.format_version,
                        repo_id: legacy.repo_id,
                        revision_id: legacy.revision_id,
                        generation: legacy.generation,
                        manifest_digest: legacy.manifest_digest,
                        model_id: legacy.model_id,
                        model_version: legacy.model_version,
                        dimension: legacy.dimension,
                        distance_metric: legacy.distance_metric,
                        normalization: legacy.normalization,
                        row_count: legacy.row_count,
                        semantic_row_root_digest: legacy.semantic_row_root_digest,
                        built_at_unix_nanos: legacy.built_at_unix_nanos,
                        present_corpora: legacy.present_corpora,
                        required_corpora: legacy.required_corpora,
                        card_schema_versions: legacy.card_schema_versions,
                        render_policy_digests: legacy.render_policy_digests,
                        corpus_policy_digest: legacy.corpus_policy_digest,
                        cluster_membership_root_digest: legacy.cluster_membership_root_digest,
                        cluster_membership_cluster_count: legacy.cluster_membership_cluster_count,
                        cluster_membership_member_row_count: legacy
                            .cluster_membership_member_row_count,
                        vector_index: None,
                    });
                }
                if let Ok(legacy) = codec::decode::<SemanticManifestV6>(bytes, "semantic manifest")
                {
                    return Ok(Self {
                        format_version: legacy.format_version,
                        repo_id: legacy.repo_id,
                        revision_id: legacy.revision_id,
                        generation: legacy.generation,
                        manifest_digest: legacy.manifest_digest,
                        model_id: legacy.model_id,
                        model_version: legacy.model_version,
                        dimension: legacy.dimension,
                        distance_metric: legacy.distance_metric,
                        normalization: legacy.normalization,
                        row_count: legacy.row_count,
                        semantic_row_root_digest: String::new(),
                        built_at_unix_nanos: legacy.built_at_unix_nanos,
                        present_corpora: legacy.present_corpora,
                        required_corpora: legacy.required_corpora,
                        card_schema_versions: legacy.card_schema_versions,
                        render_policy_digests: legacy.render_policy_digests,
                        corpus_policy_digest: legacy.corpus_policy_digest,
                        cluster_membership_root_digest: legacy.cluster_membership_root_digest,
                        cluster_membership_cluster_count: legacy.cluster_membership_cluster_count,
                        cluster_membership_member_row_count: legacy
                            .cluster_membership_member_row_count,
                        vector_index: None,
                    });
                }
                if let Ok(legacy) = codec::decode::<SemanticManifestV5>(bytes, "semantic manifest")
                {
                    return Ok(Self {
                        format_version: legacy.format_version,
                        repo_id: legacy.repo_id,
                        revision_id: legacy.revision_id,
                        generation: legacy.generation,
                        manifest_digest: legacy.manifest_digest,
                        model_id: legacy.model_id,
                        model_version: legacy.model_version,
                        dimension: legacy.dimension,
                        distance_metric: legacy.distance_metric,
                        normalization: legacy.normalization,
                        row_count: legacy.row_count,
                        semantic_row_root_digest: String::new(),
                        built_at_unix_nanos: legacy.built_at_unix_nanos,
                        present_corpora: legacy.present_corpora,
                        required_corpora: legacy.required_corpora,
                        card_schema_versions: legacy.card_schema_versions,
                        render_policy_digests: legacy.render_policy_digests,
                        corpus_policy_digest: legacy.corpus_policy_digest,
                        cluster_membership_root_digest: String::new(),
                        cluster_membership_cluster_count: 0,
                        cluster_membership_member_row_count: 0,
                        vector_index: None,
                    });
                }
                if let Ok(legacy_v3) =
                    codec::decode::<SemanticManifestV3>(bytes, "semantic manifest")
                {
                    return Ok(Self {
                        format_version: legacy_v3.format_version,
                        repo_id: legacy_v3.repo_id,
                        revision_id: legacy_v3.revision_id,
                        generation: legacy_v3.generation,
                        manifest_digest: legacy_v3.manifest_digest,
                        model_id: legacy_v3.model_id,
                        model_version: legacy_v3.model_version,
                        dimension: legacy_v3.dimension,
                        distance_metric: legacy_v3.distance_metric,
                        normalization: legacy_v3.normalization,
                        row_count: legacy_v3.row_count,
                        semantic_row_root_digest: String::new(),
                        built_at_unix_nanos: legacy_v3.built_at_unix_nanos,
                        present_corpora: Vec::new(),
                        required_corpora: Vec::new(),
                        card_schema_versions: Vec::new(),
                        render_policy_digests: Vec::new(),
                        corpus_policy_digest: None,
                        cluster_membership_root_digest: String::new(),
                        cluster_membership_cluster_count: 0,
                        cluster_membership_member_row_count: 0,
                        vector_index: None,
                    });
                }
                if let Ok(legacy_v2) =
                    codec::decode::<SemanticManifestV3>(bytes, "semantic manifest")
                {
                    return Ok(Self {
                        format_version: legacy_v2.format_version,
                        repo_id: legacy_v2.repo_id,
                        revision_id: legacy_v2.revision_id,
                        generation: legacy_v2.generation,
                        manifest_digest: legacy_v2.manifest_digest,
                        model_id: legacy_v2.model_id,
                        model_version: legacy_v2.model_version,
                        dimension: legacy_v2.dimension,
                        distance_metric: legacy_v2.distance_metric,
                        normalization: legacy_v2.normalization,
                        row_count: legacy_v2.row_count,
                        semantic_row_root_digest: String::new(),
                        built_at_unix_nanos: legacy_v2.built_at_unix_nanos,
                        present_corpora: Vec::new(),
                        required_corpora: Vec::new(),
                        card_schema_versions: Vec::new(),
                        render_policy_digests: Vec::new(),
                        corpus_policy_digest: None,
                        cluster_membership_root_digest: String::new(),
                        cluster_membership_cluster_count: 0,
                        cluster_membership_member_row_count: 0,
                        vector_index: None,
                    });
                }
                Err(current_err)
            }
        }
    }

    /// What this manifest's format carries and proves.
    pub(crate) fn capabilities(&self) -> Result<FormatCapabilitiesV1, CoreError> {
        format_capabilities_v1(self.format_version)
            .ok_or_else(|| unsupported_format(self.format_version))
    }

    /// The sealed index contract a current-format manifest carries.
    pub(crate) fn vector_index_seal(&self) -> Result<Option<&VectorIndexSealV1>, CoreError> {
        let capabilities = self.capabilities()?;
        match (capabilities.vector_index_seal(), self.vector_index.as_ref()) {
            (true, Some(seal)) => Ok(Some(seal)),
            (true, None) => Err(CoreError::Storage(format!(
                "semantic: manifest format version {} requires a vector index seal",
                self.format_version
            ))),
            (false, Some(_)) => Err(CoreError::Storage(format!(
                "semantic: manifest format version {} predates the vector index seal it carries",
                self.format_version
            ))),
            (false, None) => Ok(None),
        }
    }

    /// Fail closed unless the manifest describes exactly the requested scope and
    /// a supported format version.
    pub(crate) fn validate_scope(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<(), CoreError> {
        let capabilities = self.capabilities()?;
        if capabilities.semantic_row_root()
            && !is_canonical_sha256_v1(&self.semantic_row_root_digest)
        {
            return Err(CoreError::Storage(
                "semantic: current manifest has an invalid semantic row root digest".to_string(),
            ));
        }
        if capabilities.membership_commitment()
            && !is_canonical_sha256_v1(&self.cluster_membership_root_digest)
        {
            return Err(CoreError::Storage(
                "semantic: current manifest has an invalid cluster membership root digest"
                    .to_string(),
            ));
        }
        if capabilities.membership_commitment()
            && ((self.cluster_membership_cluster_count == 0)
                != (self.cluster_membership_member_row_count == 0)
                || self.cluster_membership_cluster_count > self.cluster_membership_member_row_count)
        {
            return Err(CoreError::Storage(
                "semantic: current manifest has inconsistent cluster membership counts".to_string(),
            ));
        }
        if let Some(seal) = self.vector_index_seal()? {
            seal.validate(self.row_count)?;
        }
        if self.repo_id != repo.as_str() {
            return Err(CoreError::Storage(format!(
                "semantic: manifest repo `{}` does not match requested `{}`",
                self.repo_id,
                repo.as_str()
            )));
        }
        if self.revision_id != revision.as_str() {
            return Err(CoreError::Storage(format!(
                "semantic: manifest revision `{}` does not match requested `{}`",
                self.revision_id,
                revision.as_str()
            )));
        }
        if self.generation != generation.get() {
            return Err(CoreError::Storage(format!(
                "semantic: manifest generation {} does not match requested {}",
                self.generation,
                generation.get()
            )));
        }
        if self.distance_metric != SUPPORTED_DISTANCE_METRIC {
            return Err(CoreError::Storage(format!(
                "semantic: manifest distance_metric `{}` is not supported by this adapter (serves `{SUPPORTED_DISTANCE_METRIC}` only)",
                self.distance_metric
            )));
        }
        self.validate_corpus_coverage()?;
        Ok(())
    }

    pub(crate) fn validate_corpus_coverage(&self) -> Result<(), CoreError> {
        let present: BTreeSet<&str> = self.present_corpora.iter().map(String::as_str).collect();
        if present.len() != self.present_corpora.len() {
            return Err(CoreError::Storage(
                "semantic: manifest present_corpora contains duplicates".to_string(),
            ));
        }
        let required: BTreeSet<&str> = self.required_corpora.iter().map(String::as_str).collect();
        if required.len() != self.required_corpora.len() {
            return Err(CoreError::Storage(
                "semantic: manifest required_corpora contains duplicates".to_string(),
            ));
        }
        for corpus in &required {
            if !present.contains(corpus) {
                return Err(CoreError::Storage(format!(
                    "semantic: manifest required corpus `{corpus}` missing from present_corpora"
                )));
            }
        }
        if self
            .corpus_policy_digest
            .as_deref()
            .is_some_and(str::is_empty)
        {
            return Err(CoreError::Storage(
                "semantic: manifest corpus_policy_digest must not be empty when present"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_v6_decodes_without_fabricating_semantic_row_root_v1() {
        let legacy = SemanticManifestV6 {
            format_version: LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION,
            repo_id: "repo".to_string(),
            revision_id: "rev".to_string(),
            generation: 7,
            manifest_digest: "manifest".to_string(),
            model_id: "model".to_string(),
            model_version: None,
            dimension: 3,
            distance_metric: "cosine".to_string(),
            normalization: "l2_unit".to_string(),
            row_count: 1,
            built_at_unix_nanos: 0,
            present_corpora: vec!["ClusterCard".to_string()],
            required_corpora: vec!["ClusterCard".to_string()],
            card_schema_versions: vec![1],
            render_policy_digests: vec!["render".to_string()],
            corpus_policy_digest: Some("policy".to_string()),
            cluster_membership_root_digest: format!("sha256:{}", "1".repeat(64)),
            cluster_membership_cluster_count: 1,
            cluster_membership_member_row_count: 1,
        };
        let bytes = codec::encode(&legacy, "legacy semantic manifest").expect("encode legacy");
        let decoded = SemanticManifest::decode(&bytes).expect("decode legacy");
        assert_eq!(
            decoded.format_version,
            LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION
        );
        assert!(decoded.semantic_row_root_digest.is_empty());
        decoded
            .validate_scope(
                &RepoId::new("repo"),
                &RevisionId::new("rev"),
                ManifestGeneration::new(7),
            )
            .expect("v6 membership proof remains valid");
    }

    #[test]
    fn format_v5_decodes_without_fabricating_membership_commitment_v1() {
        let legacy = SemanticManifestV5 {
            format_version: LEGACY_UNCOMMITTED_MEMBERSHIP_FORMAT_VERSION,
            repo_id: "repo".to_string(),
            revision_id: "rev".to_string(),
            generation: 7,
            manifest_digest: "manifest".to_string(),
            model_id: "model".to_string(),
            model_version: None,
            dimension: 3,
            distance_metric: "cosine".to_string(),
            normalization: "l2_unit".to_string(),
            row_count: 1,
            built_at_unix_nanos: 0,
            present_corpora: vec!["ClusterCard".to_string()],
            required_corpora: vec!["ClusterCard".to_string()],
            card_schema_versions: vec![1],
            render_policy_digests: vec!["render".to_string()],
            corpus_policy_digest: Some("policy".to_string()),
        };
        let bytes = codec::encode(&legacy, "legacy semantic manifest").expect("encode legacy");
        let decoded = SemanticManifest::decode(&bytes).expect("decode legacy");
        assert_eq!(
            decoded.format_version,
            LEGACY_UNCOMMITTED_MEMBERSHIP_FORMAT_VERSION
        );
        assert!(decoded.cluster_membership_root_digest.is_empty());
        assert_eq!(decoded.cluster_membership_cluster_count, 0);
        assert_eq!(decoded.cluster_membership_member_row_count, 0);
    }

    fn v7_manifest(row_count: u64) -> SemanticManifestV7 {
        SemanticManifestV7 {
            format_version: LEGACY_UNSEALED_VECTOR_INDEX_FORMAT_VERSION,
            repo_id: "repo".to_string(),
            revision_id: "rev".to_string(),
            generation: 7,
            manifest_digest: "manifest".to_string(),
            model_id: "model".to_string(),
            model_version: Some("rev-1".to_string()),
            dimension: 3,
            distance_metric: "cosine".to_string(),
            normalization: "l2_unit".to_string(),
            row_count,
            semantic_row_root_digest: format!("sha256:{}", "2".repeat(64)),
            built_at_unix_nanos: 0,
            present_corpora: Vec::new(),
            required_corpora: Vec::new(),
            card_schema_versions: Vec::new(),
            render_policy_digests: Vec::new(),
            corpus_policy_digest: None,
            cluster_membership_root_digest: format!("sha256:{}", "1".repeat(64)),
            cluster_membership_cluster_count: 0,
            cluster_membership_member_row_count: 0,
        }
    }

    fn ann_seal(indexed_rows: u64) -> VectorIndexSealV1 {
        VectorIndexSealV1 {
            mode: VECTOR_INDEX_MODE_IVF_HNSW_SQ.to_string(),
            library: "lancedb".to_string(),
            library_version: "0.30.0".to_string(),
            index_min_rows: 256,
            ann: Some(AnnIndexSealV1 {
                index_name: "vector_ivf_hnsw_sq".to_string(),
                distance: "cosine".to_string(),
                num_partitions: 1,
                sample_rate: 256,
                max_iterations: 50,
                hnsw_m: 20,
                hnsw_ef_construction: 300,
                indexed_rows,
                index_segments: 1,
                nprobes: 1,
                ef_floor: 64,
                ef_per_candidate: 2,
                refine_factor: 2,
            }),
        }
    }

    fn exact_seal() -> VectorIndexSealV1 {
        VectorIndexSealV1 {
            mode: VECTOR_INDEX_MODE_EXACT.to_string(),
            library: "lancedb".to_string(),
            library_version: "0.30.0".to_string(),
            index_min_rows: 256,
            ann: None,
        }
    }

    #[test]
    fn format_v7_decodes_without_fabricating_a_vector_index_seal() {
        let bytes = codec::encode(&v7_manifest(300), "legacy semantic manifest").expect("encode");
        let decoded = SemanticManifest::decode(&bytes).expect("decode legacy");
        assert_eq!(
            decoded.format_version,
            LEGACY_UNSEALED_VECTOR_INDEX_FORMAT_VERSION
        );
        assert!(decoded.vector_index.is_none());
        assert_eq!(
            decoded.semantic_row_root_digest,
            format!("sha256:{}", "2".repeat(64))
        );
        let seal = decoded
            .vector_index_seal()
            .expect("a v7 manifest is supported");
        assert!(seal.is_none(), "v7 has no seal to report");
        decoded
            .validate_scope(
                &RepoId::new("repo"),
                &RevisionId::new("rev"),
                ManifestGeneration::new(7),
            )
            .expect("v7 row-root proof remains valid");
    }

    #[test]
    fn a_current_manifest_round_trips_its_vector_index_seal() {
        let mut manifest =
            SemanticManifest::decode(&codec::encode(&v7_manifest(300), "fixture").expect("encode"))
                .expect("decode");
        manifest.format_version = FORMAT_VERSION;
        manifest.vector_index = Some(ann_seal(300));
        let bytes = manifest.encode().expect("encode current");
        let decoded = SemanticManifest::decode(&bytes).expect("decode current");
        assert_eq!(decoded.format_version, FORMAT_VERSION);
        let seal = decoded
            .vector_index_seal()
            .expect("supported")
            .expect("current format carries a seal");
        assert_eq!(seal.mode, VECTOR_INDEX_MODE_IVF_HNSW_SQ);
        let ann = seal.ann.as_ref().expect("ann record");
        assert_eq!(ann.index_name, "vector_ivf_hnsw_sq");
        assert_eq!(ann.indexed_rows, 300);
        assert_eq!((ann.hnsw_m, ann.hnsw_ef_construction), (20, 300));
        decoded
            .validate_scope(
                &RepoId::new("repo"),
                &RevisionId::new("rev"),
                ManifestGeneration::new(7),
            )
            .expect("consistent seal");
    }

    #[test]
    fn a_current_manifest_without_a_seal_is_refused() {
        let mut manifest =
            SemanticManifest::decode(&codec::encode(&v7_manifest(300), "fixture").expect("encode"))
                .expect("decode");
        manifest.format_version = FORMAT_VERSION;
        let error = manifest
            .validate_scope(
                &RepoId::new("repo"),
                &RevisionId::new("rev"),
                ManifestGeneration::new(7),
            )
            .expect_err("format 8 without a seal");
        assert!(
            error.to_string().contains("requires a vector index seal"),
            "{error}"
        );
    }

    fn capability_bits(capabilities: FormatCapabilitiesV1) -> [bool; 6] {
        [
            capabilities.build_contract(),
            capabilities.corpus_metadata(),
            capabilities.membership_commitment(),
            capabilities.semantic_row_root(),
            capabilities.file_commitment(),
            capabilities.vector_index_seal(),
        ]
    }

    #[test]
    fn format_capabilities_are_monotone_and_bounded() {
        assert!(format_capabilities_v1(1).is_none());
        assert!(format_capabilities_v1(FORMAT_VERSION.saturating_add(1)).is_none());
        let v2 = format_capabilities_v1(2).expect("v2");
        assert_eq!(capability_bits(v2), [false; 6]);
        let v7 = format_capabilities_v1(7).expect("v7");
        assert_eq!(capability_bits(v7), [true, true, true, true, true, false]);
        let v8 = format_capabilities_v1(8).expect("v8");
        assert_eq!(capability_bits(v8), [true; 6]);
        // Each capability, once gained, is never lost by a later format.
        let mut previous = capability_bits(v2);
        for version in 3..=FORMAT_VERSION {
            let current = capability_bits(format_capabilities_v1(version).expect("supported"));
            assert!(
                previous
                    .iter()
                    .zip(current.iter())
                    .all(|(before, after)| !before || *after),
                "format {version} lost a capability: {previous:?} -> {current:?}"
            );
            previous = current;
        }
    }

    #[test]
    fn the_vector_index_seal_refuses_every_contradiction() {
        exact_seal().validate(255).expect("exact below the floor");
        ann_seal(256).validate(256).expect("ann at the floor");
        let cases: Vec<(&str, VectorIndexSealV1, u64)> = vec![
            ("exact at the floor", exact_seal(), 256),
            ("ann below the floor", ann_seal(255), 255),
            ("ann rows differ from the manifest", ann_seal(300), 301),
            (
                "exact with an index record",
                VectorIndexSealV1 {
                    mode: VECTOR_INDEX_MODE_EXACT.to_string(),
                    ..ann_seal(300)
                },
                255,
            ),
            (
                "ann without an index record",
                VectorIndexSealV1 {
                    ann: None,
                    ..ann_seal(300)
                },
                300,
            ),
            (
                "unknown mode",
                VectorIndexSealV1 {
                    mode: "flat".to_string(),
                    ..exact_seal()
                },
                10,
            ),
            (
                "empty provenance",
                VectorIndexSealV1 {
                    library_version: String::new(),
                    ..exact_seal()
                },
                10,
            ),
        ];
        for (label, seal, rows) in cases {
            assert!(seal.validate(rows).is_err(), "{label} must be refused");
        }
        let mut wrong_distance = ann_seal(300);
        if let Some(ann) = wrong_distance.ann.as_mut() {
            ann.distance = "dot".to_string();
        }
        assert!(wrong_distance.validate(300).is_err(), "dot distance");
        let mut too_many_probes = ann_seal(300);
        if let Some(ann) = too_many_probes.ann.as_mut() {
            ann.nprobes = 2;
        }
        assert!(
            too_many_probes.validate(300).is_err(),
            "nprobes beyond partitions"
        );
        let mut zero_refine = ann_seal(300);
        if let Some(ann) = zero_refine.ann.as_mut() {
            ann.refine_factor = 0;
        }
        assert!(zero_refine.validate(300).is_err(), "zero refine factor");
    }
}
