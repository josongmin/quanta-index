//! Persisted semantic generation manifest (LDB-01 manifest contract).
//!
//! Internal to `quanta-index`, but correctness-critical: the open path fails
//! closed on a format, scope, or integrity mismatch rather than guessing around
//! missing or inconsistent state.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use quanta_index_contract::{
    EmbeddingDistanceMetric, EmbeddingNormalization, ManifestGeneration, RepoId, RevisionId,
};

/// Distance metric the lancedb adapter actually serves at query time.
///
/// The adapter pins `DistanceType::Cosine` everywhere; a manifest that records
/// a different metric is rejected at open so a future producer that switches
/// metric mid-flight cannot serve cosine-quantized data through a `dot` /
/// `euclidean` contract.
const SUPPORTED_DISTANCE_METRIC: &str = "cosine";
use quanta_index_core::CoreError;

use crate::codec::{self, cbor_serde};
use crate::generation_contract::GenerationContract;

/// Current manifest format version. Bumped on any durable shape change.
///
/// `3` = lancedb-backed dataset plus `semantic-build-contract.cbor`, which
/// keeps the pre-seal batch contract authoritative and lets open validate the
/// sealed manifest against the accepted build contract.
pub(crate) const FORMAT_VERSION: u32 = 3;

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
    pub(crate) built_at_unix_nanos: u64,
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
        built_at_unix_nanos: u64,
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
            built_at_unix_nanos,
        }
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        codec::encode(self, "semantic manifest")
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        codec::decode(bytes, "semantic manifest")
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
            && self.format_version != LEGACY_LANCEDB_FORMAT_VERSION
        {
            return Err(CoreError::Storage(format!(
                "semantic: manifest format version {} unsupported (expected {FORMAT_VERSION} or legacy {LEGACY_LANCEDB_FORMAT_VERSION})",
                self.format_version,
            )));
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
        Ok(())
    }
}
