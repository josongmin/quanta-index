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
    EmbeddingDistanceMetric, EmbeddingNormalization, ManifestGeneration, RepoId, RevisionId,
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
/// `7` = v6 membership commitment plus the canonical semantic-row root.
pub(crate) const FORMAT_VERSION: u32 = 7;
pub(crate) const LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION: u32 = 6;

/// Legacy structured-membership manifest without a sealed sidecar commitment.
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

impl SemanticManifest {
    pub(crate) fn from_generation_contract(
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        generation_contract: &GenerationContract,
        manifest_digest: &str,
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
    ) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            repo_id: repo.as_str().to_owned(),
            revision_id: revision.as_str().to_owned(),
            generation: generation.get(),
            manifest_digest: manifest_digest.to_owned(),
            model_id: generation_contract.model_id.clone(),
            model_version: generation_contract.model_version.clone(),
            dimension: generation_contract.dimension,
            distance_metric: generation_contract.distance_metric.clone(),
            normalization: generation_contract.normalization.clone(),
            row_count,
            semantic_row_root_digest,
            built_at_unix_nanos,
            present_corpora,
            required_corpora,
            card_schema_versions,
            render_policy_digests,
            corpus_policy_digest,
            cluster_membership_root_digest,
            cluster_membership_cluster_count,
            cluster_membership_member_row_count,
        }
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        codec::encode(self, "semantic manifest")
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        match codec::decode(bytes, "semantic manifest") {
            Ok(current) => Ok(current),
            Err(current_err) => {
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
                    });
                }
                Err(current_err)
            }
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
        if self.format_version != FORMAT_VERSION
            && self.format_version != LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION
            && self.format_version != LEGACY_UNCOMMITTED_MEMBERSHIP_FORMAT_VERSION
            && self.format_version != LEGACY_SEMANTIC_CORPUS_FORMAT_VERSION
            && self.format_version != LEGACY_BUILD_CONTRACT_FORMAT_VERSION
            && self.format_version != LEGACY_LANCEDB_FORMAT_VERSION
        {
            return Err(CoreError::Storage(format!(
                "semantic: manifest format version {} unsupported (expected {FORMAT_VERSION}, legacy {LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION}, {LEGACY_UNCOMMITTED_MEMBERSHIP_FORMAT_VERSION}, {LEGACY_SEMANTIC_CORPUS_FORMAT_VERSION}, {LEGACY_BUILD_CONTRACT_FORMAT_VERSION}, or {LEGACY_LANCEDB_FORMAT_VERSION})",
                self.format_version,
            )));
        }
        if self.format_version == FORMAT_VERSION
            && !is_canonical_sha256_v1(&self.semantic_row_root_digest)
        {
            return Err(CoreError::Storage(
                "semantic: current manifest has an invalid semantic row root digest".to_string(),
            ));
        }
        if matches!(
            self.format_version,
            FORMAT_VERSION | LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION
        ) && !is_canonical_sha256_v1(&self.cluster_membership_root_digest)
        {
            return Err(CoreError::Storage(
                "semantic: current manifest has an invalid cluster membership root digest"
                    .to_string(),
            ));
        }
        if matches!(
            self.format_version,
            FORMAT_VERSION | LEGACY_UNCOMMITTED_SEMANTIC_ROW_ROOT_FORMAT_VERSION
        ) && ((self.cluster_membership_cluster_count == 0)
            != (self.cluster_membership_member_row_count == 0)
            || self.cluster_membership_cluster_count > self.cluster_membership_member_row_count)
        {
            return Err(CoreError::Storage(
                "semantic: current manifest has inconsistent cluster membership counts".to_string(),
            ));
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
}
