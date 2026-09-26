//! Transient ingest observations. These are not durable acknowledgements.

use serde::{Deserialize, Serialize};

use crate::{BatchPublishReceipt, ManifestGeneration, RepoId, RevisionId};

/// Wall-clock nanoseconds observed by the semantic storage owner.
/// Nested delete/append timings overlap the stream/clear/tombstone passes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    pub seal: Option<u64>,
    /// Provider calls only; excludes window planning and storage.
    pub embedding: Option<u64>,
}

/// Actual operations performed by one semantic storage build.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestObservationStatus {
    Executed,
    PartialRecovery,
    FinalizeOnly,
    Replayed,
}

/// One call's identity-bound, non-persisted timing observation.
/// `None` means not measured/executed, never zero-filled elapsed time.
/// Failed/uncertain calls return errors, not successful observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCorpusIngestObservation {
    pub request_id: u64,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub status: IngestObservationStatus,
    pub semantic: Option<Box<IngestStageReport>>,
    pub lexical_build_ns: Option<u64>,
    pub finalize_ns: Option<u64>,
    /// Activation belongs to a separate control request, not this ingest.
    pub activation_ns: Option<u64>,
}

/// The current publish response: durable identity and separate transient
/// observation. The optional field is explicitly required on the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchCorpusPublishOutcome {
    pub receipt: BatchPublishReceipt,
    pub observation: Option<SearchCorpusIngestObservation>,
}

// One manual wire implementation for these closed records. Each map slot is
// Option<FieldType>, so an explicit nullable field is Some(None), while an
// omitted field remains None and is refused. No unknown or duplicate key is
// discarded. Typed next_value/next_element retains each field's strict codec.
macro_rules! impl_observation_struct_serde {
    ($name:ident { $($field:ident: $ty:ty => $wire:literal),+ $(,)? }) => {
        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                const FIELDS: &[&str] = &[$($wire),+];
                let mut state = serializer.serialize_struct(stringify!($name), FIELDS.len())?;
                $(serde::ser::SerializeStruct::serialize_field(&mut state, $wire, &self.$field)?;)+
                serde::ser::SerializeStruct::end(state)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                const FIELDS: &[&str] = &[$($wire),+];
                struct WireVisitor;
                impl<'de> serde::de::Visitor<'de> for WireVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(concat!("a complete ", stringify!($name), " record"))
                    }

                    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
                    where
                        A: serde::de::MapAccess<'de>,
                    {
                        $(let mut $field: Option<$ty> = None;)+
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $($wire => {
                                    if $field.is_some() {
                                        return Err(serde::de::Error::duplicate_field($wire));
                                    }
                                    $field = Some(map.next_value::<$ty>()?);
                                },)+
                                _ => return Err(serde::de::Error::unknown_field(&key, FIELDS)),
                            }
                        }
                        Ok($name {
                            $($field: $field.ok_or_else(|| serde::de::Error::missing_field($wire))?,)+
                        })
                    }

                    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
                    where
                        A: serde::de::SeqAccess<'de>,
                    {
                        $(let $field = seq.next_element::<$ty>()?
                            .ok_or_else(|| serde::de::Error::missing_field($wire))?;)+
                        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                            return Err(serde::de::Error::invalid_length(FIELDS.len().saturating_add(1), &self));
                        }
                        Ok($name { $($field,)+ })
                    }
                }
                deserializer.deserialize_struct(stringify!($name), FIELDS, WireVisitor)
            }
        }
    };
}

impl_observation_struct_serde!(IngestStageDurations {
    total: u64 => "total",
    prepare: u64 => "prepare",
    promotion: u64 => "promotion",
    clear_surfaces: u64 => "clear_surfaces",
    stream: u64 => "stream",
    semantic_delete: u64 => "semantic_delete",
    membership_delete: u64 => "membership_delete",
    semantic_append: u64 => "semantic_append",
    membership_append: u64 => "membership_append",
    tombstones: u64 => "tombstones",
    seal: Option<u64> => "seal",
    embedding: Option<u64> => "embedding",
});

impl_observation_struct_serde!(IngestStageReport {
    owner_scopes: u64 => "owner_scopes",
    windows: u64 => "windows",
    semantic_delete_calls: u64 => "semantic_delete_calls",
    semantic_delete_commits: u64 => "semantic_delete_commits",
    membership_delete_calls: u64 => "membership_delete_calls",
    membership_delete_commits: u64 => "membership_delete_commits",
    semantic_append_calls: u64 => "semantic_append_calls",
    membership_append_calls: u64 => "membership_append_calls",
    durations: IngestStageDurations => "durations",
});

impl_observation_struct_serde!(SearchCorpusIngestObservation {
    request_id: u64 => "request_id",
    repo_id: RepoId => "repo_id",
    revision_id: RevisionId => "revision_id",
    generation: ManifestGeneration => "generation",
    batch_digest: String => "batch_digest",
    status: IngestObservationStatus => "status",
    semantic: Option<Box<IngestStageReport>> => "semantic",
    lexical_build_ns: Option<u64> => "lexical_build_ns",
    finalize_ns: Option<u64> => "finalize_ns",
    activation_ns: Option<u64> => "activation_ns",
});

impl_observation_struct_serde!(SearchCorpusPublishOutcome {
    receipt: BatchPublishReceipt => "receipt",
    observation: Option<SearchCorpusIngestObservation> => "observation",
});

impl Serialize for IngestObservationStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let (index, tag) = match self {
            Self::Executed => (0, "executed"),
            Self::PartialRecovery => (1, "partial_recovery"),
            Self::FinalizeOnly => (2, "finalize_only"),
            Self::Replayed => (3, "replayed"),
        };
        serializer.serialize_unit_variant("IngestObservationStatus", index, tag)
    }
}

impl<'de> Deserialize<'de> for IngestObservationStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const VARIANTS: &[&str] = &["executed", "partial_recovery", "finalize_only", "replayed"];
        struct StatusVisitor;
        impl<'de> serde::de::Visitor<'de> for StatusVisitor {
            type Value = IngestObservationStatus;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a closed ingest execution status")
            }

            fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::EnumAccess<'de>,
            {
                let (tag, variant) = data.variant::<String>()?;
                serde::de::VariantAccess::unit_variant(variant)?;
                match tag.as_str() {
                    "executed" => Ok(IngestObservationStatus::Executed),
                    "partial_recovery" => Ok(IngestObservationStatus::PartialRecovery),
                    "finalize_only" => Ok(IngestObservationStatus::FinalizeOnly),
                    "replayed" => Ok(IngestObservationStatus::Replayed),
                    _ => Err(serde::de::Error::unknown_variant(&tag, VARIANTS)),
                }
            }
        }
        deserializer.deserialize_enum("IngestObservationStatus", VARIANTS, StatusVisitor)
    }
}

impl SearchCorpusIngestObservation {
    /// Validate a compact request binding without retaining/copying corpus
    /// source text or vectors in the caller's response binding.
    pub fn validate_identity(
        &self,
        request_id: u64,
        pin: &crate::GenerationPin,
        batch_digest: &str,
        sealed: bool,
        receipt: &BatchPublishReceipt,
    ) -> Result<(), String> {
        if self.request_id != request_id
            || self.repo_id != pin.repo_id
            || self.revision_id != pin.revision_id
            || self.generation != pin.manifest_generation
            || self.batch_digest != batch_digest
            || self.generation != receipt.generation
            || self.batch_digest != receipt.batch_digest
        {
            return Err(
                "ingest observation identity does not match request and receipt".to_string(),
            );
        }
        if self.activation_ns.is_some() {
            return Err(
                "ingest observation cannot measure a separate activation request".to_string(),
            );
        }
        let semantic = self.semantic.is_some();
        let lexical = self.lexical_build_ns.is_some();
        let finalize = self.finalize_ns.is_some();
        let valid = match self.status {
            IngestObservationStatus::Executed => receipt.applied && semantic && lexical && finalize,
            IngestObservationStatus::PartialRecovery => {
                receipt.applied && semantic != lexical && finalize
            }
            IngestObservationStatus::FinalizeOnly => {
                receipt.applied && !semantic && !lexical && finalize
            }
            IngestObservationStatus::Replayed => {
                !receipt.applied && !semantic && !lexical && !finalize
            }
        };
        if !valid {
            return Err("ingest observation stages contradict execution status".to_string());
        }
        if let Some(report) = &self.semantic {
            if report.durations.seal.is_some() != sealed {
                return Err("ingest observation seal availability contradicts request".to_string());
            }
            let durations = &report.durations;
            let passes = [
                durations.prepare,
                durations.clear_surfaces,
                durations.stream,
                durations.tombstones,
                durations.seal.unwrap_or(0),
                durations.promotion,
            ];
            let pass_total = passes
                .into_iter()
                .try_fold(0_u64, u64::checked_add)
                .ok_or_else(|| "ingest observation duration sum overflow".to_string())?;
            if pass_total > durations.total
                || durations
                    .embedding
                    .is_some_and(|embedding| embedding > durations.stream)
            {
                return Err(
                    "ingest observation nested durations exceed containing build".to_string(),
                );
            }
            let storage_total = [
                durations.semantic_delete,
                durations.membership_delete,
                durations.semantic_append,
                durations.membership_append,
            ]
            .into_iter()
            .try_fold(0_u64, u64::checked_add)
            .ok_or_else(|| "ingest observation storage duration sum overflow".to_string())?;
            let mutation_total = [
                durations.clear_surfaces,
                durations.stream,
                durations.tombstones,
            ]
            .into_iter()
            .try_fold(0_u64, u64::checked_add)
            .ok_or_else(|| "ingest observation mutation duration sum overflow".to_string())?;
            if storage_total > mutation_total {
                return Err(
                    "ingest observation storage durations exceed containing passes".to_string(),
                );
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
        Self {
            receipt,
            observation: None,
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fallible wire fixtures propagate setup errors; test assertions deliberately fail on violated refusal or identity invariants"
)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn batch() -> Result<crate::SearchCorpusIngestBatch, Box<dyn std::error::Error>> {
        let mut batch = crate::SearchCorpusIngestBatch {
            source_event: crate::SourcePublicationEvent {
                stream_id: "fixture".into(),
                event_id: "empty".into(),
                expected_base_event_id: None,
                payload_sha256: [0; 32],
            },
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
            seal: true,
        };
        batch.source_event.payload_sha256 = crate::source_event_payload_sha256(&batch)?;
        Ok(batch)
    }

    fn outcome() -> Result<SearchCorpusPublishOutcome, Box<dyn std::error::Error>> {
        let batch = batch()?;
        Ok(SearchCorpusPublishOutcome {
            receipt: BatchPublishReceipt::empty_for(
                batch.generation,
                Some(batch.manifest_digest),
                batch.batch_digest.clone(),
            ),
            observation: Some(SearchCorpusIngestObservation {
                request_id: 11,
                repo_id: batch.repo_id,
                revision_id: batch.revision_id,
                generation: batch.generation,
                batch_digest: batch.batch_digest,
                status: IngestObservationStatus::Executed,
                semantic: Some(Box::new(IngestStageReport {
                    durations: IngestStageDurations {
                        seal: Some(1),
                        ..IngestStageDurations::default()
                    },
                    ..IngestStageReport::default()
                })),
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
        assert_eq!(
            serde_json::from_value::<SearchCorpusPublishOutcome>(wire.clone())?,
            outcome
        );
        for pointer in [
            "/observation",
            "/observation/semantic",
            "/observation/lexical_build_ns",
            "/observation/finalize_ns",
            "/observation/activation_ns",
            "/observation/semantic/durations/seal",
            "/observation/semantic/durations/embedding",
        ] {
            let (parent, key) = pointer.rsplit_once('/').ok_or("invalid pointer")?;
            let mut missing = wire.clone();
            let _removed = missing
                .pointer_mut(parent)
                .and_then(serde_json::Value::as_object_mut)
                .ok_or("missing object")?
                .remove(key);
            assert!(
                serde_json::from_value::<SearchCorpusPublishOutcome>(missing).is_err(),
                "accepted missing {pointer}"
            );
        }
        let mut unknown = wire;
        let _previous = unknown
            .as_object_mut()
            .ok_or("missing outcome")?
            .insert("unbound_timing".to_string(), serde_json::Value::from(1));
        assert!(serde_json::from_value::<SearchCorpusPublishOutcome>(unknown).is_err());
        let old_payload = serde_json::json!({"SearchCorpusReceipt": outcome.receipt});
        assert!(
            serde_json::from_value::<crate::SearchPlaneIngestIpcResponse>(old_payload).is_err(),
            "old bare receipt must not silently mix with observed payload"
        );
        let duplicated = format!(
            "{{\"receipt\":{},\"observation\":null,\"observation\":null}}",
            serde_json::to_string(&outcome.receipt)?
        );
        assert!(
            serde_json::from_str::<SearchCorpusPublishOutcome>(&duplicated).is_err(),
            "duplicate nullable observation must be refused"
        );
        Ok(())
    }

    #[test]
    fn ingest_observation_manual_wire_preserves_order_and_strict_json_cbor_types() -> TestResult {
        let durations = IngestStageDurations::default();
        assert_eq!(
            serde_json::to_string(&durations)?,
            "{\"total\":0,\"prepare\":0,\"promotion\":0,\"clear_surfaces\":0,\"stream\":0,\"semantic_delete\":0,\"membership_delete\":0,\"semantic_append\":0,\"membership_append\":0,\"tombstones\":0,\"seal\":null,\"embedding\":null}"
        );
        for (status, tag) in [
            (IngestObservationStatus::Executed, "executed"),
            (IngestObservationStatus::PartialRecovery, "partial_recovery"),
            (IngestObservationStatus::FinalizeOnly, "finalize_only"),
            (IngestObservationStatus::Replayed, "replayed"),
        ] {
            assert_eq!(serde_json::to_string(&status)?, format!("\"{tag}\""));
            let mut bytes = Vec::new();
            ciborium::into_writer(&status, &mut bytes)?;
            let value: ciborium::Value = ciborium::from_reader(bytes.as_slice())?;
            assert_eq!(value, ciborium::Value::Text(tag.to_string()));
            assert_eq!(
                ciborium::from_reader::<IngestObservationStatus, _>(bytes.as_slice())?,
                status
            );
        }
        assert!(serde_json::from_str::<IngestObservationStatus>("\"unknown\"").is_err());
        for key in [
            "total",
            "prepare",
            "promotion",
            "clear_surfaces",
            "stream",
            "semantic_delete",
            "membership_delete",
            "semantic_append",
            "membership_append",
            "tombstones",
            "seal",
            "embedding",
        ] {
            for invalid in [
                serde_json::json!(-1),
                serde_json::json!(1.5),
                serde_json::json!("1"),
            ] {
                let mut wire = serde_json::to_value(&durations)?;
                let _replaced = wire
                    .as_object_mut()
                    .ok_or("missing duration object")?
                    .insert(key.to_string(), invalid);
                assert!(serde_json::from_value::<IngestStageDurations>(wire.clone()).is_err());
                let mut bytes = Vec::new();
                ciborium::into_writer(&wire, &mut bytes)?;
                assert!(
                    ciborium::from_reader::<IngestStageDurations, _>(bytes.as_slice()).is_err()
                );
            }
        }
        let mut bytes = Vec::new();
        ciborium::into_writer(&outcome()?, &mut bytes)?;
        let value: ciborium::Value = ciborium::from_reader(bytes.as_slice())?;
        let ciborium::Value::Map(fields) = value else {
            return Err("missing outcome map".into());
        };
        let receipt = fields
            .iter()
            .find(|(key, _)| key == &ciborium::Value::Text("receipt".to_string()))
            .ok_or("missing receipt")?
            .clone();
        for fields in [
            vec![receipt.clone()],
            vec![
                receipt.clone(),
                (
                    ciborium::Value::Text("observation".to_string()),
                    ciborium::Value::Null,
                ),
                (
                    ciborium::Value::Text("observation".to_string()),
                    ciborium::Value::Null,
                ),
            ],
            vec![
                receipt,
                (
                    ciborium::Value::Text("observation".to_string()),
                    ciborium::Value::Null,
                ),
                (
                    ciborium::Value::Text("unknown".to_string()),
                    ciborium::Value::Null,
                ),
            ],
        ] {
            let mut mutant = Vec::new();
            ciborium::into_writer(&ciborium::Value::Map(fields), &mut mutant)?;
            assert!(
                ciborium::from_reader::<SearchCorpusPublishOutcome, _>(mutant.as_slice()).is_err()
            );
        }
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
        assert!(
            observation
                .validate_for(11, &batch, &outcome.receipt)
                .is_err()
        );
        observation.semantic = None;
        observation.lexical_build_ns = None;
        observation.finalize_ns = None;
        observation.validate_for(11, &batch, &outcome.receipt)?;
        observation.lexical_build_ns = Some(0);
        assert!(
            observation
                .validate_for(11, &batch, &outcome.receipt)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn ingest_observation_rejects_partial_stages_and_impossible_timing() -> TestResult {
        let batch = batch()?;
        let mut outcome = outcome()?;
        let observation = outcome.observation.as_mut().ok_or("missing observation")?;
        observation
            .semantic
            .as_mut()
            .ok_or("missing semantic")?
            .durations
            .prepare = 1;
        assert!(
            observation
                .validate_for(11, &batch, &outcome.receipt)
                .is_err()
        );
        observation
            .semantic
            .as_mut()
            .ok_or("missing semantic")?
            .durations
            .prepare = 0;
        observation.status = IngestObservationStatus::PartialRecovery;
        assert!(
            observation
                .validate_for(11, &batch, &outcome.receipt)
                .is_err()
        );
        observation.semantic = None;
        observation.validate_for(11, &batch, &outcome.receipt)?;
        observation.status = IngestObservationStatus::FinalizeOnly;
        assert!(
            observation
                .validate_for(11, &batch, &outcome.receipt)
                .is_err()
        );
        observation.lexical_build_ns = None;
        observation.validate_for(11, &batch, &outcome.receipt)?;
        Ok(())
    }

    #[test]
    fn ingest_observation_wire_discriminant_refuses_mixed_versions_before_dispatch() -> TestResult {
        struct OldRequest {
            body: crate::SearchCorpusIngestBatch,
        }
        impl_observation_struct_serde!(OldRequest {
            body: crate::SearchCorpusIngestBatch => "PublishSearchCorpusBatch",
        });
        let batch = batch()?;
        let body = serde_json::to_value(&batch)?;
        let old_request = serde_json::json!({"PublishSearchCorpusBatch": body});
        let old_decoded: OldRequest = serde_json::from_value(old_request.clone())?;
        assert_eq!(
            old_decoded.body, batch,
            "reference old decoder accepts old fixture"
        );
        assert!(
            serde_json::from_value::<crate::SearchPlaneIngestIpcRequest>(old_request.clone())
                .is_err()
        );
        let mut old_cbor = Vec::new();
        ciborium::into_writer(&old_request, &mut old_cbor)?;
        let old_on_new: Result<crate::SearchPlaneIngestIpcRequest, _> =
            ciborium::from_reader(old_cbor.as_slice());
        assert!(
            old_on_new.is_err(),
            "old CBOR tag must be refused before a dispatchable request exists"
        );
        let new_request = crate::SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch);
        let new_json = serde_json::to_value(&new_request)?;
        assert_eq!(
            new_json.get("PublishSearchCorpusBatchV2"),
            Some(&body),
            "versioning changes only outer wire discriminant, not durable batch bytes"
        );
        assert!(serde_json::from_value::<OldRequest>(new_json).is_err());
        let mut new_cbor = Vec::new();
        ciborium::into_writer(&new_request, &mut new_cbor)?;
        let new_on_old: Result<OldRequest, _> = ciborium::from_reader(new_cbor.as_slice());
        assert!(new_on_old.is_err(), "old decoder must refuse new CBOR tag");
        let outcome = outcome()?;
        let new_response = serde_json::to_value(
            crate::SearchPlaneIngestIpcResponse::SearchCorpusReceipt(outcome),
        )?;
        assert!(new_response.get("SearchCorpusReceiptV2").is_some());
        Ok(())
    }
}
