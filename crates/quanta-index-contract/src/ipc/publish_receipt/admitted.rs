//! Original-owned decode of the canonical auxiliary publish receipt.

use core::fmt;

use quanta_index_contract_base::{
    AdmittedOptionalSeedV1, AdmittedTextSeedV1, DecodeBackingAdmissionV1,
};
use serde::Deserializer;
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};

use super::{BATCH_PUBLISH_RECEIPT_FIELDS, BatchPublishReceipt};
use crate::{ManifestGeneration, SemanticContentRootsV1};

struct KnownFieldSeed;
struct RootsFieldSeed;

impl<'de> DeserializeSeed<'de> for KnownFieldSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for KnownFieldSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a known publish receipt field")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        BATCH_PUBLISH_RECEIPT_FIELDS
            .iter()
            .copied()
            .find(|field| *field == value)
            .ok_or_else(|| E::unknown_field(value, BATCH_PUBLISH_RECEIPT_FIELDS))
    }

    fn visit_string<E: de::Error>(self, _value: String) -> Result<Self::Value, E> {
        Err(E::custom("publish receipt field must be borrowed"))
    }
}

impl<'de> DeserializeSeed<'de> for RootsFieldSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for RootsFieldSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a known semantic roots field")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        ["row_root_digest", "membership_root_digest"]
            .into_iter()
            .find(|field| *field == value)
            .ok_or_else(|| E::unknown_field(value, &["row_root_digest", "membership_root_digest"]))
    }
}

struct SemanticRootsSeed<'a, A: ?Sized> {
    admission: &'a A,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de> for SemanticRootsSeed<'_, A> {
    type Value = SemanticContentRootsV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_struct(
            "SemanticContentRootsV1",
            &["row_root_digest", "membership_root_digest"],
            self,
        )
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> Visitor<'de> for SemanticRootsSeed<'_, A> {
    type Value = SemanticContentRootsV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("admitted semantic content roots")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut row_root_digest = None;
        let mut membership_root_digest = None;
        while let Some(key) = map.next_key_seed(RootsFieldSeed)? {
            match key {
                "row_root_digest" if row_root_digest.is_none() => {
                    row_root_digest =
                        Some(map.next_value_seed(AdmittedTextSeedV1::new(self.admission, 71))?);
                }
                "membership_root_digest" if membership_root_digest.is_none() => {
                    membership_root_digest =
                        Some(map.next_value_seed(AdmittedTextSeedV1::new(self.admission, 71))?);
                }
                duplicate => return Err(de::Error::duplicate_field(duplicate)),
            }
        }
        let roots = SemanticContentRootsV1 {
            row_root_digest: row_root_digest
                .ok_or_else(|| de::Error::missing_field("row_root_digest"))?,
            membership_root_digest: membership_root_digest
                .ok_or_else(|| de::Error::missing_field("membership_root_digest"))?,
        };
        if !roots.is_canonical_v1() {
            return Err(de::Error::custom(
                "semantic content roots must be canonical",
            ));
        }
        Ok(roots)
    }
}

pub(super) struct AuxiliaryReceiptSeedV1<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'a, A: DecodeBackingAdmissionV1 + ?Sized> AuxiliaryReceiptSeedV1<'a, A> {
    pub(super) const fn new(admission: &'a A, max_text_bytes: usize) -> Self {
        Self {
            admission,
            max_text_bytes,
        }
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AuxiliaryReceiptSeedV1<'_, A>
{
    type Value = BatchPublishReceipt;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_struct("BatchPublishReceipt", BATCH_PUBLISH_RECEIPT_FIELDS, self)
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> Visitor<'de> for AuxiliaryReceiptSeedV1<'_, A> {
    type Value = BatchPublishReceipt;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an admitted auxiliary BatchPublishReceipt")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut generation: Option<ManifestGeneration> = None;
        let mut manifest_digest: Option<Option<String>> = None;
        let mut batch_digest: Option<String> = None;
        let mut accepted_replace_scopes: Option<u32> = None;
        let mut accepted_tombstone_scopes: Option<u32> = None;
        let mut accepted_semantic_replace_scopes: Option<u32> = None;
        let mut accepted_semantic_tombstone_scopes: Option<u32> = None;
        let mut accepted_clear_surfaces: Option<u32> = None;
        let mut sealed: Option<bool> = None;
        let mut applied: Option<bool> = None;
        let mut durable_sequence: Option<u64> = None;
        let mut semantic_content: Option<Option<crate::SemanticContentRootsV1>> = None;
        while let Some(key) = map.next_key_seed(KnownFieldSeed)? {
            match key {
                "generation" if generation.is_none() => generation = Some(map.next_value()?),
                "manifest_digest" if manifest_digest.is_none() => {
                    manifest_digest = Some(map.next_value_seed(AdmittedOptionalSeedV1::new(
                        self.admission,
                        AdmittedTextSeedV1::new(self.admission, self.max_text_bytes),
                    ))?);
                }
                "batch_digest" if batch_digest.is_none() => {
                    batch_digest =
                        Some(map.next_value_seed(AdmittedTextSeedV1::new(self.admission, 64))?);
                }
                "accepted_replace_scopes" if accepted_replace_scopes.is_none() => {
                    accepted_replace_scopes = Some(map.next_value()?);
                }
                "accepted_tombstone_scopes" if accepted_tombstone_scopes.is_none() => {
                    accepted_tombstone_scopes = Some(map.next_value()?);
                }
                "accepted_semantic_replace_scopes"
                    if accepted_semantic_replace_scopes.is_none() =>
                {
                    accepted_semantic_replace_scopes = Some(map.next_value()?);
                }
                "accepted_semantic_tombstone_scopes"
                    if accepted_semantic_tombstone_scopes.is_none() =>
                {
                    accepted_semantic_tombstone_scopes = Some(map.next_value()?);
                }
                "accepted_clear_surfaces" if accepted_clear_surfaces.is_none() => {
                    accepted_clear_surfaces = Some(map.next_value()?);
                }
                "sealed" if sealed.is_none() => sealed = Some(map.next_value()?),
                "applied" if applied.is_none() => applied = Some(map.next_value()?),
                "durable_sequence" if durable_sequence.is_none() => {
                    durable_sequence = Some(map.next_value()?);
                }
                "semantic_content" if semantic_content.is_none() => {
                    semantic_content = Some(map.next_value_seed(AdmittedOptionalSeedV1::new(
                        self.admission,
                        SemanticRootsSeed {
                            admission: self.admission,
                        },
                    ))?);
                }
                duplicate => return Err(de::Error::duplicate_field(duplicate)),
            }
        }
        Ok(BatchPublishReceipt {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            accepted_replace_scopes: accepted_replace_scopes
                .ok_or_else(|| de::Error::missing_field("accepted_replace_scopes"))?,
            accepted_tombstone_scopes: accepted_tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("accepted_tombstone_scopes"))?,
            accepted_semantic_replace_scopes: accepted_semantic_replace_scopes
                .ok_or_else(|| de::Error::missing_field("accepted_semantic_replace_scopes"))?,
            accepted_semantic_tombstone_scopes: accepted_semantic_tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("accepted_semantic_tombstone_scopes"))?,
            accepted_clear_surfaces: accepted_clear_surfaces
                .ok_or_else(|| de::Error::missing_field("accepted_clear_surfaces"))?,
            sealed: sealed.ok_or_else(|| de::Error::missing_field("sealed"))?,
            applied: applied.ok_or_else(|| de::Error::missing_field("applied"))?,
            durable_sequence: durable_sequence
                .ok_or_else(|| de::Error::missing_field("durable_sequence"))?,
            semantic_content: semantic_content
                .ok_or_else(|| de::Error::missing_field("semantic_content"))?,
        })
    }
}
