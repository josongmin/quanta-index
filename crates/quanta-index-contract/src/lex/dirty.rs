//! `DirtyRecord`.
//!
//! Wire shape: [`docs/ssot/producer-handoff.md`](../../../../docs/ssot/producer-handoff.md)
//! §3.2.1 (`UpsertDirty` payload).
//!
//! The op-level `UpsertDirty { repo, revision, generation, doc_id, applied_at_ms,
//! payload_hash }` carries identity through the channel envelope; this
//! scaffold lands the **payload subset** that downstream `lq_runtime`
//! placeholders model directly. The full op (with repo / revision / generation)
//! is already on the wire via `LexicalChannelOp`.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
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

const DIRTY_RECORD_FIELDS: &[&str] =
    &["wire_version", "doc_id", "applied_at_ms", "payload_hash"];

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

struct DirtyRecordVisitor;

impl<'de> Visitor<'de> for DirtyRecordVisitor {
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
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
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
                    doc_id = Some(map.next_value()?);
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
        let wire_version =
            wire_version.ok_or_else(|| de::Error::missing_field("wire_version"))?;
        let doc_id = doc_id.ok_or_else(|| de::Error::missing_field("doc_id"))?;
        let applied_at_ms =
            applied_at_ms.ok_or_else(|| de::Error::missing_field("applied_at_ms"))?;
        let payload_hash =
            payload_hash.ok_or_else(|| de::Error::missing_field("payload_hash"))?;
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
        deserializer.deserialize_struct("DirtyRecord", DIRTY_RECORD_FIELDS, DirtyRecordVisitor)
    }
}
