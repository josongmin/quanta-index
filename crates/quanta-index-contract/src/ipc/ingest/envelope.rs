//! Top-level ingest request and response wire envelopes.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, VariantAccess, Visitor},
    ser::SerializeStruct,
};

use super::super::{BatchPublishReceipt, SearchPlaneIpcError};
use super::{
    DirtyIngestBatch, FileContributorIngestBatch, FileOwnershipIngestBatch, HistoryIngestBatch,
    RepoCommitRecencyIngestBatch, RepoDescriptionIngestBatch, RepoMetaIngestBatch,
    RepoTopicIngestBatch, RuntimeCatalogIngestBatch, SearchCorpusIngestBatch,
    SourcePublicationUploadAck, SourcePublicationUploadCommit, SourcePublicationUploadIdentity,
    SourcePublicationUploadPart, StructuralIngestBatch,
};
use crate::{DiagnosticAttemptFieldV1, RepoMapPublishBundleRequestV2, RepoMapTerminalReceiptV2};

// Batch-dependent binding helpers live beside the request DTO so transient
// response observations do not depend back on the ingest request module.
impl super::super::SourcePublicationBinding {
    #[must_use]
    pub fn for_batch(batch: &SearchCorpusIngestBatch) -> Self {
        Self {
            event: batch.source_event.clone(),
            target: crate::GenerationSnapshot {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                track: crate::SearchPlaneTrackKind::Lexical,
                manifest_generation: batch.generation,
                manifest_digest: batch.manifest_digest.clone(),
            },
            batch_digest: batch.batch_digest.clone(),
        }
    }
}

impl super::super::SearchCorpusIngestObservation {
    pub fn validate_for(
        &self,
        request_id: u64,
        batch: &SearchCorpusIngestBatch,
        publication: &super::super::SourcePublicationBinding,
        receipt: &BatchPublishReceipt,
    ) -> Result<(), String> {
        self.validate_identity(
            request_id,
            &super::super::SourcePublicationBinding::for_batch(batch),
            batch.seal,
            publication,
            receipt,
        )
    }
}

// =============================================================================
// Top-level request / response enums
// =============================================================================

/// Typed ingest request payload sent over `ingest.sock`.
#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaneIngestIpcRequest {
    StageSourcePublication(SourcePublicationUploadPart),
    PublishStagedSourcePublication(SourcePublicationUploadCommit),
    DiscardSourcePublicationUpload(SourcePublicationUploadIdentity),
    PublishSearchCorpusBatch(SearchCorpusIngestBatch),
    PublishHistoryBatch(HistoryIngestBatch),
    PublishRepoCommitRecencyBatch(RepoCommitRecencyIngestBatch),
    PublishRepoTopicBatch(RepoTopicIngestBatch),
    PublishFileOwnershipBatch(FileOwnershipIngestBatch),
    PublishFileContributorBatch(FileContributorIngestBatch),
    PublishDirtyBatch(DirtyIngestBatch),
    PublishRuntimeCatalogBatch(RuntimeCatalogIngestBatch),
    PublishStructuralBatch(StructuralIngestBatch),
    PublishRepoMapBundleV2(RepoMapPublishBundleRequestV2),
    PublishRepoMetaBatch(RepoMetaIngestBatch),
    PublishRepoDescriptionBatch(RepoDescriptionIngestBatch),
}

/// Read-only view of the five Required auxiliary publish routes. The payload remains
/// owned by the caller until the synchronous transport send returns.
#[derive(Clone, Copy, Debug)]
pub enum BorrowedAuxiliaryIngestIpcRequestV1<'a> {
    History(&'a HistoryIngestBatch),
    RepoCommitRecency(&'a RepoCommitRecencyIngestBatch),
    FileContributor(&'a FileContributorIngestBatch),
    Dirty(&'a DirtyIngestBatch),
    Structural(&'a StructuralIngestBatch),
    RepoMap(crate::BorrowedRepoMapPublishBundleRequestV3<'a>),
}

fn serialize_auxiliary_publish_variant_v1<S: Serializer, P: Serialize>(
    serializer: S,
    variant: u32,
    name: &'static str,
    payload: &P,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_newtype_variant("SearchPlaneIngestIpcRequest", variant, name, payload)
}

impl Serialize for BorrowedAuxiliaryIngestIpcRequestV1<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::History(payload) => serialize_auxiliary_publish_variant_v1(
                serializer,
                2,
                "PublishHistoryBatch",
                payload,
            ),
            Self::RepoCommitRecency(payload) => serialize_auxiliary_publish_variant_v1(
                serializer,
                3,
                "PublishRepoCommitRecencyBatch",
                payload,
            ),
            Self::FileContributor(payload) => serialize_auxiliary_publish_variant_v1(
                serializer,
                11,
                "PublishFileContributorBatch",
                payload,
            ),
            Self::Dirty(payload) => {
                serialize_auxiliary_publish_variant_v1(serializer, 6, "PublishDirtyBatch", payload)
            }
            Self::Structural(payload) => serialize_auxiliary_publish_variant_v1(
                serializer,
                8,
                "PublishStructuralBatch",
                payload,
            ),
            Self::RepoMap(payload) => serialize_auxiliary_publish_variant_v1(
                serializer,
                14,
                "PublishRepoMapBundleV2",
                payload,
            ),
        }
    }
}

const SEARCH_PLANE_INGEST_REQUEST_VARIANTS: &[&str] = &[
    "StageSourcePublication",
    "PublishStagedSourcePublication",
    "DiscardSourcePublicationUpload",
    "PublishSearchCorpusBatchV2",
    "PublishHistoryBatch",
    "PublishRepoCommitRecencyBatch",
    "PublishRepoTopicBatch",
    "PublishFileOwnershipBatch",
    "PublishFileContributorBatch",
    "PublishDirtyBatch",
    "PublishRuntimeCatalogBatch",
    "PublishStructuralBatch",
    "PublishRepoMapBundleV2",
    "PublishRepoMetaBatch",
    "PublishRepoDescriptionBatch",
];

impl Serialize for SearchPlaneIngestIpcRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::StageSourcePublication(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                16,
                "StageSourcePublication",
                payload,
            ),
            Self::PublishStagedSourcePublication(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                17,
                "PublishStagedSourcePublication",
                payload,
            ),
            Self::DiscardSourcePublicationUpload(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                18,
                "DiscardSourcePublicationUpload",
                payload,
            ),
            Self::PublishSearchCorpusBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                0,
                "PublishSearchCorpusBatchV2",
                payload,
            ),
            Self::PublishHistoryBatch(payload) => {
                BorrowedAuxiliaryIngestIpcRequestV1::History(payload).serialize(serializer)
            }
            Self::PublishRepoCommitRecencyBatch(payload) => {
                BorrowedAuxiliaryIngestIpcRequestV1::RepoCommitRecency(payload)
                    .serialize(serializer)
            }
            Self::PublishRepoTopicBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                4,
                "PublishRepoTopicBatch",
                payload,
            ),
            Self::PublishFileOwnershipBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                5,
                "PublishFileOwnershipBatch",
                payload,
            ),
            Self::PublishFileContributorBatch(payload) => {
                BorrowedAuxiliaryIngestIpcRequestV1::FileContributor(payload).serialize(serializer)
            }
            Self::PublishDirtyBatch(payload) => {
                BorrowedAuxiliaryIngestIpcRequestV1::Dirty(payload).serialize(serializer)
            }
            Self::PublishRuntimeCatalogBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                7,
                "PublishRuntimeCatalogBatch",
                payload,
            ),
            Self::PublishStructuralBatch(payload) => {
                BorrowedAuxiliaryIngestIpcRequestV1::Structural(payload).serialize(serializer)
            }
            Self::PublishRepoMapBundleV2(payload) => BorrowedAuxiliaryIngestIpcRequestV1::RepoMap(
                crate::BorrowedRepoMapPublishBundleRequestV3 {
                    source_bundle_digest: &payload.source_bundle_digest,
                    bundle: &payload.bundle,
                },
            )
            .serialize(serializer),
            Self::PublishRepoMetaBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                10,
                "PublishRepoMetaBatch",
                payload,
            ),
            Self::PublishRepoDescriptionBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                12,
                "PublishRepoDescriptionBatch",
                payload,
            ),
        }
    }
}

struct SearchPlaneIngestIpcRequestVisitor;

impl<'de> Visitor<'de> for SearchPlaneIngestIpcRequestVisitor {
    type Value = SearchPlaneIngestIpcRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneIngestIpcRequest enum")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::EnumAccess<'de>,
    {
        let (tag, variant) = data.variant::<String>()?;
        match tag.as_str() {
            "StageSourcePublication" => Ok(SearchPlaneIngestIpcRequest::StageSourcePublication(
                variant.newtype_variant()?,
            )),
            "PublishStagedSourcePublication" => {
                Ok(SearchPlaneIngestIpcRequest::PublishStagedSourcePublication(
                    variant.newtype_variant()?,
                ))
            }
            "DiscardSourcePublicationUpload" => {
                Ok(SearchPlaneIngestIpcRequest::DiscardSourcePublicationUpload(
                    variant.newtype_variant()?,
                ))
            }
            "PublishSearchCorpusBatchV2" => Ok(
                SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(variant.newtype_variant()?),
            ),
            "PublishHistoryBatch" => Ok(SearchPlaneIngestIpcRequest::PublishHistoryBatch(
                variant.newtype_variant()?,
            )),
            "PublishRepoCommitRecencyBatch" => {
                Ok(SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(
                    variant.newtype_variant()?,
                ))
            }
            "PublishRepoTopicBatch" => Ok(SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(
                variant.newtype_variant()?,
            )),
            "PublishFileOwnershipBatch" => Ok(
                SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(variant.newtype_variant()?),
            ),
            "PublishFileContributorBatch" => {
                Ok(SearchPlaneIngestIpcRequest::PublishFileContributorBatch(
                    variant.newtype_variant()?,
                ))
            }
            "PublishDirtyBatch" => Ok(SearchPlaneIngestIpcRequest::PublishDirtyBatch(
                variant.newtype_variant()?,
            )),
            "PublishRuntimeCatalogBatch" => Ok(
                SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(variant.newtype_variant()?),
            ),
            "PublishStructuralBatch" => Ok(SearchPlaneIngestIpcRequest::PublishStructuralBatch(
                variant.newtype_variant()?,
            )),
            "PublishRepoMapBundleV2" => Ok(SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(
                variant.newtype_variant()?,
            )),
            "PublishRepoMetaBatch" => Ok(SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(
                variant.newtype_variant()?,
            )),
            "PublishRepoDescriptionBatch" => {
                Ok(SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(
                    variant.newtype_variant()?,
                ))
            }
            other => Err(de::Error::unknown_variant(
                other,
                SEARCH_PLANE_INGEST_REQUEST_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for SearchPlaneIngestIpcRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_enum(
            "SearchPlaneIngestIpcRequest",
            SEARCH_PLANE_INGEST_REQUEST_VARIANTS,
            SearchPlaneIngestIpcRequestVisitor,
        )
    }
}

/// Typed ingest response payload returned by `ingest.sock`.
#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "wire payloads retain the existing by-value API; boxing adds allocation and changes construction across unrelated ingest routes"
)]
pub enum SearchPlaneIngestIpcResponse {
    SourcePublicationUploadAck(SourcePublicationUploadAck),
    SearchCorpusReceipt(super::super::SearchCorpusPublishOutcome),
    HistoryReceipt(BatchPublishReceipt),
    RepoCommitRecencyReceipt(BatchPublishReceipt),
    RepoTopicReceipt(BatchPublishReceipt),
    FileOwnershipReceipt(BatchPublishReceipt),
    FileContributorReceipt(BatchPublishReceipt),
    DirtyReceipt(BatchPublishReceipt),
    RuntimeCatalogReceipt(BatchPublishReceipt),
    StructuralReceipt(BatchPublishReceipt),
    RepoMapTerminalReceiptV2(RepoMapTerminalReceiptV2),
    RepoMetaReceipt(BatchPublishReceipt),
    RepoDescriptionReceipt(BatchPublishReceipt),
    Error(SearchPlaneIpcError),
}

const SEARCH_PLANE_INGEST_RESPONSE_VARIANTS: &[&str] = &[
    "SourcePublicationUploadAck",
    "SearchCorpusReceiptV2",
    "HistoryReceipt",
    "RepoCommitRecencyReceipt",
    "RepoTopicReceipt",
    "FileOwnershipReceipt",
    "FileContributorReceipt",
    "DirtyReceipt",
    "RuntimeCatalogReceipt",
    "StructuralReceipt",
    "RepoMapTerminalReceiptV2",
    "RepoMetaReceipt",
    "RepoDescriptionReceipt",
    "Error",
];

impl Serialize for SearchPlaneIngestIpcResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::SourcePublicationUploadAck(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                15,
                "SourcePublicationUploadAck",
                payload,
            ),
            Self::SearchCorpusReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                0,
                "SearchCorpusReceiptV2",
                payload,
            ),
            Self::HistoryReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                2,
                "HistoryReceipt",
                payload,
            ),
            Self::RepoCommitRecencyReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                3,
                "RepoCommitRecencyReceipt",
                payload,
            ),
            Self::RepoTopicReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                4,
                "RepoTopicReceipt",
                payload,
            ),
            Self::FileOwnershipReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                5,
                "FileOwnershipReceipt",
                payload,
            ),
            Self::FileContributorReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                12,
                "FileContributorReceipt",
                payload,
            ),
            Self::DirtyReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                6,
                "DirtyReceipt",
                payload,
            ),
            Self::RuntimeCatalogReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                7,
                "RuntimeCatalogReceipt",
                payload,
            ),
            Self::StructuralReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                8,
                "StructuralReceipt",
                payload,
            ),
            Self::RepoMapTerminalReceiptV2(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                14,
                "RepoMapTerminalReceiptV2",
                payload,
            ),
            Self::RepoMetaReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                11,
                "RepoMetaReceipt",
                payload,
            ),
            Self::RepoDescriptionReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                13,
                "RepoDescriptionReceipt",
                payload,
            ),
            Self::Error(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                10,
                "Error",
                payload,
            ),
        }
    }
}

struct SearchPlaneIngestIpcResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneIngestIpcResponseVisitor {
    type Value = SearchPlaneIngestIpcResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneIngestIpcResponse enum")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::EnumAccess<'de>,
    {
        let (tag, variant) = data.variant::<String>()?;
        match tag.as_str() {
            "SourcePublicationUploadAck" => {
                Ok(SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(
                    variant.newtype_variant()?,
                ))
            }
            "SearchCorpusReceiptV2" => Ok(SearchPlaneIngestIpcResponse::SearchCorpusReceipt(
                variant.newtype_variant()?,
            )),
            "HistoryReceipt" => Ok(SearchPlaneIngestIpcResponse::HistoryReceipt(
                variant.newtype_variant()?,
            )),
            "RepoCommitRecencyReceipt" => Ok(
                SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(variant.newtype_variant()?),
            ),
            "RepoTopicReceipt" => Ok(SearchPlaneIngestIpcResponse::RepoTopicReceipt(
                variant.newtype_variant()?,
            )),
            "FileOwnershipReceipt" => Ok(SearchPlaneIngestIpcResponse::FileOwnershipReceipt(
                variant.newtype_variant()?,
            )),
            "FileContributorReceipt" => Ok(SearchPlaneIngestIpcResponse::FileContributorReceipt(
                variant.newtype_variant()?,
            )),
            "DirtyReceipt" => Ok(SearchPlaneIngestIpcResponse::DirtyReceipt(
                variant.newtype_variant()?,
            )),
            "RuntimeCatalogReceipt" => Ok(SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(
                variant.newtype_variant()?,
            )),
            "StructuralReceipt" => Ok(SearchPlaneIngestIpcResponse::StructuralReceipt(
                variant.newtype_variant()?,
            )),
            "RepoMapTerminalReceiptV2" => Ok(
                SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(variant.newtype_variant()?),
            ),
            "RepoMetaReceipt" => Ok(SearchPlaneIngestIpcResponse::RepoMetaReceipt(
                variant.newtype_variant()?,
            )),
            "RepoDescriptionReceipt" => Ok(SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(
                variant.newtype_variant()?,
            )),
            "Error" => Ok(SearchPlaneIngestIpcResponse::Error(
                variant.newtype_variant()?,
            )),
            other => Err(de::Error::unknown_variant(
                other,
                SEARCH_PLANE_INGEST_RESPONSE_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for SearchPlaneIngestIpcResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_enum(
            "SearchPlaneIngestIpcResponse",
            SEARCH_PLANE_INGEST_RESPONSE_VARIANTS,
            SearchPlaneIngestIpcResponseVisitor,
        )
    }
}

// =============================================================================
// Envelopes
// =============================================================================

/// Ingest request envelope (`request_id` + `payload`). Same shape as the
/// existing query / control envelopes in `split.rs` so the same
/// `quanta-index-ipc` UDS server / client machinery carries it.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneIngestIpcRequestEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneIngestIpcRequest,
    pub diagnostic_attempt: DiagnosticAttemptFieldV1,
}

/// Borrowed form of the canonical ingest envelope for History routes.
#[derive(Clone, Copy, Debug)]
pub struct BorrowedAuxiliaryIngestIpcRequestEnvelopeV1<'a> {
    pub request_id: u64,
    pub payload: BorrowedAuxiliaryIngestIpcRequestV1<'a>,
    pub diagnostic_attempt: DiagnosticAttemptFieldV1,
}

const SEARCH_PLANE_INGEST_REQUEST_ENVELOPE_FIELDS: &[&str] =
    &["request_id", "payload", "diagnostic_attempt"];

fn serialize_ingest_request_envelope_v1<S: Serializer, P: Serialize>(
    serializer: S,
    request_id: u64,
    payload: &P,
    diagnostic_attempt: &DiagnosticAttemptFieldV1,
) -> Result<S::Ok, S::Error> {
    let token = diagnostic_attempt.token();
    let mut state = serializer.serialize_struct(
        "SearchPlaneIngestIpcRequestEnvelope",
        if token.is_some() { 3 } else { 2 },
    )?;
    state.serialize_field("request_id", &request_id)?;
    state.serialize_field("payload", payload)?;
    if let Some(token) = token {
        state.serialize_field("diagnostic_attempt", &token)?;
    }
    state.end()
}

impl Serialize for SearchPlaneIngestIpcRequestEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_ingest_request_envelope_v1(
            serializer,
            self.request_id,
            &self.payload,
            &self.diagnostic_attempt,
        )
    }
}

impl Serialize for BorrowedAuxiliaryIngestIpcRequestEnvelopeV1<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_ingest_request_envelope_v1(
            serializer,
            self.request_id,
            &self.payload,
            &self.diagnostic_attempt,
        )
    }
}

struct SearchPlaneIngestIpcRequestEnvelopeVisitor;

impl<'de> Visitor<'de> for SearchPlaneIngestIpcRequestEnvelopeVisitor {
    type Value = SearchPlaneIngestIpcRequestEnvelope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneIngestIpcRequestEnvelope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut request_id: Option<u64> = None;
        let mut payload: Option<SearchPlaneIngestIpcRequest> = None;
        let mut diagnostic_attempt = DiagnosticAttemptFieldV1::Absent;
        let mut attempt_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "request_id" => {
                    if request_id.is_some() {
                        return Err(de::Error::duplicate_field("request_id"));
                    }
                    request_id = Some(map.next_value()?);
                }
                "payload" => {
                    if payload.is_some() {
                        return Err(de::Error::duplicate_field("payload"));
                    }
                    payload = Some(map.next_value()?);
                }
                "diagnostic_attempt" => {
                    let decoded: super::super::diagnostic_context::TolerantDiagnosticAttemptV1 =
                        map.next_value()?;
                    if attempt_seen {
                        diagnostic_attempt = DiagnosticAttemptFieldV1::Duplicate;
                    } else {
                        diagnostic_attempt = decoded.0;
                        attempt_seen = true;
                    }
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_INGEST_REQUEST_ENVELOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneIngestIpcRequestEnvelope {
            request_id: request_id.ok_or_else(|| de::Error::missing_field("request_id"))?,
            payload: payload.ok_or_else(|| de::Error::missing_field("payload"))?,
            diagnostic_attempt,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneIngestIpcRequestEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneIngestIpcRequestEnvelope",
            SEARCH_PLANE_INGEST_REQUEST_ENVELOPE_FIELDS,
            SearchPlaneIngestIpcRequestEnvelopeVisitor,
        )
    }
}

/// Ingest response envelope (`request_id` echoed + `payload`).
#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneIngestIpcResponseEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneIngestIpcResponse,
}

const SEARCH_PLANE_INGEST_RESPONSE_ENVELOPE_FIELDS: &[&str] = &["request_id", "payload"];

impl Serialize for SearchPlaneIngestIpcResponseEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneIngestIpcResponseEnvelope", 2)?;
        state.serialize_field("request_id", &self.request_id)?;
        state.serialize_field("payload", &self.payload)?;
        state.end()
    }
}

struct SearchPlaneIngestIpcResponseEnvelopeVisitor;

impl<'de> Visitor<'de> for SearchPlaneIngestIpcResponseEnvelopeVisitor {
    type Value = SearchPlaneIngestIpcResponseEnvelope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneIngestIpcResponseEnvelope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut request_id: Option<u64> = None;
        let mut payload: Option<SearchPlaneIngestIpcResponse> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "request_id" => {
                    if request_id.is_some() {
                        return Err(de::Error::duplicate_field("request_id"));
                    }
                    request_id = Some(map.next_value()?);
                }
                "payload" => {
                    if payload.is_some() {
                        return Err(de::Error::duplicate_field("payload"));
                    }
                    payload = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_INGEST_RESPONSE_ENVELOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneIngestIpcResponseEnvelope {
            request_id: request_id.ok_or_else(|| de::Error::missing_field("request_id"))?,
            payload: payload.ok_or_else(|| de::Error::missing_field("payload"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneIngestIpcResponseEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneIngestIpcResponseEnvelope",
            SEARCH_PLANE_INGEST_RESPONSE_ENVELOPE_FIELDS,
            SearchPlaneIngestIpcResponseEnvelopeVisitor,
        )
    }
}

/// Decode the five auxiliary receipt variants and the canonical ingest
/// refusal under the caller's retained parser/visitor allocation owner.
pub fn admitted_auxiliary_response_seed_v1<
    'a,
    A: crate::SourceIdentityDecodeAdmissionV1 + ?Sized,
>(
    admission: &'a A,
    max_text_bytes: usize,
) -> impl for<'de> serde::de::DeserializeSeed<'de, Value = SearchPlaneIngestIpcResponseEnvelope> + 'a
{
    admitted_auxiliary_response::AuxiliaryResponseEnvelopeSeedV1::new(admission, max_text_bytes)
}

#[path = "envelope/admitted_auxiliary_response.rs"]
mod admitted_auxiliary_response;

#[cfg(test)]
mod borrowed_auxiliary_wire_tests {
    use super::*;
    use crate::{ManifestGeneration, RepoId, RevisionId};

    fn same_canonical_wire_v1(
        owned: SearchPlaneIngestIpcRequest,
        borrowed: BorrowedAuxiliaryIngestIpcRequestV1<'_>,
        variant: &str,
    ) {
        let owned = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 41,
            payload: owned,
            diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
        };
        let borrowed = BorrowedAuxiliaryIngestIpcRequestEnvelopeV1 {
            request_id: 41,
            payload: borrowed,
            diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
        };
        let owned_json = serde_json::to_value(&owned).expect("owned JSON");
        let borrowed_json = serde_json::to_value(borrowed).expect("borrowed JSON");
        assert_eq!(owned_json, borrowed_json);
        assert!(
            owned_json
                .get("payload")
                .and_then(|payload| payload.get(variant))
                .is_some()
        );
        let mut owned_cbor = Vec::new();
        let mut borrowed_cbor = Vec::new();
        ciborium::into_writer(&owned, &mut owned_cbor).expect("owned CBOR");
        ciborium::into_writer(&borrowed, &mut borrowed_cbor).expect("borrowed CBOR");
        assert_eq!(owned_cbor, borrowed_cbor);
    }

    #[test]
    fn borrowed_auxiliary_envelopes_match_owned_tags_and_bytes_v1() {
        let repo_id = RepoId::new("repo-1").expect("repo ID");
        let revision_id = RevisionId::new("rev-1").expect("revision ID");
        let generation = ManifestGeneration::new(7);
        let history = HistoryIngestBatch {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            generation,
            manifest_digest: None,
            batch_digest: "digest:history".to_string(),
            commits: Vec::new(),
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: Vec::new(),
        };
        let recency = RepoCommitRecencyIngestBatch {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            generation,
            batch_digest: "digest:recency".to_string(),
            entries: Vec::new(),
        };
        let contributor = FileContributorIngestBatch {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            generation,
            batch_digest: "digest:contributor".to_string(),
            entries: Vec::new(),
        };
        same_canonical_wire_v1(
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(history.clone()),
            BorrowedAuxiliaryIngestIpcRequestV1::History(&history),
            "PublishHistoryBatch",
        );
        same_canonical_wire_v1(
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(recency.clone()),
            BorrowedAuxiliaryIngestIpcRequestV1::RepoCommitRecency(&recency),
            "PublishRepoCommitRecencyBatch",
        );
        same_canonical_wire_v1(
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(contributor.clone()),
            BorrowedAuxiliaryIngestIpcRequestV1::FileContributor(&contributor),
            "PublishFileContributorBatch",
        );
        let dirty = DirtyIngestBatch {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            generation,
            overlay_epoch_ms: 17,
            batch_digest: "digest:dirty".to_string(),
            entries: Vec::new(),
        };
        same_canonical_wire_v1(
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(dirty.clone()),
            BorrowedAuxiliaryIngestIpcRequestV1::Dirty(&dirty),
            "PublishDirtyBatch",
        );
        let structural = StructuralIngestBatch {
            repo_id,
            revision_id,
            generation,
            base_generation: None,
            manifest_digest: "manifest:structural".to_string(),
            batch_digest: "digest:structural".to_string(),
            mode: crate::BatchIngestMode::ReplaceGeneration,
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            seal: true,
        };
        same_canonical_wire_v1(
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(structural.clone()),
            BorrowedAuxiliaryIngestIpcRequestV1::Structural(&structural),
            "PublishStructuralBatch",
        );
    }
}
