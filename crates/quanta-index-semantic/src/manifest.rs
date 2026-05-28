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
    EmbeddingDistanceMetric, EmbeddingModelContract, EmbeddingNormalization, ManifestGeneration,
    RepoId, RevisionId,
};
use quanta_index_core::CoreError;

use crate::codec::{self, cbor_serde};

/// Manifest + dataset shard format version. Bumped on any durable shape change.
pub(crate) const FORMAT_VERSION: u32 = 1;

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
    pub(crate) content_checksum: String,
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
    content_checksum: String,
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
    #[expect(
        clippy::too_many_arguments,
        reason = "manifest aggregates the full LDB-01 §3 field set from distinct build inputs; \
                  bundling them into a transient struct would only move the surface, not reduce it"
    )]
    pub(crate) fn from_build(
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        model_contract: &EmbeddingModelContract,
        manifest_digest: &str,
        dimension: u32,
        row_count: u64,
        built_at_unix_nanos: u64,
        content_checksum: String,
    ) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            repo_id: repo.as_str().to_owned(),
            revision_id: revision.as_str().to_owned(),
            generation: generation.get(),
            manifest_digest: manifest_digest.to_owned(),
            model_id: model_contract.model_id.to_string(),
            model_version: model_contract.model_version.as_deref().map(str::to_owned),
            dimension,
            distance_metric: distance_metric_token(model_contract.distance_metric).to_owned(),
            normalization: normalization_token(model_contract.normalization).to_owned(),
            row_count,
            built_at_unix_nanos,
            content_checksum,
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
        if self.format_version != FORMAT_VERSION {
            return Err(CoreError::Storage(format!(
                "semantic: manifest format version {} unsupported (expected {FORMAT_VERSION})",
                self.format_version
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
        Ok(())
    }
}
