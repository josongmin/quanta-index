//! Original-owned decode of an auxiliary ingest refusal.

use core::fmt;

use quanta_index_contract_base::{
    AdmittedOptionalSeedV1, AdmittedSequenceSeedV1, AdmittedTextSeedV1, DecodeBackingAdmissionV1,
};
use serde::Deserializer;
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};

use super::{
    QUERY_ERROR_REPAIR_FIELDS, QueryErrorRepair, RepairClass, SEARCH_PLANE_IPC_ERROR_FIELDS,
    SearchPlaneErrorCodeV2, SearchPlaneIpcError,
};

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
        formatter.write_str("a known search-plane error field")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        SEARCH_PLANE_IPC_ERROR_FIELDS
            .iter()
            .copied()
            .find(|field| *field == value)
            .ok_or_else(|| E::unknown_field(value, SEARCH_PLANE_IPC_ERROR_FIELDS))
    }

    fn visit_string<E: de::Error>(self, _value: String) -> Result<Self::Value, E> {
        Err(E::custom("search-plane error field must be borrowed"))
    }
}

struct RepairFieldSeed;

impl<'de> DeserializeSeed<'de> for RepairFieldSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for RepairFieldSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a known repair field")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        QUERY_ERROR_REPAIR_FIELDS
            .iter()
            .copied()
            .find(|field| *field == value)
            .ok_or_else(|| E::unknown_field(value, QUERY_ERROR_REPAIR_FIELDS))
    }
}

struct RepairSeed<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de> for RepairSeed<'_, A> {
    type Value = QueryErrorRepair;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_struct("QueryErrorRepair", QUERY_ERROR_REPAIR_FIELDS, self)
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> Visitor<'de> for RepairSeed<'_, A> {
    type Value = QueryErrorRepair;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an admitted query repair")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut class: Option<RepairClass> = None;
        let mut supported_alternatives = None;
        let mut docs_anchor = None;
        let mut docs_anchor_seen = false;
        while let Some(key) = map.next_key_seed(RepairFieldSeed)? {
            match key {
                "class" if class.is_none() => class = Some(map.next_value()?),
                "supported_alternatives" if supported_alternatives.is_none() => {
                    supported_alternatives = Some(map.next_value_seed(
                        AdmittedSequenceSeedV1::new(self.admission, self.max_text_bytes, || {
                            AdmittedTextSeedV1::new(self.admission, self.max_text_bytes)
                        }),
                    )?);
                }
                "docs_anchor" if !docs_anchor_seen => {
                    docs_anchor_seen = true;
                    docs_anchor = map.next_value_seed(AdmittedOptionalSeedV1::new(
                        self.admission,
                        AdmittedTextSeedV1::new(self.admission, self.max_text_bytes),
                    ))?;
                }
                duplicate => return Err(de::Error::duplicate_field(duplicate)),
            }
        }
        Ok(QueryErrorRepair {
            class: class.ok_or_else(|| de::Error::missing_field("class"))?,
            supported_alternatives: supported_alternatives
                .ok_or_else(|| de::Error::missing_field("supported_alternatives"))?,
            docs_anchor,
        })
    }
}

pub(super) struct AuxiliaryErrorSeedV1<'a, A: ?Sized> {
    admission: &'a A,
    max_message_bytes: usize,
}

impl<'a, A: DecodeBackingAdmissionV1 + ?Sized> AuxiliaryErrorSeedV1<'a, A> {
    pub(super) const fn new(admission: &'a A, max_message_bytes: usize) -> Self {
        Self {
            admission,
            max_message_bytes,
        }
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AuxiliaryErrorSeedV1<'_, A>
{
    type Value = SearchPlaneIpcError;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_struct("SearchPlaneIpcError", SEARCH_PLANE_IPC_ERROR_FIELDS, self)
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> Visitor<'de> for AuxiliaryErrorSeedV1<'_, A> {
    type Value = SearchPlaneIpcError;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an admitted auxiliary ingest error")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut code: Option<SearchPlaneErrorCodeV2> = None;
        let mut message: Option<String> = None;
        let mut repair: Option<Option<QueryErrorRepair>> = None;
        while let Some(key) = map.next_key_seed(KnownFieldSeed)? {
            match key {
                "code" if code.is_none() => code = Some(map.next_value()?),
                "message" if message.is_none() => {
                    message = Some(map.next_value_seed(AdmittedTextSeedV1::new(
                        self.admission,
                        self.max_message_bytes,
                    ))?);
                }
                "repair" if repair.is_none() => {
                    repair = Some(map.next_value_seed(AdmittedOptionalSeedV1::new(
                        self.admission,
                        RepairSeed {
                            admission: self.admission,
                            max_text_bytes: self.max_message_bytes,
                        },
                    ))?);
                }
                duplicate => return Err(de::Error::duplicate_field(duplicate)),
            }
        }
        Ok(SearchPlaneIpcError {
            code: code.ok_or_else(|| de::Error::missing_field("code"))?,
            message: message.ok_or_else(|| de::Error::missing_field("message"))?,
            repair: repair.ok_or_else(|| de::Error::missing_field("repair"))?,
        })
    }
}
