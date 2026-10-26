//! The existing auxiliary ingest response IR decoded with admitted backings.

use core::fmt;

use quanta_index_contract_base::SourceIdentityDecodeAdmissionV1;
use serde::Deserializer;
use serde::de::{self, DeserializeSeed, EnumAccess, MapAccess, VariantAccess, Visitor};

use super::{
    SEARCH_PLANE_INGEST_RESPONSE_ENVELOPE_FIELDS, SEARCH_PLANE_INGEST_RESPONSE_VARIANTS,
    SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope,
};
use crate::ipc::error::admitted_auxiliary_error_seed_v1;
use crate::ipc::publish_receipt::admitted_auxiliary_receipt_seed_v1;

struct KnownFieldSeed;

impl<'de> DeserializeSeed<'de> for KnownFieldSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for KnownFieldSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a known ingest response field")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        SEARCH_PLANE_INGEST_RESPONSE_ENVELOPE_FIELDS
            .iter()
            .copied()
            .find(|field| *field == value)
            .ok_or_else(|| E::unknown_field(value, SEARCH_PLANE_INGEST_RESPONSE_ENVELOPE_FIELDS))
    }

    fn visit_string<E: de::Error>(self, _value: String) -> Result<Self::Value, E> {
        Err(E::custom("ingest response field must be borrowed"))
    }
}

struct AuxiliaryVariantSeed;

impl<'de> DeserializeSeed<'de> for AuxiliaryVariantSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for AuxiliaryVariantSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an auxiliary ingest response variant")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        match value {
            "HistoryReceipt" => Ok("HistoryReceipt"),
            "RepoCommitRecencyReceipt" => Ok("RepoCommitRecencyReceipt"),
            "FileContributorReceipt" => Ok("FileContributorReceipt"),
            "DirtyReceipt" => Ok("DirtyReceipt"),
            "StructuralReceipt" => Ok("StructuralReceipt"),
            "RepoMapTerminalReceiptV2" => Ok("RepoMapTerminalReceiptV2"),
            "Error" => Ok("Error"),
            other => Err(E::unknown_variant(
                other,
                SEARCH_PLANE_INGEST_RESPONSE_VARIANTS,
            )),
        }
    }

    fn visit_string<E: de::Error>(self, _value: String) -> Result<Self::Value, E> {
        Err(E::custom("auxiliary response variant must be borrowed"))
    }
}

pub(super) struct AuxiliaryResponseEnvelopeSeedV1<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'a, A: SourceIdentityDecodeAdmissionV1 + ?Sized> AuxiliaryResponseEnvelopeSeedV1<'a, A> {
    pub(super) const fn new(admission: &'a A, max_text_bytes: usize) -> Self {
        Self {
            admission,
            max_text_bytes,
        }
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AuxiliaryResponseEnvelopeSeedV1<'_, A>
{
    type Value = SearchPlaneIngestIpcResponseEnvelope;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_struct(
            "SearchPlaneIngestIpcResponseEnvelope",
            SEARCH_PLANE_INGEST_RESPONSE_ENVELOPE_FIELDS,
            self,
        )
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> Visitor<'de>
    for AuxiliaryResponseEnvelopeSeedV1<'_, A>
{
    type Value = SearchPlaneIngestIpcResponseEnvelope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an admitted auxiliary ingest response envelope")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut request_id: Option<u64> = None;
        let mut payload: Option<SearchPlaneIngestIpcResponse> = None;
        while let Some(key) = map.next_key_seed(KnownFieldSeed)? {
            match key {
                "request_id" if request_id.is_none() => request_id = Some(map.next_value()?),
                "payload" if payload.is_none() => {
                    payload = Some(map.next_value_seed(AuxiliaryPayloadSeedV1 {
                        admission: self.admission,
                        max_text_bytes: self.max_text_bytes,
                    })?);
                }
                duplicate => return Err(de::Error::duplicate_field(duplicate)),
            }
        }
        Ok(SearchPlaneIngestIpcResponseEnvelope {
            request_id: request_id.ok_or_else(|| de::Error::missing_field("request_id"))?,
            payload: payload.ok_or_else(|| de::Error::missing_field("payload"))?,
        })
    }
}

struct AuxiliaryPayloadSeedV1<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AuxiliaryPayloadSeedV1<'_, A>
{
    type Value = SearchPlaneIngestIpcResponse;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_enum(
            "SearchPlaneIngestIpcResponse",
            SEARCH_PLANE_INGEST_RESPONSE_VARIANTS,
            self,
        )
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> Visitor<'de>
    for AuxiliaryPayloadSeedV1<'_, A>
{
    type Value = SearchPlaneIngestIpcResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an auxiliary ingest receipt or error")
    }

    fn visit_enum<E: EnumAccess<'de>>(self, data: E) -> Result<Self::Value, E::Error> {
        let (tag, variant) = data.variant_seed(AuxiliaryVariantSeed)?;
        let receipt = || admitted_auxiliary_receipt_seed_v1(self.admission, self.max_text_bytes);
        match tag {
            "HistoryReceipt" => variant
                .newtype_variant_seed(receipt())
                .map(SearchPlaneIngestIpcResponse::HistoryReceipt),
            "RepoCommitRecencyReceipt" => variant
                .newtype_variant_seed(receipt())
                .map(SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt),
            "FileContributorReceipt" => variant
                .newtype_variant_seed(receipt())
                .map(SearchPlaneIngestIpcResponse::FileContributorReceipt),
            "DirtyReceipt" => variant
                .newtype_variant_seed(receipt())
                .map(SearchPlaneIngestIpcResponse::DirtyReceipt),
            "StructuralReceipt" => variant
                .newtype_variant_seed(receipt())
                .map(SearchPlaneIngestIpcResponse::StructuralReceipt),
            "RepoMapTerminalReceiptV2" => variant
                .newtype_variant_seed(
                    crate::RepoMapTerminalReceiptV2::deserialize_seed_with_admission_v3(
                        self.admission,
                        self.max_text_bytes,
                    ),
                )
                .map(SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2),
            "Error" => variant
                .newtype_variant_seed(admitted_auxiliary_error_seed_v1(
                    self.admission,
                    self.max_text_bytes,
                ))
                .map(SearchPlaneIngestIpcResponse::Error),
            other => Err(de::Error::unknown_variant(
                other,
                SEARCH_PLANE_INGEST_RESPONSE_VARIANTS,
            )),
        }
    }
}
