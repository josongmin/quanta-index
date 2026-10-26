//! `DirtyRecord`.
//!
//! This file defines the wire shape for the `UpsertDirty` payload subset;
//! `docs/adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md` owns
//! ingress boundaries.
//!
//! This is the **payload subset** of the producer's `UpsertDirty` entry;
//! repo / revision / generation travel on the enclosing
//! [`crate::DirtyIngestBatch`], the only dirty ingress.

use core::fmt;

use quanta_index_contract_base::DecodeBackingAdmissionV1;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeSeed, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::ChunkId;

/// Producer-authored dirty-buffer entry per producer-handoff §3.2.1.
///
/// `doc_id` reuses [`ChunkId`] — the same chunk identity already carried by
/// [`crate::UpsertChunk`]. This binds dirty entries to chunks via the
/// producer's chunk namespace.
///
/// `payload_hash` is an opaque 32-byte producer-supplied content hash; the
/// search side does not interpret it (producer is the authority per
/// producer-handoff §2).
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DirtyRecord {
    pub wire_version: u32,
    pub doc_id: ChunkId,
    pub applied_at_ms: u64,
    pub payload_hash: [u8; 32],
}

const DIRTY_RECORD_FIELDS: &[&str] = &["wire_version", "doc_id", "applied_at_ms", "payload_hash"];

impl Serialize for DirtyRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DirtyRecord", 4)?;
        state.serialize_field("wire_version", &self.wire_version)?;
        state.serialize_field("doc_id", &self.doc_id)?;
        state.serialize_field("applied_at_ms", &self.applied_at_ms)?;
        state.serialize_field("payload_hash", &self.payload_hash)?;
        state.end()
    }
}

struct DirtyRecordVisitor<P>(P);

trait DirtyRecordDecodePolicy<'de> {
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error>;
    fn next_doc_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ChunkId, M::Error>;
}

struct OrdinaryDirtyRecord;

struct DirtyRecordFieldSeed;

impl<'de> DeserializeSeed<'de> for DirtyRecordFieldSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for DirtyRecordFieldSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed DirtyRecord field")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        DIRTY_RECORD_FIELDS
            .iter()
            .copied()
            .find(|field| *field == value)
            .ok_or_else(|| E::unknown_field(value, DIRTY_RECORD_FIELDS))
    }

    fn visit_string<E: de::Error>(self, _value: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted DirtyRecord field must be borrowed"))
    }
}

impl<'de> DirtyRecordDecodePolicy<'de> for OrdinaryDirtyRecord {
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error> {
        map.next_key::<String>()?
            .map(|key| {
                DIRTY_RECORD_FIELDS
                    .iter()
                    .copied()
                    .find(|field| *field == key)
                    .ok_or_else(|| de::Error::unknown_field(key.as_str(), DIRTY_RECORD_FIELDS))
            })
            .transpose()
    }

    fn next_doc_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ChunkId, M::Error> {
        map.next_value()
    }
}

struct AdmittedDirtyRecord<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DirtyRecordDecodePolicy<'de>
    for AdmittedDirtyRecord<'_, A>
{
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error> {
        map.next_key_seed(DirtyRecordFieldSeed)
    }

    fn next_doc_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ChunkId, M::Error> {
        map.next_value_seed(ChunkId::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_text_bytes,
        ))
    }
}

impl DirtyRecord {
    /// The caller retains `admission` and its parser backing with the result.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        AdmittedDirtyRecord {
            admission,
            max_text_bytes,
        }
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedDirtyRecord<'_, A>
{
    type Value = DirtyRecord;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_struct(
            "DirtyRecord",
            DIRTY_RECORD_FIELDS,
            DirtyRecordVisitor(self),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

impl<'de, P: DirtyRecordDecodePolicy<'de>> Visitor<'de> for DirtyRecordVisitor<P> {
    type Value = DirtyRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DirtyRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut wire_version: Option<u32> = None;
        let mut doc_id: Option<ChunkId> = None;
        let mut applied_at_ms: Option<u64> = None;
        let mut payload_hash: Option<[u8; 32]> = None;
        while let Some(key) = self.0.next_field(&mut map)? {
            match key {
                "wire_version" => {
                    if wire_version.is_some() {
                        return Err(de::Error::duplicate_field("wire_version"));
                    }
                    wire_version = Some(map.next_value()?);
                }
                "doc_id" => {
                    if doc_id.is_some() {
                        return Err(de::Error::duplicate_field("doc_id"));
                    }
                    doc_id = Some(self.0.next_doc_id(&mut map)?);
                }
                "applied_at_ms" => {
                    if applied_at_ms.is_some() {
                        return Err(de::Error::duplicate_field("applied_at_ms"));
                    }
                    applied_at_ms = Some(map.next_value()?);
                }
                "payload_hash" => {
                    if payload_hash.is_some() {
                        return Err(de::Error::duplicate_field("payload_hash"));
                    }
                    payload_hash = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, DIRTY_RECORD_FIELDS)),
            }
        }
        let wire_version = wire_version.ok_or_else(|| de::Error::missing_field("wire_version"))?;
        let doc_id = doc_id.ok_or_else(|| de::Error::missing_field("doc_id"))?;
        let applied_at_ms =
            applied_at_ms.ok_or_else(|| de::Error::missing_field("applied_at_ms"))?;
        let payload_hash = payload_hash.ok_or_else(|| de::Error::missing_field("payload_hash"))?;
        Ok(DirtyRecord {
            wire_version,
            doc_id,
            applied_at_ms,
            payload_hash,
        })
    }
}

impl<'de> Deserialize<'de> for DirtyRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "DirtyRecord",
            DIRTY_RECORD_FIELDS,
            DirtyRecordVisitor(OrdinaryDirtyRecord),
        )
    }
}
