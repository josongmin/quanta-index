//! Admitted decode of the existing `RepoMap` control response wire.

use core::fmt;
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, Visitor};

use super::{
    SEARCH_PLANE_ADJACENT_TAG_FIELDS, SEARCH_PLANE_ENVELOPE_FIELDS, SearchPlaneControlIpcResponse,
    SearchPlaneControlIpcResponseEnvelope,
};
use crate::SourceIdentityDecodeAdmissionV1;

#[derive(Clone, Copy)]
enum FieldV3 {
    RequestId,
    Payload,
    Kind,
    Unknown,
}

struct FieldSeedV3;

impl<'de> DeserializeSeed<'de> for FieldSeedV3 {
    type Value = FieldV3;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for FieldSeedV3 {
    type Value = FieldV3;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a control response field")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(match value {
            "request_id" => FieldV3::RequestId,
            "payload" => FieldV3::Payload,
            "kind" => FieldV3::Kind,
            _ => FieldV3::Unknown,
        })
    }
}

struct KindSeedV3;

#[derive(Clone, Copy)]
enum RepoMapControlKindV3 {
    Receipt,
    Error,
}

impl<'de> DeserializeSeed<'de> for KindSeedV3 {
    type Value = RepoMapControlKindV3;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for KindSeedV3 {
    type Value = RepoMapControlKindV3;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMap control response kind")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        match value {
            "RepoMapTerminalReceiptV2" => Ok(RepoMapControlKindV3::Receipt),
            "Error" => Ok(RepoMapControlKindV3::Error),
            other => Err(E::unknown_variant(
                other,
                &["RepoMapTerminalReceiptV2", "Error"],
            )),
        }
    }
}

pub(super) struct RepoMapControlResponseEnvelopeSeedV3<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'a, A: SourceIdentityDecodeAdmissionV1 + ?Sized> RepoMapControlResponseEnvelopeSeedV3<'a, A> {
    pub(super) const fn new(admission: &'a A, max_text_bytes: usize) -> Self {
        Self {
            admission,
            max_text_bytes,
        }
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for RepoMapControlResponseEnvelopeSeedV3<'_, A>
{
    type Value = SearchPlaneControlIpcResponseEnvelope;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_struct(
            "SearchPlaneControlIpcResponseEnvelope",
            SEARCH_PLANE_ENVELOPE_FIELDS,
            self,
        )
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> Visitor<'de>
    for RepoMapControlResponseEnvelopeSeedV3<'_, A>
{
    type Value = SearchPlaneControlIpcResponseEnvelope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMap control response envelope")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut request_id = None;
        let mut payload = None;
        while let Some(field) = map.next_key_seed(FieldSeedV3)? {
            match field {
                FieldV3::RequestId if request_id.is_none() => request_id = Some(map.next_value()?),
                FieldV3::Payload if payload.is_none() => {
                    payload = Some(map.next_value_seed(PayloadSeedV3 {
                        admission: self.admission,
                        max_text_bytes: self.max_text_bytes,
                    })?);
                }
                FieldV3::RequestId => return Err(de::Error::duplicate_field("request_id")),
                FieldV3::Payload => return Err(de::Error::duplicate_field("payload")),
                FieldV3::Kind | FieldV3::Unknown => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        Ok(SearchPlaneControlIpcResponseEnvelope {
            request_id: request_id.ok_or_else(|| de::Error::missing_field("request_id"))?,
            payload: payload.ok_or_else(|| de::Error::missing_field("payload"))?,
        })
    }
}

struct PayloadSeedV3<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for PayloadSeedV3<'_, A>
{
    type Value = SearchPlaneControlIpcResponse;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_struct(
            "SearchPlaneControlIpcResponse",
            SEARCH_PLANE_ADJACENT_TAG_FIELDS,
            self,
        )
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> Visitor<'de> for PayloadSeedV3<'_, A> {
    type Value = SearchPlaneControlIpcResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMap control receipt or error")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut kind = None;
        let mut payload = None;
        while let Some(field) = map.next_key_seed(FieldSeedV3)? {
            match field {
                FieldV3::Kind if kind.is_none() => kind = Some(map.next_value_seed(KindSeedV3)?),
                FieldV3::Payload if payload.is_none() => {
                    payload = Some(match kind.ok_or_else(|| de::Error::custom("SearchPlaneControlIpcResponse payload preceded kind"))? {
                        RepoMapControlKindV3::Receipt => SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(
                            map.next_value_seed(crate::RepoMapTerminalReceiptV2::deserialize_seed_with_admission_v3(
                                self.admission,
                                self.max_text_bytes,
                            ))?,
                        ),
                        RepoMapControlKindV3::Error => SearchPlaneControlIpcResponse::Error(map.next_value_seed(
                            crate::ipc::error::admitted_auxiliary_error_seed_v1(
                                self.admission,
                                self.max_text_bytes,
                            ),
                        )?),
                    });
                }
                FieldV3::Kind => return Err(de::Error::duplicate_field("kind")),
                FieldV3::Payload => return Err(de::Error::duplicate_field("payload")),
                FieldV3::RequestId | FieldV3::Unknown => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        let _kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        payload.ok_or_else(|| de::Error::missing_field("payload"))
    }
}
