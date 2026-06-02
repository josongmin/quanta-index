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

pub(crate) struct GenerationContract {
    pub(crate) format_version: u32,
    pub(crate) mode: BatchIngestMode,
    pub(crate) base_generation: Option<ManifestGeneration>,
    pub(crate) model_id: String,
    pub(crate) model_version: Option<String>,
    pub(crate) dimension: u32,
    pub(crate) distance_metric: String,
    pub(crate) normalization: String,
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
        if self.mode != observed.mode {
            return Err(CoreError::InvalidContract(format!(
                "semantic: existing generation mode {:?} does not match batch mode {:?}",
                self.mode, observed.mode
            )));
        }
        if self.base_generation != observed.base_generation {
            return Err(CoreError::InvalidContract(format!(
                "semantic: existing generation base_generation {:?} does not match batch base_generation {:?}",
                self.base_generation, observed.base_generation
            )));
        }
        if self.model_id != observed.model_id {
            return Err(CoreError::InvalidContract(format!(
                "semantic: existing generation model_id `{}` does not match batch model_id `{}`",
                self.model_id, observed.model_id
            )));
        }
        if self.model_version != observed.model_version {
            return Err(CoreError::InvalidContract(format!(
                "semantic: existing generation model_version {:?} does not match batch model_version {:?}",
                self.model_version, observed.model_version
            )));
        }
        if self.dimension != observed.dimension {
            return Err(CoreError::InvalidContract(format!(
                "semantic: existing generation dimension {} does not match batch dimension {}",
                self.dimension, observed.dimension
            )));
        }
        if self.distance_metric != observed.distance_metric {
            return Err(CoreError::InvalidContract(format!(
                "semantic: existing generation distance_metric `{}` does not match batch distance_metric `{}`",
                self.distance_metric, observed.distance_metric
            )));
        }
        if self.normalization != observed.normalization {
            return Err(CoreError::InvalidContract(format!(
                "semantic: existing generation normalization `{}` does not match batch normalization `{}`",
                self.normalization, observed.normalization
            )));
        }
        Ok(())
    }

    pub(crate) fn validate_manifest(&self, manifest: &SemanticManifest) -> Result<(), CoreError> {
        self.validate_format()?;
        if manifest.model_id != self.model_id {
            return Err(CoreError::Storage(format!(
                "semantic: manifest model_id `{}` does not match sealed build contract `{}`",
                manifest.model_id, self.model_id
            )));
        }
        if manifest.model_version != self.model_version {
            return Err(CoreError::Storage(format!(
                "semantic: manifest model_version {:?} does not match sealed build contract {:?}",
                manifest.model_version, self.model_version
            )));
        }
        if manifest.dimension != self.dimension {
            return Err(CoreError::Storage(format!(
                "semantic: manifest dimension {} does not match sealed build contract {}",
                manifest.dimension, self.dimension
            )));
        }
        if manifest.distance_metric != self.distance_metric {
            return Err(CoreError::Storage(format!(
                "semantic: manifest distance_metric `{}` does not match sealed build contract `{}`",
                manifest.distance_metric, self.distance_metric
            )));
        }
        if manifest.normalization != self.normalization {
            return Err(CoreError::Storage(format!(
                "semantic: manifest normalization `{}` does not match sealed build contract `{}`",
                manifest.normalization, self.normalization
            )));
        }
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
