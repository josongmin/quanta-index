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
    StructuralIngestBatch,
    SourcePublicationUploadPart, SourcePublicationUploadCommit, SourcePublicationUploadIdentity,
    SourcePublicationUploadAck,
};
use crate::{RepoMapPublishBundleRequestV2, RepoMapTerminalReceiptV2};

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

const SEARCH_PLANE_INGEST_REQUEST_VARIANTS: &[&str] = &[
    "StageSourcePublication", "PublishStagedSourcePublication", "DiscardSourcePublicationUpload",
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
                "SearchPlaneIngestIpcRequest", 16, "StageSourcePublication", payload),
            Self::PublishStagedSourcePublication(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest", 17, "PublishStagedSourcePublication", payload),
            Self::DiscardSourcePublicationUpload(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest", 18, "DiscardSourcePublicationUpload", payload),
            Self::PublishSearchCorpusBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                0,
                "PublishSearchCorpusBatchV2",
                payload,
            ),
            Self::PublishHistoryBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                2,
                "PublishHistoryBatch",
                payload,
            ),
            Self::PublishRepoCommitRecencyBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                3,
                "PublishRepoCommitRecencyBatch",
                payload,
            ),
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
            Self::PublishFileContributorBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                11,
                "PublishFileContributorBatch",
                payload,
            ),
            Self::PublishDirtyBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                6,
                "PublishDirtyBatch",
                payload,
            ),
            Self::PublishRuntimeCatalogBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                7,
                "PublishRuntimeCatalogBatch",
                payload,
            ),
            Self::PublishStructuralBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                8,
                "PublishStructuralBatch",
                payload,
            ),
            Self::PublishRepoMapBundleV2(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                14,
                "PublishRepoMapBundleV2",
                payload,
            ),
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
            "StageSourcePublication" => Ok(SearchPlaneIngestIpcRequest::StageSourcePublication(variant.newtype_variant()?)),
            "PublishStagedSourcePublication" => Ok(SearchPlaneIngestIpcRequest::PublishStagedSourcePublication(variant.newtype_variant()?)),
            "DiscardSourcePublicationUpload" => Ok(SearchPlaneIngestIpcRequest::DiscardSourcePublicationUpload(variant.newtype_variant()?)),
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
                "SearchPlaneIngestIpcResponse", 15, "SourcePublicationUploadAck", payload),
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
            "SourcePublicationUploadAck" => Ok(SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(variant.newtype_variant()?)),
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
}

const SEARCH_PLANE_INGEST_REQUEST_ENVELOPE_FIELDS: &[&str] = &["request_id", "payload"];

impl Serialize for SearchPlaneIngestIpcRequestEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneIngestIpcRequestEnvelope", 2)?;
        state.serialize_field("request_id", &self.request_id)?;
        state.serialize_field("payload", &self.payload)?;
        state.end()
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
                        SEARCH_PLANE_INGEST_REQUEST_ENVELOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneIngestIpcRequestEnvelope {
            request_id: request_id.ok_or_else(|| de::Error::missing_field("request_id"))?,
            payload: payload.ok_or_else(|| de::Error::missing_field("payload"))?,
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
