//! Durable pre-seal semantic batch contract and delta-base provenance.
//!
//! Written on the first accepted batch for a target generation and validated on
//! every subsequent batch plus at sealed open. This keeps `mode`,
//! `base_generation`, and the model contract authoritative even before the final
//! sealed manifest exists.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use quanta_index_contract::{BatchIngestMode, ManifestGeneration, SemanticIngestBatch};
use quanta_index_core::CoreError;

use crate::codec::{self, cbor_serde};
use crate::manifest::{SemanticManifest, distance_metric_token, normalization_token};

const GENERATION_CONTRACT_VERSION: u32 = 1;

/// Return `Err($variant(format!(...)))` when two contract fields disagree.
///
/// Both validation paths (`validate_batch`, `validate_manifest`) are a run of
/// field-equality guards that differ only in the `CoreError` variant and the
/// message wording — this keeps each guard a single, uniform line.
macro_rules! ensure_field_eq {
    ($variant:expr, $lhs:expr, $rhs:expr, $fmt:literal) => {
        if $lhs != $rhs {
            return Err($variant(format!($fmt, $lhs, $rhs)));
        }
    };
}

pub(crate) struct GenerationContract {
    pub(crate) format_version: u32,
    pub(crate) mode: BatchIngestMode,
    pub(crate) base_generation: Option<ManifestGeneration>,
    pub(crate) model_id: String,
    pub(crate) model_version: Option<String>,
    pub(crate) dimension: u32,
    pub(crate) distance_metric: String,
    pub(crate) normalization: String,
    pub(crate) required_corpora: Vec<String>,
    pub(crate) corpus_policy_digest: Option<String>,
}

cbor_serde!(GenerationContract {
    format_version: u32,
    mode: BatchIngestMode,
    base_generation: Option<ManifestGeneration>,
    model_id: String,
    model_version: Option<String>,
    dimension: u32,
    distance_metric: String,
    normalization: String,
    required_corpora: Vec<String>,
    corpus_policy_digest: Option<String>,
});

impl GenerationContract {
    pub(crate) fn from_batch(batch: &SemanticIngestBatch) -> Self {
        Self {
            format_version: GENERATION_CONTRACT_VERSION,
            mode: batch.mode,
            base_generation: batch.base_generation,
            model_id: batch.model_contract.model_id.to_string(),
            model_version: batch
                .model_contract
                .model_version
                .as_deref()
                .map(str::to_owned),
            dimension: batch.model_contract.dimension,
            distance_metric: distance_metric_token(batch.model_contract.distance_metric).to_owned(),
            normalization: normalization_token(batch.model_contract.normalization).to_owned(),
            required_corpora: batch
                .required_corpora
                .iter()
                .map(|kind| kind.as_code_str().to_string())
                .collect(),
            corpus_policy_digest: batch.corpus_policy_digest.clone(),
        }
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        codec::encode(self, "semantic generation contract")
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        codec::decode(bytes, "semantic generation contract")
    }

    pub(crate) fn validate_batch_shape(batch: &SemanticIngestBatch) -> Result<(), CoreError> {
        match (batch.mode, batch.base_generation) {
            (BatchIngestMode::ReplaceGeneration, Some(_)) => Err(CoreError::InvalidContract(
                "semantic: ReplaceGeneration batches must not carry base_generation".to_string(),
            )),
            (BatchIngestMode::Delta, None) => Err(CoreError::InvalidContract(
                "semantic: Delta batches must carry base_generation".to_string(),
            )),
            (BatchIngestMode::ReplaceGeneration, None) | (BatchIngestMode::Delta, Some(_)) => {
                Ok(())
            }
        }
    }

    pub(crate) fn validate_batch(&self, batch: &SemanticIngestBatch) -> Result<(), CoreError> {
        Self::validate_batch_shape(batch)?;
        self.validate_format()?;
        let observed = Self::from_batch(batch);
        ensure_field_eq!(
            CoreError::InvalidContract,
            self.mode,
            observed.mode,
            "semantic: existing generation mode {:?} does not match batch mode {:?}"
        );
        ensure_field_eq!(
            CoreError::InvalidContract,
            self.base_generation,
            observed.base_generation,
            "semantic: existing generation base_generation {:?} does not match batch base_generation {:?}"
        );
        ensure_field_eq!(
            CoreError::InvalidContract,
            self.model_id,
            observed.model_id,
            "semantic: existing generation model_id `{}` does not match batch model_id `{}`"
        );
        ensure_field_eq!(
            CoreError::InvalidContract,
            self.model_version,
            observed.model_version,
            "semantic: existing generation model_version {:?} does not match batch model_version {:?}"
        );
        ensure_field_eq!(
            CoreError::InvalidContract,
            self.dimension,
            observed.dimension,
            "semantic: existing generation dimension {} does not match batch dimension {}"
        );
        ensure_field_eq!(
            CoreError::InvalidContract,
            self.distance_metric,
            observed.distance_metric,
            "semantic: existing generation distance_metric `{}` does not match batch distance_metric `{}`"
        );
        ensure_field_eq!(
            CoreError::InvalidContract,
            self.normalization,
            observed.normalization,
            "semantic: existing generation normalization `{}` does not match batch normalization `{}`"
        );
        ensure_field_eq!(
            CoreError::InvalidContract,
            self.required_corpora,
            observed.required_corpora,
            "semantic: existing generation required_corpora {:?} does not match batch required_corpora {:?}"
        );
        ensure_field_eq!(
            CoreError::InvalidContract,
            self.corpus_policy_digest,
            observed.corpus_policy_digest,
            "semantic: existing generation corpus_policy_digest {:?} does not match batch corpus_policy_digest {:?}"
        );
        Ok(())
    }

    pub(crate) fn validate_manifest(&self, manifest: &SemanticManifest) -> Result<(), CoreError> {
        self.validate_format()?;
        ensure_field_eq!(
            CoreError::Storage,
            manifest.model_id,
            self.model_id,
            "semantic: manifest model_id `{}` does not match sealed build contract `{}`"
        );
        ensure_field_eq!(
            CoreError::Storage,
            manifest.model_version,
            self.model_version,
            "semantic: manifest model_version {:?} does not match sealed build contract {:?}"
        );
        ensure_field_eq!(
            CoreError::Storage,
            manifest.dimension,
            self.dimension,
            "semantic: manifest dimension {} does not match sealed build contract {}"
        );
        ensure_field_eq!(
            CoreError::Storage,
            manifest.distance_metric,
            self.distance_metric,
            "semantic: manifest distance_metric `{}` does not match sealed build contract `{}`"
        );
        ensure_field_eq!(
            CoreError::Storage,
            manifest.normalization,
            self.normalization,
            "semantic: manifest normalization `{}` does not match sealed build contract `{}`"
        );
        ensure_field_eq!(
            CoreError::Storage,
            manifest.required_corpora,
            self.required_corpora,
            "semantic: manifest required_corpora {:?} does not match sealed build contract {:?}"
        );
        ensure_field_eq!(
            CoreError::Storage,
            manifest.corpus_policy_digest,
            self.corpus_policy_digest,
            "semantic: manifest corpus_policy_digest {:?} does not match sealed build contract {:?}"
        );
        Ok(())
    }

    fn validate_format(&self) -> Result<(), CoreError> {
        if self.format_version != GENERATION_CONTRACT_VERSION {
            return Err(CoreError::Storage(format!(
                "semantic: generation contract format version {} unsupported (expected {GENERATION_CONTRACT_VERSION})",
                self.format_version
            )));
        }
        Ok(())
    }
}
