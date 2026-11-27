//! Native DSL publication uses the ordinary immutable producer transaction.

use std::{collections::BTreeSet, fmt, path::Path};

use quanta_index_contract::{
    index::{
        Digest, IngressRef, ObjectRef, OperationKey, OwnerKey, OwnerMutation, ProducerCompleteness,
        ProjectionBinding, ProjectionCertificate, ProjectionId, ProjectionOwnerContribution,
        ProjectionTransition, SourceEvent, SubmitOutcome, UpdateKind, UpdateRequest, ViewRef,
        ViewSelection,
    },
    native_protocol::{
        NATIVE_DSL_HEADER_OWNER_V1, NATIVE_DSL_POLICY_V1, NATIVE_DSL_PRODUCER_V1,
        NATIVE_DSL_RUNTIME_CATALOG_MAX_BYTES_V1, NATIVE_DSL_RUNTIME_CATALOG_PAGE_BYTES_V1,
        NATIVE_DSL_SCHEMA_V1,
        MAX_NATIVE_DSL_STRUCTURAL_CHUNKS_PER_PAGE_V1, MAX_NATIVE_DSL_STRUCTURAL_CHUNK_BYTES_V1,
        NativeDslStructuralChunksCursorV1,
        NativeDslCorpusV1, NativeDslCursorV1, NativeDslFileV1,
        NativeDslKindV1, NativeDslPageV1, NativeDslQueryV1, NativeDslReplayBasisV1,
        NativeDslReplayV1, NativeDslRuntimeCatalogV1, NativeDslScopeV1, NativeIndexRequest,
        NativeIndexResponse, NativeReadSessionRequest, NativeReadSessionState,
        NativeReadSessionStatus, NativeSealedDslSubmitV1, native_dsl_context_v1, native_dsl_file_owner_v1,
        native_dsl_sequence_v1,
    },
};
use quanta_index_ipc::{
    CanonicalBatchDigestProgressError, MemoryLease, MemoryLifetime, MemoryScope, RequestBudgetV1,
    canonical_batch_digest_with_progress_v1, matches_batch_digest_token_v1,
};
use serde::{Deserialize, Serialize, de::Visitor};
use sha2::{Digest as _, Sha256};

use super::{AdmittedNativePublicationReply, NativeUdsClient, NativeUdsError};

const MAX_PROJECTION_OBJECT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct NativeDslFileChangeV1 {
    pub source_owner: OwnerKey,
    /// None removes the previous contribution, including when the source was
    /// deleted. An admitted file with no chunks is an explicit empty value.
    pub next: Option<NativeDslFileV1>,
}
quanta_index_contract::native_serde_struct!([serde_identifier] NativeDslFileChangeV1 {
    source_owner: OwnerKey,
    next: Option<NativeDslFileV1> = default,
});

#[derive(Clone, Debug)]
pub enum NativeDslChangesV1 {
    Corpus {
        header: NativeDslCorpusV1,
        files: Vec<NativeDslFileChangeV1>,
    },
    RuntimeMetadata(NativeDslRuntimeCatalogV1),
}

impl Serialize for NativeDslChangesV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Corpus { header, files } => {
                let mut record =
                    serializer.serialize_struct_variant("NativeDslChangesV1", 0, "Corpus", 2)?;
                serde::ser::SerializeStructVariant::serialize_field(&mut record, "header", header)?;
                serde::ser::SerializeStructVariant::serialize_field(&mut record, "files", files)?;
                serde::ser::SerializeStructVariant::end(record)
            }
            Self::RuntimeMetadata(runtime) => serializer.serialize_newtype_variant(
                "NativeDslChangesV1",
                1,
                "RuntimeMetadata",
                runtime,
            ),
        }
    }
}

enum DslChangeVariant {
    Corpus,
    RuntimeMetadata,
}

impl<'de> Deserialize<'de> for DslChangeVariant {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct NameVisitor;
        impl Visitor<'_> for NameVisitor {
            type Value = DslChangeVariant;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a Native DSL change variant")
            }

            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<Self::Value, E> {
                match name {
                    "Corpus" => Ok(DslChangeVariant::Corpus),
                    "RuntimeMetadata" => Ok(DslChangeVariant::RuntimeMetadata),
                    _ => Err(E::unknown_variant(name, &["Corpus", "RuntimeMetadata"])),
                }
            }

            fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
                let name = std::str::from_utf8(bytes).map_err(|_error| {
                    E::invalid_value(serde::de::Unexpected::Bytes(bytes), &self)
                })?;
                self.visit_str(name)
            }

            fn visit_u64<E: serde::de::Error>(self, index: u64) -> Result<Self::Value, E> {
                match index {
                    0 => Ok(DslChangeVariant::Corpus),
                    1 => Ok(DslChangeVariant::RuntimeMetadata),
                    _ => Err(E::invalid_value(
                        serde::de::Unexpected::Unsigned(index),
                        &self,
                    )),
                }
            }
        }
        decoder.deserialize_identifier(NameVisitor)
    }
}

enum CorpusField {
    Header,
    Files,
    Ignore,
}

impl<'de> Deserialize<'de> for CorpusField {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct NameVisitor;
        impl Visitor<'_> for NameVisitor {
            type Value = CorpusField;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a Native DSL corpus field")
            }

            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<Self::Value, E> {
                Ok(match name {
                    "header" => CorpusField::Header,
                    "files" => CorpusField::Files,
                    _ => CorpusField::Ignore,
                })
            }

            fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
                Ok(match bytes {
                    b"header" => CorpusField::Header,
                    b"files" => CorpusField::Files,
                    _ => CorpusField::Ignore,
                })
            }
        }
        decoder.deserialize_identifier(NameVisitor)
    }
}

impl<'de> Deserialize<'de> for NativeDslChangesV1 {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct ChangesVisitor;
        impl<'de> Visitor<'de> for ChangesVisitor {
            type Value = NativeDslChangesV1;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a Native DSL change variant")
            }

            fn visit_enum<A: serde::de::EnumAccess<'de>>(
                self,
                variants: A,
            ) -> Result<Self::Value, A::Error> {
                use serde::de::VariantAccess as _;
                let (variant, payload) = variants.variant::<DslChangeVariant>()?;
                match variant {
                    DslChangeVariant::RuntimeMetadata => payload
                        .newtype_variant()
                        .map(NativeDslChangesV1::RuntimeMetadata),
                    DslChangeVariant::Corpus => {
                        struct CorpusVisitor;
                        impl<'de> Visitor<'de> for CorpusVisitor {
                            type Value = NativeDslChangesV1;

                            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                                formatter.write_str("a Native DSL Corpus record")
                            }

                            fn visit_map<M: serde::de::MapAccess<'de>>(
                                self,
                                mut map: M,
                            ) -> Result<Self::Value, M::Error> {
                                let mut header = None;
                                let mut files = None;
                                while let Some(field) = map.next_key::<CorpusField>()? {
                                    match field {
                                        CorpusField::Header => {
                                            if header.is_some() {
                                                return Err(serde::de::Error::duplicate_field(
                                                    "header",
                                                ));
                                            }
                                            header = Some(map.next_value()?);
                                        }
                                        CorpusField::Files => {
                                            if files.is_some() {
                                                return Err(serde::de::Error::duplicate_field(
                                                    "files",
                                                ));
                                            }
                                            files = Some(map.next_value()?);
                                        }
                                        CorpusField::Ignore => {
                                            let _: serde::de::IgnoredAny = map.next_value()?;
                                        }
                                    }
                                }
                                Ok(NativeDslChangesV1::Corpus {
                                    header: header
                                        .ok_or_else(|| serde::de::Error::missing_field("header"))?,
                                    files: files
                                        .ok_or_else(|| serde::de::Error::missing_field("files"))?,
                                })
                            }

                            fn visit_seq<M: serde::de::SeqAccess<'de>>(
                                self,
                                mut sequence: M,
                            ) -> Result<Self::Value, M::Error> {
                                let header = sequence
                                    .next_element()?
                                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                                let files = sequence
                                    .next_element()?
                                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                                if sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {
                                    return Err(serde::de::Error::invalid_length(3, &self));
                                }
                                Ok(NativeDslChangesV1::Corpus { header, files })
                            }
                        }
                        payload.struct_variant(&["header", "files"], CorpusVisitor)
                    }
                }
            }
        }
        decoder.deserialize_enum(
            "NativeDslChangesV1",
            &["Corpus", "RuntimeMetadata"],
            ChangesVisitor,
        )
    }
}

impl NativeDslChangesV1 {
    fn scope_kind(&self) -> (NativeDslScopeV1, NativeDslKindV1) {
        match self {
            Self::Corpus { header, .. } => (
                NativeDslScopeV1::for_pin(&header.pin),
                NativeDslKindV1::Structural,
            ),
            Self::RuntimeMetadata(runtime) => (
                NativeDslScopeV1 {
                    repository: runtime.catalog.repo_id.clone(),
                    revision: runtime.catalog.revision_id.clone(),
                },
                NativeDslKindV1::RuntimeMetadata,
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NativeDslPublicationV1 {
    pub operation: OperationKey,
    pub expected_view: ViewRef,
    pub event_predecessor: u64,
    pub authority: Digest,
    pub changes: NativeDslChangesV1,
}

/// Original candidate ingress and immutable Q request retained before admission.
pub struct SealedNativeDslRequestV1 {
    pub request: UpdateRequest,
    pub submission: NativeSealedDslSubmitV1,
}
quanta_index_contract::native_serde_struct!([serde_identifier] NativeDslPublicationV1 {
    operation: OperationKey,
    expected_view: ViewRef,
    event_predecessor: u64,
    authority: Digest,
    changes: NativeDslChangesV1,
});

#[cfg(test)]
mod codec_tests {
    use super::*;
    use quanta_index_contract::{
        GenerationPin, ManifestGeneration, RepoId, RevisionId,
        index::{ProducerId, StableOwnerId},
    };

    #[test]
    fn dsl_file_and_corpus_keep_old_cbor_and_optional_open_fields() {
        let file = NativeDslFileChangeV1 {
            source_owner: OwnerKey {
                producer: ProducerId([3; 16]),
                stable_id: StableOwnerId([4; 16]),
            },
            next: None,
        };
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&file, &mut encoded).expect("file CBOR");
        assert_eq!(
            format!("{:x}", Sha256::digest(&encoded)),
            "2a57f0300247cedc37c47c823321326860e103f11da2dadd403e5da2b4e815ff"
        );
        let mut value = ciborium::value::Value::serialized(&file).expect("file value");
        let ciborium::value::Value::Map(fields) = &mut value else {
            panic!("file record must be a map");
        };
        fields.retain(|(key, _)| key != &ciborium::value::Value::Text("next".into()));
        let (key, _) = fields.first_mut().expect("source owner");
        *key = ciborium::value::Value::Bytes(b"source_owner".to_vec());
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&value, &mut encoded).expect("file byte-key CBOR");
        let decoded: NativeDslFileChangeV1 =
            ciborium::de::from_reader(encoded.as_slice()).expect("missing next defaults");
        assert!(decoded.next.is_none());

        let corpus = NativeDslChangesV1::Corpus {
            header: NativeDslCorpusV1 {
                pin: GenerationPin::new(
                    RepoId::new("repo").expect("repo"),
                    RevisionId::new("revision").expect("revision"),
                    ManifestGeneration::new(1),
                ),
                source_producer: ProducerId([3; 16]),
                structural_ready: true,
            },
            files: vec![file],
        };
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&corpus, &mut encoded).expect("corpus CBOR");
        assert_eq!(
            format!("{:x}", Sha256::digest(&encoded)),
            "674942f86f11f3135c3bddb8891ec07ddde3e7344a9e2d1321028b7cd40852ba"
        );
        let mut value = ciborium::value::Value::serialized(&corpus).expect("corpus value");
        let ciborium::value::Value::Map(variants) = &mut value else {
            panic!("changes must be a map");
        };
        let (variant, payload) = variants.first_mut().expect("Corpus variant");
        *variant = ciborium::value::Value::Bytes(b"Corpus".to_vec());
        let ciborium::value::Value::Map(fields) = payload else {
            panic!("Corpus must be a record");
        };
        fields.push((
            ciborium::value::Value::Text("irrelevant".into()),
            ciborium::value::Value::Null,
        ));
        fields.push((
            ciborium::value::Value::Bytes(vec![0xff]),
            ciborium::value::Value::Null,
        ));
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&value, &mut encoded).expect("open Corpus CBOR");
        assert!(matches!(
            ciborium::de::from_reader::<NativeDslChangesV1, _>(encoded.as_slice()),
            Ok(NativeDslChangesV1::Corpus { .. })
        ));
    }
}

impl<P: AsRef<Path>> NativeUdsClient<P> {
    /// Prepare the exact operation. A caller requiring acknowledgement-loss
    /// recovery retains this request and uses the existing submit/status door.
    pub fn prepare_dsl_publication_v1(
        &self,
        input: &NativeDslPublicationV1,
        budget: &RequestBudgetV1,
    ) -> Result<UpdateRequest, NativeUdsError> {
        self.prepare_dsl_publication_scoped_v1(input, input.authority, budget, None, None)
            .map(|(request, _)| request)
    }

    pub fn prepare_dsl_publication_admitted_v1(
        &self,
        input: &NativeDslPublicationV1,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<UpdateRequest, NativeUdsError> {
        self.prepare_dsl_publication_scoped_v1(input, input.authority, budget, Some(memory), None)
            .map(|(request, _)| request)
    }

    /// Seal the canonical request under the same ingress as its projection
    /// inputs so the ordinary Submit preserves that fence.
    pub fn prepare_dsl_publication_sealed_admitted_v1(
        &self,
        input: &NativeDslPublicationV1,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<SealedNativeDslRequestV1, NativeUdsError> {
        let (request, ingress) = self.prepare_dsl_publication_scoped_v1(
            input, input.authority, budget, Some(memory), None,
        )?;
        let request_ref = self.seal_dsl_request_borrowed_admitted_v1(
            ingress, &request, budget, memory,
        )?;
        let (scope, kind) = input.changes.scope_kind();
        let replay = NativeDslReplayV1 {
            operation: input.operation,
            projection: scope.projection(kind),
            basis: NativeDslReplayBasisV1::Exact {
                view: input.expected_view,
                event_predecessor: input.event_predecessor,
            },
            authority: input.authority,
        };
        Ok(SealedNativeDslRequestV1 {
            request,
            submission: NativeSealedDslSubmitV1 { replay, ingress, request_ref },
        })
    }

    fn prepare_dsl_publication_scoped_v1(
        &self,
        input: &NativeDslPublicationV1,
        authority: Digest,
        budget: &RequestBudgetV1,
        memory: Option<&MemoryScope>,
        checked_header_size: Option<usize>,
    ) -> Result<(UpdateRequest, IngressRef), NativeUdsError> {
        let access = DslAccess {
            client: self,
            memory,
            budget,
        };
        let tracked_inputs = match &input.changes {
            NativeDslChangesV1::Corpus { files, .. } => files.len(),
            NativeDslChangesV1::RuntimeMetadata(runtime) => runtime.dirty.len(),
        };
        let scratch_bytes = u64::try_from(tracked_inputs)
            .ok()
            .and_then(|count| count.checked_mul(128))
            .and_then(|bytes| bytes.checked_add(64 * 1024))
            .ok_or_else(|| protocol("Native DSL publication allocation bounds overflow"))?;
        let _scratch = access.reserve(scratch_bytes)?;
        match &input.changes {
            NativeDslChangesV1::Corpus { header, files } => {
                let mut owners = BTreeSet::new();
                for change in files {
                    let owner = native_dsl_file_owner_v1(change.source_owner);
                    if change.source_owner.producer != header.source_producer
                        || owner == NATIVE_DSL_HEADER_OWNER_V1
                        || !owners.insert(owner)
                    {
                        return Err(protocol("Native DSL file changes repeat an owner"));
                    }
                }
            }
            NativeDslChangesV1::RuntimeMetadata(runtime) => {
                validate_runtime_catalog_payload(runtime, budget)?;
            }
        };
        let (scope, kind) = input.changes.scope_kind();
        let key = input.operation;
        if key.producer != NATIVE_DSL_PRODUCER_V1
            || key.stream != scope.stream(kind)
            || key.workspace != input.expected_view.workspace
            || key.epoch != input.expected_view.epoch
            || key.stream_epoch != 1
        {
            return Err(protocol(
                "Native DSL operation does not own the requested projection",
            ));
        }
        let event_sequence = input
            .event_predecessor
            .checked_add(1)
            .ok_or_else(|| protocol("Native DSL event sequence overflow"))?;
        let header_size = match checked_header_size {
            Some(size) => size,
            None => publication_header_size(&input.changes, budget)?,
        };
        super::read_session::admitted_budget_checkpoint(budget)?;
        let selection = ViewSelection::Exact(input.expected_view);
        let view = access.call(NativeIndexRequest::ReadView(selection), |response| {
            if let NativeIndexResponse::View(view) = response {
                Some(view.clone())
            } else {
                None
            }
        })?;
        if memory.is_none() {
            super::validate_catalog_view_selection(selection, &view)?;
        }
        let projection = scope.projection(kind);
        let prior = access.projection(selection, projection)?;
        if prior.is_some_and(|binding| !is_dsl_binding(binding, projection)) {
            return Err(protocol("Native DSL predecessor binding differs"));
        }
        if let NativeDslChangesV1::RuntimeMetadata(runtime) = &input.changes {
            let corpus = scope.projection(NativeDslKindV1::Structural);
            let binding = access.projection(selection, corpus)?;
            if !binding.is_some_and(|binding| {
                is_dsl_binding(binding, corpus)
                    && binding.source == view.source
                    && binding.certificate == runtime.corpus_certificate
            }) {
                return Err(protocol(
                    "Native RuntimeMetadata requires the current chunk projection",
                ));
            }
        }
        if prior.map_or(0, |binding| {
            native_dsl_sequence_v1(binding.dependency_context)
        }) != input.event_predecessor
        {
            return Err(protocol(
                "Native DSL event predecessor differs from its retained projection",
            ));
        }
        if input.authority == Digest([0; 32]) {
            return Err(protocol(
                "Native DSL publication requires the producer input identity",
            ));
        }
        let context = native_dsl_context_v1(event_sequence, authority);
        let ingress = access.call(NativeIndexRequest::BeginInput(key), |response| {
            if let NativeIndexResponse::InputOpened(ingress) = response {
                (ingress.operation == key).then_some(*ingress)
            } else {
                None
            }
        })?;
        let stage = PublicationStage {
            access,
            selection,
            ingress,
            projection,
            prior,
        };
        let header = match &input.changes {
            NativeDslChangesV1::Corpus { header, .. } => stage.replace(
                NATIVE_DSL_HEADER_OWNER_V1,
                None,
                Some(header),
                Some(header_size),
            )?,
            NativeDslChangesV1::RuntimeMetadata(runtime) => stage.replace(
                NATIVE_DSL_HEADER_OWNER_V1,
                None,
                Some(runtime),
                Some(header_size),
            )?,
        }
        .ok_or_else(|| protocol("Native DSL header mutation is missing"))?;
        let changes = match &input.changes {
            NativeDslChangesV1::Corpus { files, .. } => files.as_slice(),
            NativeDslChangesV1::RuntimeMetadata(_) => &[],
        };
        let owners = self.stage_mutations_with_send(
            ingress,
            std::iter::once(Ok(header)).chain(changes.iter().filter_map(|file| {
                match stage.file(file) {
                    Ok(Some(change)) => Some(Ok(change)),
                    Ok(None) => None,
                    Err(error) => Some(Err(error)),
                }
            })),
            |request| {
                stage.access.call(request, |response| match response {
                    NativeIndexResponse::MutationPageStaged(root) => {
                        Some(NativeIndexResponse::MutationPageStaged(*root))
                    }
                    NativeIndexResponse::MutationsStaged(set) => {
                        Some(NativeIndexResponse::MutationsStaged(*set))
                    }
                    _ => None,
                })
            },
        )?;
        let certificate = stage
            .access
            .staged(NativeIndexRequest::StageProjectionCertificate {
                ingress,
                certificate: ProjectionCertificate {
                    projection,
                    producer: key.producer,
                    prior,
                    target_source: view.source,
                    schema: NATIVE_DSL_SCHEMA_V1,
                    producer_policy: NATIVE_DSL_POLICY_V1,
                    dependency_context: context,
                    affected: owners,
                },
            })?;
        let mut transition = ProjectionTransition {
            projection,
            producer: key.producer,
            expected: prior,
            target_source: view.source,
            schema: NATIVE_DSL_SCHEMA_V1,
            producer_policy: NATIVE_DSL_POLICY_V1,
            dependency_context: context,
            completeness: ProducerCompleteness::CompleteAffectedDomain { certificate },
            owners,
            events: Vec::new(),
        };
        transition.events.push(SourceEvent {
            producer: key.producer,
            stream: key.stream,
            stream_epoch: key.stream_epoch,
            predecessor: input.event_predecessor,
            sequence: event_sequence,
            payload: transition
                .payload_digest()
                .map_err(|error| NativeUdsError::Protocol(error.to_string()))?,
        });
        let request = UpdateRequest {
            operation: key,
            expected_view: input.expected_view,
            kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
        };
        Ok((request, ingress))
    }

    pub fn publish_dsl_v1(
        &self,
        input: &NativeDslPublicationV1,
        budget: &RequestBudgetV1,
    ) -> Result<SubmitOutcome, NativeUdsError> {
        let header_size = publication_header_size(&input.changes, budget)?;
        let authority = publication_digest(input, budget)?;
        let (scope, kind) = input.changes.scope_kind();
        let replay = NativeDslReplayV1 {
            operation: input.operation,
            projection: scope.projection(kind),
            basis: NativeDslReplayBasisV1::Exact {
                view: input.expected_view,
                event_predecessor: input.event_predecessor,
            },
            authority,
        };
        let access = DslAccess {
            client: self,
            memory: None,
            budget,
        };
        if let outcome @ (SubmitOutcome::Accepted(_) | SubmitOutcome::Terminal(_)) =
            access.replay(replay)?
        {
            return Ok(outcome);
        }
        let request = match self.prepare_dsl_publication_scoped_v1(
            input, authority, budget, None, Some(header_size),
        ) {
            Ok((request, _)) => request,
            Err(error) => match access.replay(replay) {
                Ok(SubmitOutcome::Unknown(_)) | Err(NativeUdsError::Unknown(_)) => {
                    return Err(error);
                }
                result => return result,
            },
        };
        self.submit_borrowed(&request, budget)
    }

    /// Reconcile producer-owned facts before any attempt to reopen staging.
    pub fn replay_dsl_publication_admitted_v1(
        &self,
        input: NativeDslReplayV1,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<SubmitOutcome, NativeUdsError> {
        DslAccess {
            client: self,
            memory: Some(memory),
            budget,
        }
        .replay(input)
    }

    /// Read an existing seal before recording the producer's publication claim.
    pub fn read_sealed_dsl_admitted_v1(
        &self,
        replay: NativeDslReplayV1,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<Option<NativeSealedDslSubmitV1>, NativeUdsError> {
        DslAccess { client: self, memory: Some(memory), budget }.call(
            NativeIndexRequest::ReadSealedDslV1(replay),
            |response| match response {
                NativeIndexResponse::SealedDslRequestV1(seal) => Some(*seal),
                _ => None,
            },
        )
    }

    pub fn submit_sealed_dsl_admitted_v1(
        &self,
        input: NativeSealedDslSubmitV1,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<SubmitOutcome, NativeUdsError> {
        DslAccess { client: self, memory: Some(memory), budget }.call(
            NativeIndexRequest::SubmitSealedDslV1(input),
            |response| match response {
                NativeIndexResponse::Submitted(reply) =>
                    super::validate_submit_reply(input.replay.operation, reply.clone()).ok(),
                _ => None,
            },
        )
    }

    pub fn search_dsl_in_read_session_v1(
        &self,
        expected: &NativeReadSessionStatus,
        query: NativeDslQueryV1,
        budget: &RequestBudgetV1,
    ) -> Result<NativeDslPageV1, NativeUdsError> {
        let digest = prepare_dsl_read(expected, &query)?;
        let kind = query.kind;
        let limit = query.text.top_k;
        let previous = query.after.clone();
        let NativeIndexResponse::DslPageV1(page) = self.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchDslV1 {
                session: expected.session,
                query,
            }),
            budget,
        )?
        else {
            return Err(protocol("unexpected retained Native DSL response"));
        };
        validate_dsl_page(expected, digest, kind, limit, previous.as_ref(), &page)?;
        Ok(page)
    }

    /// Return the canonical page with its Original raw and decoded backing.
    pub fn search_dsl_in_read_session_admitted_v1(
        &self,
        expected: &NativeReadSessionStatus,
        query: NativeDslQueryV1,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        super::read_session::admitted_budget_checkpoint(budget)?;
        let digest = prepare_dsl_read(expected, &query)?;
        let wire = NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchDslV1 {
            session: expected.session,
            query,
        });
        let reply = self.request_publication_admitted_ref(&wire, budget, memory)?;
        let NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchDslV1 {
            query, ..
        }) = &wire
        else {
            return Err(protocol("unexpected retained Native DSL request"));
        };
        match reply.response() {
            NativeIndexResponse::DslPageV1(page) => validate_dsl_page(
                expected,
                digest,
                query.kind,
                query.text.top_k,
                query.after.as_ref(),
                page,
            )?,
            NativeIndexResponse::Error(_) => {}
            _ => return Err(protocol("unexpected retained Native DSL response")),
        }
        super::read_session::admitted_budget_checkpoint(budget)?;
        Ok(reply)
    }

    /// Read one byte page of the complete RuntimeMetadata predecessor. The
    /// caller keeps the exact session open and checks immutable page identity
    /// while assembling the catalog under its Original memory scope.
    pub fn read_session_runtime_catalog_page_admitted_v1(
        &self,
        expected: &NativeReadSessionStatus,
        scope: NativeDslScopeV1,
        offset: u32,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        super::read_session::admitted_budget_checkpoint(budget)?;
        if expected.state != NativeReadSessionState::Active
            || !offset.is_multiple_of(NATIVE_DSL_RUNTIME_CATALOG_PAGE_BYTES_V1)
        {
            return Err(protocol("Native RuntimeMetadata session or offset differs"));
        }
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadDslRuntimeCatalogV1 {
            session: expected.session,
            scope,
            offset,
        });
        let reply = self.request_publication_admitted_ref(&request, budget, memory)?;
        match reply.response() {
            NativeIndexResponse::DslRuntimeCatalogPageV1(page)
                if page.session == expected.session
                    && page.view == expected.view
                    && page.matches_offset_v1(offset)
                    && page.structural_certificate.0 != Digest([0; 32])
                    && page.projection.0 != Digest([0; 32])
                    && page.output.0 != Digest([0; 32]) => {}
            NativeIndexResponse::Error(_) => {}
            _ => return Err(protocol("Native RuntimeMetadata page binding differs")),
        }
        super::read_session::admitted_budget_checkpoint(budget)?;
        Ok(reply)
    }
    /// Enumerate Structural chunk membership in one retained view. Continue
    /// with the returned cursor until it is absent. A page may be empty while
    /// advancing through owners with no chunks.
    pub fn read_session_structural_chunks_admitted_v1(
        &self,
        expected: &NativeReadSessionStatus,
        scope: NativeDslScopeV1,
        after: Option<NativeDslStructuralChunksCursorV1>,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        super::read_session::admitted_budget_checkpoint(budget)?;
        if expected.state != NativeReadSessionState::Active
            || after.is_some_and(|cursor| cursor.view != expected.view)
        {
            return Err(protocol("Native Structural chunk session or cursor differs"));
        }
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadDslStructuralChunksV1 {
            session: expected.session, scope, after,
        });
        let reply = self.request_publication_admitted_ref(&request, budget, memory)?;
        match reply.response() {
            NativeIndexResponse::DslStructuralChunksPageV1(page)
                if page.session == expected.session
                    && page.view == expected.view
                    && page.structural_certificate.0 != Digest([0; 32])
                    && page.projection.0 != Digest([0; 32])
                    && page.chunks.len() <= MAX_NATIVE_DSL_STRUCTURAL_CHUNKS_PER_PAGE_V1
                    && page.chunks.iter().try_fold(0_usize, |bytes, chunk| {
                        bytes.checked_add(chunk.chunk_id.as_str().len())
                    }).is_some_and(|bytes| bytes <= MAX_NATIVE_DSL_STRUCTURAL_CHUNK_BYTES_V1)
                    && page.chunks.iter().all(|chunk| {
                        chunk.owner.producer == NATIVE_DSL_PRODUCER_V1
                            && chunk.owner != NATIVE_DSL_HEADER_OWNER_V1
                    })
                    && page.chunks.windows(2).all(|pair| pair[0].owner <= pair[1].owner)
                    && page.chunks.first().is_none_or(|chunk| after.is_none_or(|cursor| {
                        chunk.owner > cursor.owner
                            || (chunk.owner == cursor.owner && cursor.chunk_offset.is_some())
                    }))
                    && page.chunks.last().is_none_or(|chunk| {
                        page.after.is_none_or(|cursor| cursor.owner >= chunk.owner)
                    })
                    && after.is_none_or(|cursor| cursor.projection == page.projection
                        && cursor.structural_certificate == page.structural_certificate)
                    && page.after.is_none_or(|cursor| cursor.view == page.view
                        && cursor.projection == page.projection
                        && cursor.structural_certificate == page.structural_certificate
                        && after.is_none_or(|previous| cursor.owner > previous.owner
                            || (cursor.owner == previous.owner
                                && previous.chunk_offset.is_some_and(|offset|
                                    cursor.chunk_offset.is_none_or(|next| next > offset))))) => {}
            NativeIndexResponse::Error(_) => {}
            _ => return Err(protocol("Native Structural chunk page binding differs")),
        }
        super::read_session::admitted_budget_checkpoint(budget)?;
        Ok(reply)
    }
}

fn publication_digest(
    input: &NativeDslPublicationV1,
    budget: &RequestBudgetV1,
) -> Result<Digest, NativeUdsError> {
    struct HashWriter<'a>(Sha256, &'a RequestBudgetV1);
    impl std::io::Write for HashWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.1
                .checkpoint("native-dsl:publication-identity")
                .map_err(std::io::Error::other)?;
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    super::read_session::admitted_budget_checkpoint(budget)?;
    let mut hash = HashWriter(Sha256::new(), budget);
    hash.0.update(b"quanta-native-dsl-publication-v1\0");
    let encoded = ciborium::ser::into_writer(input, &mut hash);
    super::read_session::admitted_budget_checkpoint(budget)?;
    encoded.map_err(|error| NativeUdsError::Protocol(error.to_string()))?;
    Ok(Digest(hash.0.finalize().into()))
}

fn prepare_dsl_read(
    expected: &NativeReadSessionStatus,
    query: &NativeDslQueryV1,
) -> Result<Digest, NativeUdsError> {
    let digest = query.digest().map_err(protocol)?;
    if expected.state != NativeReadSessionState::Active
        || query.after.as_ref().is_some_and(|cursor| {
            cursor.view != expected.view || cursor.query != digest || cursor.candidate_id.is_empty()
        })
    {
        return Err(protocol(
            "Native DSL session or cursor differs from the requested query",
        ));
    }
    Ok(digest)
}

fn validate_dsl_page(
    expected: &NativeReadSessionStatus,
    digest: Digest,
    kind: NativeDslKindV1,
    limit: u32,
    previous: Option<&NativeDslCursorV1>,
    page: &NativeDslPageV1,
) -> Result<(), NativeUdsError> {
    if page.session != expected.session
        || page.view != expected.view
        || page.query != digest
        || page.rows.kind() != kind
        || page.window.returned() > limit
        || page.window.has_more() != page.after.is_some()
        || previous.is_some_and(|cursor| cursor.projection != page.projection)
    {
        return Err(protocol("Native DSL response authority differs"));
    }
    let mut boundary = previous.map(|cursor| cursor.candidate_id.as_str());
    let mut count = 0;
    for candidate in page.rows.candidate_ids() {
        if candidate.is_empty() || boundary.is_some_and(|before| before >= candidate) {
            return Err(protocol("Native DSL response candidate order differs"));
        }
        boundary = Some(candidate);
        count += 1;
    }
    if count != page.window.returned()
        || page.after.as_ref().is_some_and(|cursor| {
            count == 0
                || cursor.view != page.view
                || cursor.projection != page.projection
                || cursor.query != page.query
                || Some(cursor.candidate_id.as_str()) != boundary
        })
    {
        return Err(protocol("Native DSL response continuation differs"));
    }
    Ok(())
}

fn is_dsl_binding(binding: ProjectionBinding, projection: ProjectionId) -> bool {
    binding.projection == projection
        && binding.producer == NATIVE_DSL_PRODUCER_V1
        && binding.schema == NATIVE_DSL_SCHEMA_V1
        && binding.producer_policy == NATIVE_DSL_POLICY_V1
}

fn validate_runtime_catalog_payload(
    runtime: &NativeDslRuntimeCatalogV1,
    budget: &RequestBudgetV1,
) -> Result<(), NativeUdsError> {
    let checkpoint = || super::read_session::admitted_budget_checkpoint(budget);
    let mut dirty = BTreeSet::new();
    for record in &runtime.dirty {
        checkpoint()?;
        if record.wire_version != 1
            || record.doc_id.as_str().is_empty()
            || !dirty.insert(&record.doc_id)
        {
            return Err(protocol("invalid Native RuntimeMetadata dirty membership"));
        }
    }
    if !runtime.catalog.batch_digest.is_empty() {
        let digest =
            canonical_batch_digest_with_progress_v1(&runtime.catalog, &mut |_| checkpoint())
                .map_err(|error| match error {
                    CanonicalBatchDigestProgressError::Progress(error) => error,
                    CanonicalBatchDigestProgressError::Encode(error) => {
                        NativeUdsError::Protocol(error.to_string())
                    }
                })?;
        if !matches_batch_digest_token_v1(&runtime.catalog.batch_digest, &digest) {
            return Err(protocol("Native RuntimeMetadata catalog digest differs"));
        }
    }
    Ok(())
}

struct DslAccess<'a, P> {
    client: &'a NativeUdsClient<P>,
    memory: Option<&'a MemoryScope>,
    budget: &'a RequestBudgetV1,
}

impl<P: AsRef<Path>> DslAccess<'_, P> {
    fn replay(&self, input: NativeDslReplayV1) -> Result<SubmitOutcome, NativeUdsError> {
        self.call(NativeIndexRequest::ReplayDslV1(input), |response| {
            if let NativeIndexResponse::Submitted(reply) = response {
                super::validate_submit_reply(input.operation, reply.clone()).ok()
            } else {
                None
            }
        })
    }

    fn reserve(&self, bytes: u64) -> Result<Option<MemoryLease>, NativeUdsError> {
        self.memory
            .map(|memory| {
                memory
                    .reserve(bytes, MemoryLifetime::Temporary)
                    .map_err(|error| NativeUdsError::Protocol(error.to_string()))
            })
            .transpose()
    }

    fn call<T>(
        &self,
        request: NativeIndexRequest,
        take: impl FnOnce(&NativeIndexResponse) -> Option<T>,
    ) -> Result<T, NativeUdsError> {
        let operation = super::publication_request_key(&request);
        let convert = |response: &NativeIndexResponse| {
            if let NativeIndexResponse::Error(error) = response {
                return Err(NativeUdsError::Refused(error.clone()));
            }
            take(response).ok_or_else(|| {
                operation.map_or_else(
                    || protocol("unexpected Native DSL publication reply"),
                    NativeUdsError::Unknown,
                )
            })
        };
        if let Some(memory) = self.memory {
            let reply = self
                .client
                .request_publication_admitted(request, self.budget, memory)?;
            convert(reply.response())
        } else {
            let reply = match operation {
                Some(key) => self.client.write_stage(key, request, self.budget),
                None => self.client.request(request, self.budget),
            }?;
            convert(&reply)
        }
    }

    fn projection(
        &self,
        selection: ViewSelection,
        projection: ProjectionId,
    ) -> Result<Option<ProjectionBinding>, NativeUdsError> {
        self.call(
            NativeIndexRequest::ReadProjection {
                selection,
                projection,
            },
            |response| {
                if let NativeIndexResponse::Projection(result) = response
                    && super::projection_read_matches_request(selection, projection, result)
                {
                    Some(result.binding)
                } else {
                    None
                }
            },
        )
    }

    fn staged(&self, request: NativeIndexRequest) -> Result<ObjectRef, NativeUdsError> {
        self.call(request, |response| {
            if let NativeIndexResponse::Staged(reference) = response {
                Some(*reference)
            } else {
                None
            }
        })
    }
}

struct PublicationStage<'a, P> {
    access: DslAccess<'a, P>,
    selection: ViewSelection,
    ingress: IngressRef,
    projection: ProjectionId,
    prior: Option<ProjectionBinding>,
}

impl<P: AsRef<Path>> PublicationStage<'_, P> {
    fn file(
        &self,
        change: &NativeDslFileChangeV1,
    ) -> Result<Option<OwnerMutation>, NativeUdsError> {
        let owner = native_dsl_file_owner_v1(change.source_owner);
        let source = if change.next.is_some() {
            let owners =
                super::NativeSourceOwnerKeys::new(vec![change.source_owner]).map_err(protocol)?;
            let key = change.source_owner;
            let source = self.access.call(NativeIndexRequest::LookupSourceOwners {
                selection: self.selection, owners,
            }, |response| {
                if let NativeIndexResponse::SourceOwners(result) = response
                    && matches!(self.selection, ViewSelection::Exact(view) if view == result.view)
                    && let [row] = result.rows.as_slice()
                    && row.owner == key
                {
                    Some(row.source)
                } else { None }
            })?.ok_or_else(|| protocol("Native DSL source file is absent"))?;
            Some((key, source))
        } else {
            None
        };
        self.replace(owner, source, change.next.as_ref(), None)
    }

    fn replace<T: Serialize>(
        &self,
        owner: OwnerKey,
        source: Option<(OwnerKey, ObjectRef)>,
        next: Option<&T>,
        checked_size: Option<usize>,
    ) -> Result<Option<OwnerMutation>, NativeUdsError> {
        let prior = if self.prior.is_some() {
            let owners = super::NativeSourceOwnerKeys::new(vec![owner]).map_err(protocol)?;
            self.access.call(NativeIndexRequest::LookupProjectionOwners {
                selection: self.selection, projection: self.projection, owners,
            }, |response| {
                if let NativeIndexResponse::ProjectionOwners(result) = response
                    && matches!(self.selection, ViewSelection::Exact(view) if view == result.view)
                    && result.projection == self.projection && result.binding == self.prior
                    && let [row] = result.rows.as_slice()
                    && row.owner == owner
                {
                    Some(row.contribution)
                } else { None }
            })?
        } else {
            None
        };
        let next = next
            .map(|value| {
                let (payload, _memory) = encode_projection(value, checked_size, &self.access)?;
                let stamp = Digest(Sha256::digest(&payload).into());
                let children =
                    quanta_index_contract::native_protocol::NativeProjectionOutputChildren::new(
                        Vec::new(),
                    )
                    .map_err(protocol)?;
                let output = self
                    .access
                    .staged(NativeIndexRequest::StageProjectionOutput {
                        ingress: self.ingress,
                        bytes: payload,
                        children,
                    })?;
                self.access
                    .staged(NativeIndexRequest::StageProjectionOwner {
                        ingress: self.ingress,
                        contribution: ProjectionOwnerContribution {
                            projection: self.projection,
                            producer: NATIVE_DSL_PRODUCER_V1,
                            owner,
                            source_owner: source.map(|(owner, _)| owner),
                            source: source.map(|(_, reference)| reference),
                            dependency_stamp: stamp,
                            output: Some(output),
                        },
                    })
            })
            .transpose()?;
        Ok(
            (prior.is_some() || next.is_some()).then_some(OwnerMutation {
                owner,
                expected: prior,
                next,
            }),
        )
    }
}

fn publication_header_size(
    changes: &NativeDslChangesV1,
    budget: &RequestBudgetV1,
) -> Result<usize, NativeUdsError> {
    match changes {
        NativeDslChangesV1::Corpus { header, .. } => {
            projection_payload_size(header, MAX_PROJECTION_OBJECT_BYTES, budget)
        }
        NativeDslChangesV1::RuntimeMetadata(runtime) => projection_payload_size(
            runtime,
            NATIVE_DSL_RUNTIME_CATALOG_MAX_BYTES_V1 as usize,
            budget,
        ),
    }
}

fn projection_payload_size(
    value: &impl Serialize,
    limit: usize,
    budget: &RequestBudgetV1,
) -> Result<usize, NativeUdsError> {
    struct Count<'a> {
        bytes: usize,
        limit: usize,
        budget: &'a RequestBudgetV1,
        interruption: Option<NativeUdsError>,
    }
    impl std::io::Write for Count<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if let Err(error) = super::read_session::admitted_budget_checkpoint(self.budget) {
                self.interruption = Some(error);
                return Err(std::io::Error::other("Native DSL projection measurement interrupted"));
            }
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|length| *length <= self.limit)
                .ok_or_else(|| std::io::Error::other("Native DSL output exceeds 16 MiB"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    super::read_session::admitted_budget_checkpoint(budget)?;
    let mut size = Count {
        bytes: 0,
        limit,
        budget,
        interruption: None,
    };
    let encoded = ciborium::ser::into_writer(value, &mut size);
    if let Some(error) = size.interruption {
        return Err(error);
    }
    encoded.map_err(|error| NativeUdsError::Protocol(error.to_string()))?;
    super::read_session::admitted_budget_checkpoint(budget)?;
    Ok(size.bytes)
}

fn encode_projection(
    value: &impl Serialize,
    checked_size: Option<usize>,
    access: &DslAccess<'_, impl AsRef<Path>>,
) -> Result<(Vec<u8>, Option<MemoryLease>), NativeUdsError> {
    let size = match checked_size {
        Some(size) => size,
        None => projection_payload_size(value, MAX_PROJECTION_OBJECT_BYTES, access.budget)?,
    };
    let memory = access
        .reserve(u64::try_from(size).map_err(|_| protocol("Native DSL payload length"))?)?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(size)
        .map_err(|_| protocol("Native DSL payload allocation"))?;
    ciborium::ser::into_writer(value, &mut payload)
        .map_err(|error| NativeUdsError::Protocol(error.to_string()))?;
    Ok((payload, memory))
}

fn protocol(message: &str) -> NativeUdsError {
    NativeUdsError::Protocol(message.into())
}
