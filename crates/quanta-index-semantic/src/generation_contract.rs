//! Durable pre-seal semantic batch contract and delta-base provenance.
//!
//! Written on the first accepted batch for a target generation and validated on
//! every subsequent batch plus at sealed open. This keeps `mode`,
//! `base_generation`, and the model contract authoritative even before the final
//! sealed manifest exists. Exactly one format is served; a contract written
//! under any other is refused typed with the instruction to rebuild.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use quanta_index_contract::{
    BatchIngestMode, EmbeddingDistanceMetric, EmbeddingNormalization, ManifestGeneration,
};
use quanta_index_core::{CoreError, SemanticGenerationContractV1};

use crate::codec::{self, cbor_serde, decode_current_format, format_unsupported};

pub(crate) const GENERATION_CONTRACT_VERSION: u32 = 2;

/// Return `Err($variant(format!(...)))` when two contract fields disagree.
///
/// Both validation paths (`GenerationContract::merge_batch`,
/// `SemanticManifest::validate_against`) are a run of field-equality guards
/// that differ only in the `CoreError` variant and the message wording —
/// this keeps each guard a single, uniform line.
macro_rules! ensure_field_eq {
    ($variant:expr, $lhs:expr, $rhs:expr, $fmt:literal) => {
        if $lhs != $rhs {
            return Err($variant(format!($fmt, $lhs, $rhs)));
        }
    };
}

pub(crate) use ensure_field_eq;

#[derive(Clone, Debug, Eq, PartialEq)]
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
    pub(crate) fn from_batch(batch: &SemanticGenerationContractV1) -> Self {
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

    /// Decode the current format only; any other format is refused typed.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        decode_current_format(bytes, "semantic generation contract", GENERATION_CONTRACT_VERSION)
    }

    pub(crate) fn validate_batch_shape(
        batch: &SemanticGenerationContractV1,
    ) -> Result<(), CoreError> {
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

    pub(crate) fn merge_batch(
        &self,
        batch: &SemanticGenerationContractV1,
    ) -> Result<Self, CoreError> {
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
        let corpus_policy_digest = match (
            self.corpus_policy_digest.as_ref(),
            observed.corpus_policy_digest.as_ref(),
        ) {
            (Some(existing), Some(incoming)) if existing == incoming => {
                self.corpus_policy_digest.clone()
            }
            (None, None) => None,
            _ => {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: existing generation corpus_policy_digest {:?} does not match batch corpus_policy_digest {:?}",
                    self.corpus_policy_digest, observed.corpus_policy_digest
                )));
            }
        };
        let mut required_corpora = self.required_corpora.clone();
        required_corpora.extend(observed.required_corpora);
        required_corpora.sort();
        required_corpora.dedup();

        Ok(Self {
            format_version: GENERATION_CONTRACT_VERSION,
            mode: self.mode,
            base_generation: self.base_generation,
            model_id: self.model_id.clone(),
            model_version: self.model_version.clone(),
            dimension: self.dimension,
            distance_metric: self.distance_metric.clone(),
            normalization: self.normalization.clone(),
            required_corpora,
            corpus_policy_digest,
        })
    }

    /// Refuse a contract written under any format but the served one.
    pub(crate) fn validate_format(&self) -> Result<(), CoreError> {
        if self.format_version != GENERATION_CONTRACT_VERSION {
            return Err(format_unsupported(
                "semantic generation contract",
                self.format_version,
                GENERATION_CONTRACT_VERSION,
            ));
        }
        Ok(())
    }
}

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

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning tests assert with `assert!` on fixture invariants; a violated fixture invariant is not a propagatable error"
)]
mod tests {
    use quanta_index_contract::BatchIngestMode;
    use quanta_index_core::CoreError;

    use super::{GENERATION_CONTRACT_VERSION, GenerationContract};
    use crate::codec::FORMAT_UNSUPPORTED_CODE;

    /// The shape build contract format 1 wrote: no corpus fields. It is
    /// refused typed, never decoded with fabricated corpus policy.
    struct BuildContractFormat1 {
        format_version: u32,
        mode: BatchIngestMode,
        base_generation: Option<quanta_index_contract::ManifestGeneration>,
        model_id: String,
        model_version: Option<String>,
        dimension: u32,
        distance_metric: String,
        normalization: String,
    }

    crate::codec::cbor_serde!(BuildContractFormat1 {
        format_version: u32,
        mode: BatchIngestMode,
        base_generation: Option<quanta_index_contract::ManifestGeneration>,
        model_id: String,
        model_version: Option<String>,
        dimension: u32,
        distance_metric: String,
        normalization: String,
    });

    #[test]
    fn a_format_1_contract_is_refused_typed_with_a_rebuild_instruction()
    -> Result<(), Box<dyn std::error::Error>> {
        let legacy = BuildContractFormat1 {
            format_version: 1,
            mode: BatchIngestMode::ReplaceGeneration,
            base_generation: None,
            model_id: "legacy-model".to_string(),
            model_version: Some("v3".to_string()),
            dimension: 3,
            distance_metric: "cosine".to_string(),
            normalization: "l2_unit".to_string(),
        };
        let bytes = crate::codec::encode(&legacy, "format-1 generation contract fixture")?;
        match GenerationContract::decode(&bytes) {
            Err(CoreError::Typed { code, message }) => {
                assert_eq!(code, FORMAT_UNSUPPORTED_CODE);
                assert!(message.contains("format version 1"), "{message}");
                assert!(
                    message.contains(&format!("format {GENERATION_CONTRACT_VERSION} only")),
                    "{message}"
                );
                assert!(message.contains("rebuild"), "{message}");
            }
            other => panic!("a format-1 contract must be refused typed, got {other:?}"),
        }
        Ok(())
    }
}
