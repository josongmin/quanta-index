//! Persisted semantic generation manifest (LDB-01 manifest contract).
//!
//! Internal to `quanta-index`, but correctness-critical: the open path fails
//! closed on a format, scope, or integrity mismatch rather than guessing around
//! missing or inconsistent state.
//!
//! Exactly one format is served. A manifest written under any other format
//! is refused typed as `GENERATION_MANIFEST_FORMAT_UNSUPPORTED` at every
//! door — decode, open, validate, inventory — with the instruction to
//! rebuild the generation from its producer; nothing is decoded through an
//! older shape and nothing is fabricated for a field an older shape lacked.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::collections::BTreeSet;

use quanta_index_contract::{
    EmbeddingNormalization, GenerationPin, ManifestGeneration, RepoId, RevisionId,
};
use quanta_index_core::CoreError;

use crate::codec::{self, cbor_serde};
use crate::codec::{decode_current_format, format_unsupported};
use crate::generation_contract::{GenerationContract, ensure_field_eq, normalization_token};

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

/// The one manifest format this adapter writes and serves. Bumped on any
/// durable shape change; every earlier format is refused typed.
///
/// `10` = the approximate index's per-segment build record (QI-BB-027):
/// the graph parameters every segment was actually built with, read back
/// from the library, so an appended segment is never claimed to carry the
/// trained recipe.
/// `11` = typed-source-only semantic derivation. Format 10 may contain
/// chunk-text fallback rows and is not admitted as a new-generation base.
// Format 12 requires generation-wide record_id and embedding_id uniqueness at
// seal. Older artifacts cannot prove those identities and must be rebuilt.
pub(crate) const FORMAT_VERSION: u32 = 12;

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
/// reported about it, the effort the lane spends in it, where its
/// centroids came from, and how each segment was actually built.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AnnIndexSealV1 {
    pub(crate) index_name: String,
    pub(crate) distance: String,
    pub(crate) num_partitions: u32,
    pub(crate) sample_rate: u32,
    pub(crate) max_iterations: u32,
    /// The graph recipe the policy trains with; the trained segment carries
    /// it, appended segments carry whatever [`Self::segments`] records.
    pub(crate) hnsw_m: u32,
    pub(crate) hnsw_ef_construction: u32,
    pub(crate) indexed_rows: u64,
    pub(crate) index_segments: u32,
    pub(crate) nprobes: u32,
    pub(crate) ef_floor: u32,
    pub(crate) ef_per_candidate: u32,
    pub(crate) refine_factor: u32,
    pub(crate) lineage: AnnIndexLineageV1,
    /// Every segment of the index in the library's listing order, with the
    /// graph parameters the library reports it was built with. The first
    /// is the trained segment; the rest were appended.
    pub(crate) segments: Vec<AnnIndexSegmentSealV1>,
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
    lineage: AnnIndexLineageV1,
    segments: Vec<AnnIndexSegmentSealV1>,
});

/// One segment of the approximate index as the library reports it: its
/// identity and the graph parameters it was actually built with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AnnIndexSegmentSealV1 {
    pub(crate) uuid: String,
    pub(crate) hnsw_m: u32,
    pub(crate) hnsw_ef_construction: u32,
}

cbor_serde!(AnnIndexSegmentSealV1 {
    uuid: String,
    hnsw_m: u32,
    hnsw_ef_construction: u32,
});

/// Where the sealed index's centroids came from and how far the served
/// rows have drifted from them (QI-BB-027 W3).
///
/// A delta seal appends its rows to the inherited index when the record
/// stays inside the append budget, and retrains otherwise; the budget the
/// seal was under is recorded beside the counters so the record validates
/// on its own, and a policy change cannot enter under an old seal's
/// identity. The counters satisfy
/// `trained_rows + appended_rows - deleted_rows == indexed_rows`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AnnIndexLineageV1 {
    /// The generation whose seal trained the centroids.
    pub(crate) trained_at_generation: u64,
    /// The rows the centroids were trained on: every row that seal covered.
    pub(crate) trained_rows: u64,
    /// Rows later seals assigned to those centroids without retraining,
    /// cumulative.
    pub(crate) appended_rows: u64,
    /// Rows removed from the index since training, cumulative.
    pub(crate) deleted_rows: u64,
    /// The append budget as rows per thousand trained rows.
    pub(crate) append_ratio_max_per_mille: u32,
    /// The append budget as an absolute row count.
    pub(crate) append_rows_max: u64,
    /// The most segments appends may add beside the trained one.
    pub(crate) append_segments_max: u32,
}

cbor_serde!(AnnIndexLineageV1 {
    trained_at_generation: u64,
    trained_rows: u64,
    appended_rows: u64,
    deleted_rows: u64,
    append_ratio_max_per_mille: u32,
    append_rows_max: u64,
    append_segments_max: u32,
});

impl AnnIndexLineageV1 {
    /// Live rows the record accounts for: `trained + appended - deleted`,
    /// or `None` when the counters do not add up.
    #[must_use]
    pub(crate) fn accounted_rows(&self) -> Option<u64> {
        self.trained_rows
            .checked_add(self.appended_rows)?
            .checked_sub(self.deleted_rows)
    }

    /// Whether `appended_rows` stays inside both recorded budgets; `None`
    /// when the ratio product overflows.
    #[must_use]
    pub(crate) fn within_append_budget(&self) -> Option<bool> {
        let scaled_appended = self.appended_rows.checked_mul(1_000)?;
        let scaled_budget = self
            .trained_rows
            .checked_mul(u64::from(self.append_ratio_max_per_mille))?;
        Some(scaled_appended <= scaled_budget && self.appended_rows <= self.append_rows_max)
    }

    /// Refuse a record that contradicts itself, its generation, or the
    /// index it describes.
    fn validate(&self, generation: u64, ann: &AnnIndexSealV1) -> Result<(), CoreError> {
        let invalid = |detail: &str| {
            CoreError::Storage(format!(
                "semantic: manifest vector index lineage is inconsistent: {detail}"
            ))
        };
        if self.trained_rows == 0 {
            return Err(invalid("trained on zero rows"));
        }
        if self.trained_at_generation > generation {
            return Err(invalid(&format!(
                "trained at generation {} after the sealing generation {generation}",
                self.trained_at_generation
            )));
        }
        if self.trained_at_generation == generation
            && (self.appended_rows != 0 || self.deleted_rows != 0)
        {
            return Err(invalid(&format!(
                "trained by this seal yet {} rows appended and {} deleted",
                self.appended_rows, self.deleted_rows
            )));
        }
        match self.within_append_budget() {
            Some(true) => {}
            Some(false) => {
                return Err(invalid(&format!(
                    "{} appended rows exceed the budget of {} per mille of {} trained rows or {} rows",
                    self.appended_rows,
                    self.append_ratio_max_per_mille,
                    self.trained_rows,
                    self.append_rows_max
                )));
            }
            None => return Err(invalid("append budget arithmetic overflows")),
        }
        let segments_max = self.append_segments_max.saturating_add(1);
        if ann.index_segments > segments_max {
            return Err(invalid(&format!(
                "{} segments exceed one trained plus {} appended",
                ann.index_segments, self.append_segments_max
            )));
        }
        match self.accounted_rows() {
            Some(accounted) if accounted == ann.indexed_rows => Ok(()),
            Some(accounted) => Err(invalid(&format!(
                "{} trained + {} appended - {} deleted = {accounted} rows, but the index covers {}",
                self.trained_rows, self.appended_rows, self.deleted_rows, ann.indexed_rows
            ))),
            None => Err(invalid("row accounting overflows")),
        }
    }
}

impl AnnIndexSealV1 {
    /// Refuse a segment record that does not describe every segment, that
    /// claims the trained recipe for a segment the library did not build
    /// with it, or whose trained segment is not the recipe.
    ///
    /// The first listed segment is the one the seal trained and must carry
    /// exactly `(hnsw_m, hnsw_ef_construction)`; every later one was
    /// appended by the library's incremental builder and is recorded as
    /// the library reported it. A segment without an identity or with a
    /// zero parameter was never read back from the library.
    fn validate_segments(&self) -> Result<(), CoreError> {
        let invalid = |detail: &str| {
            CoreError::Storage(format!(
                "semantic: manifest vector index segment record is inconsistent: {detail}"
            ))
        };
        let recorded = u32::try_from(self.segments.len()).map_err(|error| {
            invalid(&format!(
                "more segment records than the segment count can hold: {error}"
            ))
        })?;
        if recorded != self.index_segments {
            return Err(invalid(&format!(
                "{recorded} segment records for {} segments",
                self.index_segments
            )));
        }
        let mut seen = BTreeSet::new();
        for (position, segment) in self.segments.iter().enumerate() {
            if segment.uuid.is_empty() {
                return Err(invalid(&format!("segment {position} has no identity")));
            }
            if !seen.insert(segment.uuid.as_str()) {
                return Err(invalid(&format!(
                    "segment {} is recorded twice",
                    segment.uuid
                )));
            }
            if segment.hnsw_m == 0 || segment.hnsw_ef_construction == 0 {
                return Err(invalid(&format!(
                    "segment {} records a zero graph parameter",
                    segment.uuid
                )));
            }
        }
        let Some(trained) = self.segments.first() else {
            return Err(invalid("no trained segment"));
        };
        if trained.hnsw_m != self.hnsw_m
            || trained.hnsw_ef_construction != self.hnsw_ef_construction
        {
            return Err(invalid(&format!(
                "the trained segment {} was built with m={} ef_construction={}, the recipe says m={} ef_construction={}",
                trained.uuid,
                trained.hnsw_m,
                trained.hnsw_ef_construction,
                self.hnsw_m,
                self.hnsw_ef_construction
            )));
        }
        Ok(())
    }

    /// The segments appended after the trained one, in order.
    #[must_use]
    pub(crate) fn appended_segments(&self) -> &[AnnIndexSegmentSealV1] {
        self.segments.get(1..).unwrap_or_default()
    }
}

pub(crate) const VECTOR_INDEX_MODE_EXACT: &str = "exact";
pub(crate) const VECTOR_INDEX_MODE_IVF_HNSW_SQ: &str = "ivf_hnsw_sq";

impl VectorIndexSealV1 {
    /// Refuse a seal whose fields contradict each other, the row count or
    /// the sealing generation.
    pub(crate) fn validate(&self, row_count: u64, generation: u64) -> Result<(), CoreError> {
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
                ann.lineage.validate(generation, ann)?;
                ann.validate_segments()?;
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
    pub(crate) vector_index: VectorIndexSealV1,
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
    vector_index: VectorIndexSealV1,
});

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
    /// Refuse a manifest that disagrees with the generation's durable build
    /// contract, or a contract under a format this adapter does not serve.
    pub(crate) fn validate_against(&self, contract: &GenerationContract) -> Result<(), CoreError> {
        contract.validate_format()?;
        ensure_field_eq!(
            CoreError::Storage,
            self.model_id,
            contract.model_id,
            "semantic: manifest model_id `{}` does not match sealed build contract `{}`"
        );
        ensure_field_eq!(
            CoreError::Storage,
            self.model_version,
            contract.model_version,
            "semantic: manifest model_version {:?} does not match sealed build contract {:?}"
        );
        ensure_field_eq!(
            CoreError::Storage,
            self.dimension,
            contract.dimension,
            "semantic: manifest dimension {} does not match sealed build contract {}"
        );
        ensure_field_eq!(
            CoreError::Storage,
            self.distance_metric,
            contract.distance_metric,
            "semantic: manifest distance_metric `{}` does not match sealed build contract `{}`"
        );
        ensure_field_eq!(
            CoreError::Storage,
            self.normalization,
            contract.normalization,
            "semantic: manifest normalization `{}` does not match sealed build contract `{}`"
        );
        ensure_field_eq!(
            CoreError::Storage,
            self.required_corpora,
            contract.required_corpora,
            "semantic: manifest required_corpora {:?} does not match sealed build contract {:?}"
        );
        ensure_field_eq!(
            CoreError::Storage,
            self.corpus_policy_digest,
            contract.corpus_policy_digest,
            "semantic: manifest corpus_policy_digest {:?} does not match sealed build contract {:?}"
        );
        Ok(())
    }

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
            vector_index,
        }
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        let bytes = codec::encode(self, "semantic manifest")?;
        crate::control_file::ensure_bounded(
            &bytes,
            crate::control_file::MAX_SCOPE_MANIFEST_BYTES,
            "scope manifest",
        )?;
        Ok(bytes)
    }

    /// Decode the current format only; any other format is refused typed.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        decode_current_format(bytes, "semantic manifest", FORMAT_VERSION)
    }

    /// Fail closed unless the manifest describes exactly the requested scope
    /// and is self-consistent.
    pub(crate) fn validate_scope(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<(), CoreError> {
        if self.format_version != FORMAT_VERSION {
            return Err(format_unsupported(
                "semantic manifest",
                self.format_version,
                FORMAT_VERSION,
            ));
        }
        if !is_canonical_sha256_v1(&self.semantic_row_root_digest) {
            return Err(CoreError::Storage(
                "semantic: manifest has an invalid semantic row root digest".to_string(),
            ));
        }
        if !is_canonical_sha256_v1(&self.cluster_membership_root_digest) {
            return Err(CoreError::Storage(
                "semantic: manifest has an invalid cluster membership root digest".to_string(),
            ));
        }
        if (self.cluster_membership_cluster_count == 0)
            != (self.cluster_membership_member_row_count == 0)
            || self.cluster_membership_cluster_count > self.cluster_membership_member_row_count
        {
            return Err(CoreError::Storage(
                "semantic: manifest has inconsistent cluster membership counts".to_string(),
            ));
        }
        self.vector_index
            .validate(self.row_count, self.generation)?;
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
        let _normalization = self.normalization_contract()?;
        self.validate_corpus_coverage()?;
        Ok(())
    }

    /// The normalization every row was held to at ingest, as the contract
    /// names it: what every query vector is held to at the door
    /// (QI-BB-031). A token this adapter does not write is refused, never
    /// read as "no normalization".
    pub(crate) fn normalization_contract(&self) -> Result<EmbeddingNormalization, CoreError> {
        [EmbeddingNormalization::None, EmbeddingNormalization::L2Unit]
            .into_iter()
            .find(|normalization| normalization_token(*normalization) == self.normalization)
            .ok_or_else(|| {
                CoreError::Storage(format!(
                    "semantic: manifest normalization `{}` is not one this adapter serves (`none`, `l2_unit`)",
                    self.normalization
                ))
            })
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
    use crate::codec::FORMAT_UNSUPPORTED_CODE;

    /// The generation the fixtures seal; lineage records name it.
    const GENERATION: u64 = 7;

    /// A lineage as a fresh train at [`GENERATION`] records it.
    fn trained_lineage(rows: u64) -> AnnIndexLineageV1 {
        AnnIndexLineageV1 {
            trained_at_generation: GENERATION,
            trained_rows: rows,
            appended_rows: 0,
            deleted_rows: 0,
            append_ratio_max_per_mille: 250,
            append_rows_max: 1 << 18,
            append_segments_max: 8,
        }
    }

    fn trained_segment(uuid: &str) -> AnnIndexSegmentSealV1 {
        AnnIndexSegmentSealV1 {
            uuid: uuid.to_string(),
            hnsw_m: 20,
            hnsw_ef_construction: 300,
        }
    }

    fn appended_segment(uuid: &str) -> AnnIndexSegmentSealV1 {
        AnnIndexSegmentSealV1 {
            uuid: uuid.to_string(),
            hnsw_m: 20,
            hnsw_ef_construction: 150,
        }
    }

    fn ann_record(indexed_rows: u64, lineage: AnnIndexLineageV1) -> AnnIndexSealV1 {
        AnnIndexSealV1 {
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
            lineage,
            segments: vec![trained_segment("trained")],
        }
    }

    fn ann_seal(indexed_rows: u64) -> VectorIndexSealV1 {
        VectorIndexSealV1 {
            mode: VECTOR_INDEX_MODE_IVF_HNSW_SQ.to_string(),
            library: "lancedb".to_string(),
            library_version: "0.30.0".to_string(),
            index_min_rows: 256,
            ann: Some(ann_record(indexed_rows, trained_lineage(indexed_rows))),
        }
    }

    /// An appended seal: trained on `trained` rows at generation 1,
    /// `appended` rows assigned and `deleted` removed since, in `segments`
    /// segments each recorded with the incremental builder's parameters.
    fn appended_seal(
        trained: u64,
        appended: u64,
        deleted: u64,
        segments: u32,
    ) -> VectorIndexSealV1 {
        let mut ann = ann_record(
            trained.saturating_add(appended).saturating_sub(deleted),
            AnnIndexLineageV1 {
                trained_at_generation: 1,
                trained_rows: trained,
                appended_rows: appended,
                deleted_rows: deleted,
                ..trained_lineage(trained)
            },
        );
        ann.index_segments = segments;
        ann.segments = (0..segments)
            .map(|position| {
                if position == 0 {
                    trained_segment("trained")
                } else {
                    appended_segment(&format!("appended-{position}"))
                }
            })
            .collect();
        VectorIndexSealV1 {
            ann: Some(ann),
            ..ann_seal(trained)
        }
    }

    fn with_lineage(
        seal: &VectorIndexSealV1,
        edit: impl FnOnce(&mut AnnIndexLineageV1),
    ) -> VectorIndexSealV1 {
        let mut edited = seal.clone();
        if let Some(lineage) = edited.ann.as_mut().map(|ann| &mut ann.lineage) {
            edit(lineage);
        }
        edited
    }

    fn with_ann(
        seal: &VectorIndexSealV1,
        edit: impl FnOnce(&mut AnnIndexSealV1),
    ) -> VectorIndexSealV1 {
        let mut edited = seal.clone();
        if let Some(ann) = edited.ann.as_mut() {
            edit(ann);
        }
        edited
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

    fn manifest(row_count: u64, vector_index: VectorIndexSealV1) -> SemanticManifest {
        SemanticManifest {
            format_version: FORMAT_VERSION,
            repo_id: "repo".to_string(),
            revision_id: "rev".to_string(),
            generation: GENERATION,
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
            vector_index,
        }
    }

    fn validate(manifest: &SemanticManifest) -> Result<(), CoreError> {
        manifest.validate_scope(
            &RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(GENERATION),
        )
    }

    /// The normalization token names the contract every query vector is
    /// held to; a token this adapter never writes is refused at validation,
    /// not read as "no normalization" (QI-BB-031).
    #[test]
    fn the_normalization_token_is_the_contract_or_a_refusal() {
        for normalization in [EmbeddingNormalization::None, EmbeddingNormalization::L2Unit] {
            let mut sealed = manifest(10, exact_seal());
            sealed.normalization = normalization_token(normalization).to_string();
            assert_eq!(
                sealed.normalization_contract().expect("a written token"),
                normalization
            );
            validate(&sealed).expect("a written token validates");
        }
        for foreign in ["", "L2Unit", "l2", "unit"] {
            let mut sealed = manifest(10, exact_seal());
            sealed.normalization = foreign.to_string();
            match (sealed.normalization_contract(), validate(&sealed)) {
                (Err(CoreError::Storage(read)), Err(CoreError::Storage(validated))) => {
                    assert!(read.contains("is not one this adapter serves"), "{read}");
                    assert_eq!(read, validated);
                }
                other => panic!("the token `{foreign}` must be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_current_manifest_round_trips_its_vector_index_seal() {
        let manifest = manifest(357, appended_seal(300, 60, 3, 2));
        let bytes = manifest.encode().expect("encode current");
        let decoded = SemanticManifest::decode(&bytes).expect("decode current");
        assert_eq!(decoded.format_version, FORMAT_VERSION);
        let seal = &decoded.vector_index;
        assert_eq!(seal.mode, VECTOR_INDEX_MODE_IVF_HNSW_SQ);
        let ann = seal.ann.as_ref().expect("ann record");
        assert_eq!(ann.index_name, "vector_ivf_hnsw_sq");
        assert_eq!(ann.indexed_rows, 357);
        assert_eq!((ann.hnsw_m, ann.hnsw_ef_construction), (20, 300));
        assert_eq!(
            ann.lineage,
            AnnIndexLineageV1 {
                trained_at_generation: 1,
                trained_rows: 300,
                appended_rows: 60,
                deleted_rows: 3,
                append_ratio_max_per_mille: 250,
                append_rows_max: 1 << 18,
                append_segments_max: 8,
            }
        );
        assert_eq!(
            ann.appended_segments(),
            [appended_segment("appended-1")],
            "the appended segment is recorded with the builder's own parameters"
        );
        validate(&decoded).expect("consistent seal");
    }

    /// Every other format — older, newer, or a shape that names the
    /// current format but is not it — is refused typed, never decoded
    /// through an older shape.
    #[test]
    fn every_other_format_is_refused_typed_with_a_rebuild_instruction() {
        let typed = |bytes: &[u8]| match SemanticManifest::decode(bytes) {
            Err(CoreError::Typed { code, message }) => Some((code, message)),
            _ => None,
        };
        for foreign in [
            1_u32,
            2,
            3,
            7,
            8,
            9,
            10,
            11,
            FORMAT_VERSION.saturating_add(1),
        ] {
            // A map that only names the format: what an older or newer
            // writer's shape has in common with this one.
            let mut bytes = Vec::new();
            ciborium::into_writer(
                &ciborium::value::Value::Map(vec![
                    (
                        ciborium::value::Value::Text("format_version".to_string()),
                        ciborium::value::Value::Integer(foreign.into()),
                    ),
                    (
                        ciborium::value::Value::Text("repo_id".to_string()),
                        ciborium::value::Value::Text("repo".to_string()),
                    ),
                ]),
                &mut bytes,
            )
            .expect("encode probe");
            let (code, message) =
                typed(&bytes).unwrap_or_else(|| panic!("format {foreign} must be refused typed"));
            assert_eq!(code, FORMAT_UNSUPPORTED_CODE);
            assert!(message.contains("rebuild"), "{message}");
            assert!(
                message.contains(&format!("format version {foreign}")),
                "{message}"
            );
        }
        // The current format with a missing field is corrupt, not foreign.
        let mut bytes = Vec::new();
        ciborium::into_writer(
            &ciborium::value::Value::Map(vec![(
                ciborium::value::Value::Text("format_version".to_string()),
                ciborium::value::Value::Integer(FORMAT_VERSION.into()),
            )]),
            &mut bytes,
        )
        .expect("encode probe");
        assert!(matches!(
            SemanticManifest::decode(&bytes),
            Err(CoreError::Storage(_))
        ));
        // A full pre-uniqueness manifest must be rebuilt, even if its old
        // shape is otherwise identical to the current one.
        let mut stale = manifest(300, ann_seal(300));
        stale.format_version = 11;
        let stale_bytes = stale.encode().expect("encode old shape");
        assert!(matches!(
            SemanticManifest::decode(&stale_bytes),
            Err(CoreError::Typed { code, message })
                if code == FORMAT_UNSUPPORTED_CODE && message.contains("rebuild")
        ));
        assert!(matches!(
            validate(&stale),
            Err(CoreError::Typed { code, .. }) if code == FORMAT_UNSUPPORTED_CODE
        ));
    }

    #[test]
    fn the_vector_index_seal_refuses_every_contradiction() {
        let validate = |seal: &VectorIndexSealV1, rows: u64| seal.validate(rows, GENERATION);
        validate(&exact_seal(), 255).expect("exact below the floor");
        validate(&ann_seal(256), 256).expect("ann at the floor");
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
            (
                "dot distance",
                with_ann(&ann_seal(300), |ann| ann.distance = "dot".to_string()),
                300,
            ),
            (
                "nprobes beyond partitions",
                with_ann(&ann_seal(300), |ann| ann.nprobes = 2),
                300,
            ),
            (
                "zero refine factor",
                with_ann(&ann_seal(300), |ann| ann.refine_factor = 0),
                300,
            ),
        ];
        for (label, seal, rows) in cases {
            assert!(validate(&seal, rows).is_err(), "{label} must be refused");
        }
    }

    /// The segment record must describe every segment as the library
    /// built it.
    ///
    /// The trained segment carries the recipe, and a record that claims the
    /// recipe for an appended segment, or that names fewer segments than
    /// exist, is refused.
    #[test]
    fn the_segment_record_refuses_a_recipe_claimed_for_segments_not_built_with_it() {
        let validate = |seal: &VectorIndexSealV1| {
            let rows = seal.ann.as_ref().map_or(0, |ann| ann.indexed_rows);
            seal.validate(rows, GENERATION)
        };
        validate(&appended_seal(300, 60, 3, 3)).expect("appended segments recorded verbatim");
        let refused: Vec<(&str, VectorIndexSealV1, &str)> = vec![
            (
                "fewer records than segments",
                with_ann(&appended_seal(300, 60, 3, 3), |ann| {
                    let _dropped = ann.segments.pop();
                }),
                "segment records for 3 segments",
            ),
            (
                "no records at all",
                with_ann(&ann_seal(300), |ann| ann.segments.clear()),
                "segment records for 1 segments",
            ),
            (
                "the trained segment does not carry the recipe",
                with_ann(&ann_seal(300), |ann| {
                    ann.segments = vec![appended_segment("trained")];
                }),
                "the trained segment trained was built with m=20 ef_construction=150",
            ),
            (
                "a segment without an identity",
                with_ann(&ann_seal(300), |ann| {
                    ann.segments = vec![trained_segment("")];
                }),
                "has no identity",
            ),
            (
                "a segment recorded twice",
                with_ann(&appended_seal(300, 60, 3, 2), |ann| {
                    ann.segments = vec![trained_segment("same"), appended_segment("same")];
                }),
                "recorded twice",
            ),
            (
                "a zero graph parameter",
                with_ann(&appended_seal(300, 60, 3, 2), |ann| {
                    ann.segments = vec![
                        trained_segment("trained"),
                        AnnIndexSegmentSealV1 {
                            uuid: "appended".to_string(),
                            hnsw_m: 20,
                            hnsw_ef_construction: 0,
                        },
                    ];
                }),
                "zero graph parameter",
            ),
        ];
        for (label, seal, expected) in refused {
            let error = validate(&seal).expect_err(label);
            assert!(
                error.to_string().contains(expected),
                "{label}: expected `{expected}` in `{error}`"
            );
        }
    }

    /// The lineage record validates on its own arithmetic and against the
    /// index and generation it describes; each contradiction names its
    /// cause.
    #[test]
    fn the_index_lineage_refuses_every_contradiction() {
        let validate = |seal: &VectorIndexSealV1| {
            let rows = seal.ann.as_ref().map_or(0, |ann| ann.indexed_rows);
            seal.validate(rows, GENERATION)
        };
        // Within budget: 60 of 300 is 200 per mille, two segments.
        validate(&appended_seal(300, 60, 3, 2)).expect("an append inside the budget");
        // Exactly at the ratio and the segment cap.
        validate(&appended_seal(400, 100, 0, 9)).expect("an append at the budget");
        // A deletion-only delta appended nothing but is not a train.
        validate(&appended_seal(300, 0, 20, 1)).expect("deletions alone are inside the budget");

        let refused: Vec<(&str, VectorIndexSealV1, &str)> = vec![
            (
                "above the ratio",
                appended_seal(400, 101, 0, 2),
                "exceed the budget",
            ),
            (
                "above the absolute cap",
                with_lineage(&appended_seal(4_000_000, 300_000, 0, 2), |lineage| {
                    lineage.append_ratio_max_per_mille = 1_000;
                }),
                "exceed the budget",
            ),
            (
                "more segments than the cap admits",
                appended_seal(400, 100, 0, 10),
                "segments exceed",
            ),
            (
                "counters that do not add up to the coverage",
                with_lineage(&appended_seal(300, 60, 0, 2), |lineage| {
                    lineage.appended_rows = 59;
                }),
                "but the index covers",
            ),
            (
                "more deleted than ever indexed",
                with_lineage(&appended_seal(300, 0, 0, 1), |lineage| {
                    lineage.deleted_rows = 301;
                }),
                "row accounting overflows",
            ),
            (
                "trained after the sealing generation",
                with_lineage(&ann_seal(300), |lineage| {
                    lineage.trained_at_generation = GENERATION.saturating_add(1);
                }),
                "after the sealing generation",
            ),
            (
                "trained by this seal yet appended to",
                with_lineage(&appended_seal(300, 60, 0, 2), |lineage| {
                    lineage.trained_at_generation = GENERATION;
                }),
                "trained by this seal yet",
            ),
            (
                "trained on nothing",
                with_lineage(&ann_seal(300), |lineage| {
                    lineage.trained_rows = 0;
                }),
                "trained on zero rows",
            ),
            (
                "ratio arithmetic overflow",
                with_lineage(&appended_seal(300, 60, 0, 2), |lineage| {
                    lineage.trained_rows = u64::MAX;
                }),
                "arithmetic overflows",
            ),
        ];
        for (label, seal, expected) in refused {
            let error = validate(&seal).expect_err(label);
            assert!(
                error.to_string().contains(expected),
                "{label}: expected `{expected}` in `{error}`"
            );
        }
    }
}
