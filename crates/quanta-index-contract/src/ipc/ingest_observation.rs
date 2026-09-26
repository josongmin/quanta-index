//! Transient ingest observations. These are not durable acknowledgements.

use serde::{Deserialize, Serialize};

use crate::{BatchPublishReceipt, ManifestGeneration, RepoId, RevisionId};

/// Wall-clock nanoseconds observed by the semantic storage owner.
/// Nested delete/append timings overlap the stream/clear/tombstone passes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestStageDurations {
    /// Whole semantic build, including provider windows and durable promotion.
    pub total: u64,
    /// Validation, recovery, staging copy and opening working tables.
    pub prepare: u64,
    /// Dataset/contract promotion, manifest/marker writes and their fsyncs.
    pub promotion: u64,
    pub clear_surfaces: u64,
    pub stream: u64,
    pub semantic_delete: u64,
    pub membership_delete: u64,
    pub semantic_append: u64,
    pub membership_append: u64,
    pub tombstones: u64,
    /// Measured only when the semantic generation seals.
    #[serde(deserialize_with = "required_optional")]
    pub seal: Option<u64>,
    /// Provider calls only; excludes window planning and storage.
    #[serde(deserialize_with = "required_optional")]
    pub embedding: Option<u64>,
}

/// Actual operations performed by one semantic storage build.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestStageReport {
    pub owner_scopes: u64,
    pub windows: u64,
    pub semantic_delete_calls: u64,
    pub semantic_delete_commits: u64,
    pub membership_delete_calls: u64,
    pub membership_delete_commits: u64,
    pub semantic_append_calls: u64,
    pub membership_append_calls: u64,
    pub durations: IngestStageDurations,
}

/// Whether this request executed all storage stages, only missing tracks,
/// or returned a durable acknowledgement without executing storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestObservationStatus {
    Executed,
    PartialRecovery,
    FinalizeOnly,
    Replayed,
}

/// One call's identity-bound, non-persisted timing observation.
/// `None` means not measured/executed, never zero-filled elapsed time.
/// Failed/uncertain calls return errors, not successful observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchCorpusIngestObservation {
    pub request_id: u64,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub status: IngestObservationStatus,
    #[serde(deserialize_with = "required_optional")]
    pub semantic: Option<Box<IngestStageReport>>,
    #[serde(deserialize_with = "required_optional")]
    pub lexical_build_ns: Option<u64>,
    #[serde(deserialize_with = "required_optional")]
    pub finalize_ns: Option<u64>,
    /// Activation belongs to a separate control request, not this ingest.
    #[serde(deserialize_with = "required_optional")]
    pub activation_ns: Option<u64>,
}

/// The current publish response: durable identity and separate transient
/// observation. The optional field is explicitly required on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchCorpusPublishOutcome {
    pub receipt: BatchPublishReceipt,
    #[serde(deserialize_with = "required_observation")]
    pub observation: Option<SearchCorpusIngestObservation>,
}

fn required_observation<'de, D>(deserializer: D) -> Result<Option<SearchCorpusIngestObservation>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::deserialize(deserializer)
}

fn required_optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

impl SearchCorpusIngestObservation {
    /// Reject observations from other calls and invalid stage/status claims.
    pub fn validate_for(
        &self,
        request_id: u64,
        batch: &crate::SearchCorpusIngestBatch,
        receipt: &BatchPublishReceipt,
    ) -> Result<(), String> {
        if self.request_id != request_id
            || self.repo_id != batch.repo_id
            || self.revision_id != batch.revision_id
            || self.generation != batch.generation
            || self.batch_digest != batch.batch_digest
            || self.generation != receipt.generation
            || self.batch_digest != receipt.batch_digest
        {
            return Err("ingest observation identity does not match request and receipt".to_string());
        }
        if self.activation_ns.is_some() {
            return Err("ingest observation cannot measure a separate activation request".to_string());
        }
        let semantic = self.semantic.is_some();
        let lexical = self.lexical_build_ns.is_some();
        let finalize = self.finalize_ns.is_some();
        let valid = match self.status {
            IngestObservationStatus::Executed => receipt.applied && semantic && lexical && finalize,
            IngestObservationStatus::PartialRecovery => receipt.applied && semantic != lexical && finalize,
            IngestObservationStatus::FinalizeOnly => receipt.applied && !semantic && !lexical && finalize,
            IngestObservationStatus::Replayed => !receipt.applied && !semantic && !lexical && !finalize,
        };
        if !valid {
            return Err("ingest observation stages contradict execution status".to_string());
        }
        if let Some(report) = &self.semantic {
            if report.durations.seal.is_some() != batch.seal {
                return Err("ingest observation seal availability contradicts request".to_string());
            }
            let durations = &report.durations;
            let passes = [durations.prepare, durations.clear_surfaces, durations.stream, durations.tombstones, durations.seal.unwrap_or(0), durations.promotion];
            let pass_total = passes.into_iter().try_fold(0_u64, u64::checked_add)
                .ok_or_else(|| "ingest observation duration sum overflow".to_string())?;
            if pass_total > durations.total
                || durations.embedding.is_some_and(|embedding| embedding > durations.stream)
            {
                return Err("ingest observation nested durations exceed containing build".to_string());
            }
            let storage_total = [durations.semantic_delete, durations.membership_delete, durations.semantic_append, durations.membership_append]
                .into_iter().try_fold(0_u64, u64::checked_add)
                .ok_or_else(|| "ingest observation storage duration sum overflow".to_string())?;
            let mutation_total = [durations.clear_surfaces, durations.stream, durations.tombstones]
                .into_iter().try_fold(0_u64, u64::checked_add)
                .ok_or_else(|| "ingest observation mutation duration sum overflow".to_string())?;
            if storage_total > mutation_total {
                return Err("ingest observation storage durations exceed containing passes".to_string());
            }
        }
        Ok(())
    }
}

impl std::ops::Deref for SearchCorpusPublishOutcome {
    type Target = BatchPublishReceipt;

    fn deref(&self) -> &Self::Target {
        &self.receipt
    }
}

impl From<BatchPublishReceipt> for SearchCorpusPublishOutcome {
    fn from(receipt: BatchPublishReceipt) -> Self {
        Self { receipt, observation: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn batch() -> Result<crate::SearchCorpusIngestBatch, Box<dyn std::error::Error>> {
        Ok(crate::SearchCorpusIngestBatch {
            repo_id: RepoId::new("observation-repo")?,
            revision_id: RevisionId::new("observation-revision")?,
            generation: ManifestGeneration::new(4),
            base_generation: None,
            manifest_digest: "manifest:observed".to_string(),
            batch_digest: "a".repeat(64),
            mode: crate::BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: false,
        })
    }

    fn outcome() -> Result<SearchCorpusPublishOutcome, Box<dyn std::error::Error>> {
        let batch = batch()?;
        Ok(SearchCorpusPublishOutcome {
            receipt: BatchPublishReceipt::empty_for(batch.generation, Some(batch.manifest_digest), batch.batch_digest.clone()),
            observation: Some(SearchCorpusIngestObservation {
                request_id: 11,
                repo_id: batch.repo_id,
                revision_id: batch.revision_id,
                generation: batch.generation,
                batch_digest: batch.batch_digest,
                status: IngestObservationStatus::Executed,
                semantic: Some(Box::new(IngestStageReport::default())),
                lexical_build_ns: Some(3),
                finalize_ns: Some(7),
                activation_ns: None,
            }),
        })
    }

    #[test]
    fn ingest_observation_requires_nullable_fields_and_rejects_unknown_fields() -> TestResult {
        let outcome = outcome()?;
        let wire = serde_json::to_value(&outcome)?;
        assert_eq!(serde_json::from_value::<SearchCorpusPublishOutcome>(wire.clone())?, outcome);
        for pointer in ["/observation", "/observation/semantic", "/observation/lexical_build_ns", "/observation/finalize_ns", "/observation/activation_ns", "/observation/semantic/durations/seal", "/observation/semantic/durations/embedding"] {
            let (parent, key) = pointer.rsplit_once('/').ok_or("invalid pointer")?;
            let mut missing = wire.clone();
            let _removed = missing.pointer_mut(parent).and_then(serde_json::Value::as_object_mut).ok_or("missing object")?.remove(key);
            assert!(serde_json::from_value::<SearchCorpusPublishOutcome>(missing).is_err(), "accepted missing {pointer}");
        }
        let mut unknown = wire;
        let _previous = unknown.as_object_mut().ok_or("missing outcome")?.insert("unbound_timing".to_string(), serde_json::Value::from(1));
        assert!(serde_json::from_value::<SearchCorpusPublishOutcome>(unknown).is_err());
        let old_payload = serde_json::json!({"SearchCorpusReceipt": outcome.receipt});
        assert!(serde_json::from_value::<crate::SearchPlaneIngestIpcResponse>(old_payload).is_err(), "old bare receipt must not silently mix with observed payload");
        Ok(())
    }

    #[test]
    fn ingest_observation_replay_has_no_cached_or_zero_filled_measurements() -> TestResult {
        let batch = batch()?;
        let mut outcome = outcome()?;
        let mut observation = outcome.observation.take().ok_or("missing observation")?;
        observation.validate_for(11, &batch, &outcome.receipt)?;
        outcome.receipt = outcome.receipt.replayed();
        observation.status = IngestObservationStatus::Replayed;
        assert!(observation.validate_for(11, &batch, &outcome.receipt).is_err());
        observation.semantic = None;
        observation.lexical_build_ns = None;
        observation.finalize_ns = None;
        observation.validate_for(11, &batch, &outcome.receipt)?;
        observation.lexical_build_ns = Some(0);
        assert!(observation.validate_for(11, &batch, &outcome.receipt).is_err());
        Ok(())
    }

    #[test]
    fn ingest_observation_rejects_partial_stages_and_impossible_timing() -> TestResult {
        let batch = batch()?;
        let mut outcome = outcome()?;
        let observation = outcome.observation.as_mut().ok_or("missing observation")?;
        observation.semantic.as_mut().ok_or("missing semantic")?.durations.prepare = 1;
        assert!(observation.validate_for(11, &batch, &outcome.receipt).is_err());
        observation.semantic.as_mut().ok_or("missing semantic")?.durations.prepare = 0;
        observation.status = IngestObservationStatus::PartialRecovery;
        assert!(observation.validate_for(11, &batch, &outcome.receipt).is_err());
        observation.semantic = None;
        observation.validate_for(11, &batch, &outcome.receipt)?;
        observation.status = IngestObservationStatus::FinalizeOnly;
        assert!(observation.validate_for(11, &batch, &outcome.receipt).is_err());
        observation.lexical_build_ns = None;
        observation.validate_for(11, &batch, &outcome.receipt)?;
        Ok(())
    }
}
