//! Typed ingest IPC contract (QI-ING-01).
//!
//! Producer / search-plane integration surface for batch publishes. The
//! producer sends a [`SearchPlaneIngestIpcRequestEnvelope`] over UDS
//! `ingest.sock`; searchd's ingest dispatcher fans the typed batch out to
//! internal `LexicalChannelOp` / `SemanticChannelOp` / `RepoMap` bundle ingest
//! streams. The producer never opens a channel publisher directly.
//!
//! Wire shape: every DTO in this module implements `Serialize` /
//! `Deserialize` manually. Workspace bans proc-macro serde derives
//! (CLAUDE.md "no proc-macro derives for serialization"); the manual impls
//! keep cold-build cost bounded and make the wire shape auditable in review.
//! Unknown fields and duplicate fields fail-closed; missing required fields
//! raise `missing_field` rather than synthesising defaults.
//!
//! Wire tagging: enum variants use serde's native externally-tagged shape
//! (`{"Upsert": {...}}`) via `serialize_newtype_variant` /
//! `deserialize_enum`. This is format-agnostic — works under CBOR, JSON, or
//! any other serde transport — and reads naturally without needing a
//! format-specific intermediate value type.
//!
//! Existing query / control envelopes in `split.rs` use adjacent tagging via
//! `#[serde(tag = "kind", content = "payload")]`. They predate this module
//! and live on a separate migration timeline (see workspace rule
//! `rust-no-serde-derive`); the wire format difference between the two is
//! intentional for the new ingest surface.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, VariantAccess, Visitor},
    ser::SerializeStruct,
};

use crate::lex::SymbolRecord;
use crate::{
    ChannelSeq, ChunkId, ChunkRecord, EmbeddingId, EmbeddingRecord, ManifestGeneration, RepoId,
    RepoMapMutationAck, RepoMapSourceBundle, RevisionId, SymbolId,
};

use super::error::SearchPlaneIpcError;

// =============================================================================
// Batch mode
// =============================================================================

/// Whether a batch replaces the active generation atomically or applies as a
/// delta on top of the existing generation.
///
/// Mirrors the `BatchMode` exposed by the SDK's batch builder; lifted here so
/// it travels over the wire as part of the ingest contract rather than as an
/// SDK-only concept.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BatchIngestMode {
    ReplaceGeneration,
    Delta,
}

const BATCH_INGEST_MODE_VARIANTS: &[&str] = &["ReplaceGeneration", "Delta"];

impl Serialize for BatchIngestMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let tag = match self {
            Self::ReplaceGeneration => "ReplaceGeneration",
            Self::Delta => "Delta",
        };
        serializer.serialize_str(tag)
    }
}

struct BatchIngestModeVisitor;

impl Visitor<'_> for BatchIngestModeVisitor {
    type Value = BatchIngestMode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BatchIngestMode tag: \"ReplaceGeneration\" | \"Delta\"")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "ReplaceGeneration" => Ok(BatchIngestMode::ReplaceGeneration),
            "Delta" => Ok(BatchIngestMode::Delta),
            other => Err(de::Error::unknown_variant(
                other,
                BATCH_INGEST_MODE_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for BatchIngestMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(BatchIngestModeVisitor)
    }
}

// =============================================================================
// Lexical chunk mutation
// =============================================================================

/// Upsert payload for a single lexical chunk inside a [`LexicalIngestBatch`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalChunkUpsert {
    pub chunk_id: ChunkId,
    pub record: ChunkRecord,
}

const LEXICAL_CHUNK_UPSERT_FIELDS: &[&str] = &["chunk_id", "record"];

impl Serialize for LexicalChunkUpsert {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LexicalChunkUpsert", 2)?;
        state.serialize_field("chunk_id", &self.chunk_id)?;
        state.serialize_field("record", &self.record)?;
        state.end()
    }
}

struct LexicalChunkUpsertVisitor;

impl<'de> Visitor<'de> for LexicalChunkUpsertVisitor {
    type Value = LexicalChunkUpsert;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LexicalChunkUpsert map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut chunk_id: Option<ChunkId> = None;
        let mut record: Option<ChunkRecord> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "chunk_id" => {
                    if chunk_id.is_some() {
                        return Err(de::Error::duplicate_field("chunk_id"));
                    }
                    chunk_id = Some(map.next_value()?);
                }
                "record" => {
                    if record.is_some() {
                        return Err(de::Error::duplicate_field("record"));
                    }
                    record = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, LEXICAL_CHUNK_UPSERT_FIELDS));
                }
            }
        }
        Ok(LexicalChunkUpsert {
            chunk_id: chunk_id.ok_or_else(|| de::Error::missing_field("chunk_id"))?,
            record: record.ok_or_else(|| de::Error::missing_field("record"))?,
        })
    }
}

impl<'de> Deserialize<'de> for LexicalChunkUpsert {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "LexicalChunkUpsert",
            LEXICAL_CHUNK_UPSERT_FIELDS,
            LexicalChunkUpsertVisitor,
        )
    }
}

/// Delete payload for a single lexical chunk inside a [`LexicalIngestBatch`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalChunkDelete {
    pub chunk_id: ChunkId,
}

const LEXICAL_CHUNK_DELETE_FIELDS: &[&str] = &["chunk_id"];

impl Serialize for LexicalChunkDelete {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LexicalChunkDelete", 1)?;
        state.serialize_field("chunk_id", &self.chunk_id)?;
        state.end()
    }
}

struct LexicalChunkDeleteVisitor;

impl<'de> Visitor<'de> for LexicalChunkDeleteVisitor {
    type Value = LexicalChunkDelete;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LexicalChunkDelete map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut chunk_id: Option<ChunkId> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "chunk_id" => {
                    if chunk_id.is_some() {
                        return Err(de::Error::duplicate_field("chunk_id"));
                    }
                    chunk_id = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, LEXICAL_CHUNK_DELETE_FIELDS));
                }
            }
        }
        Ok(LexicalChunkDelete {
            chunk_id: chunk_id.ok_or_else(|| de::Error::missing_field("chunk_id"))?,
        })
    }
}

impl<'de> Deserialize<'de> for LexicalChunkDelete {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "LexicalChunkDelete",
            LEXICAL_CHUNK_DELETE_FIELDS,
            LexicalChunkDeleteVisitor,
        )
    }
}

/// One mutation against the lexical chunk surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LexicalChunkMutation {
    Upsert(LexicalChunkUpsert),
    Delete(LexicalChunkDelete),
}

const LEXICAL_CHUNK_MUTATION_VARIANTS: &[&str] = &["Upsert", "Delete"];

impl Serialize for LexicalChunkMutation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Upsert(payload) => {
                serializer.serialize_newtype_variant("LexicalChunkMutation", 0, "Upsert", payload)
            }
            Self::Delete(payload) => {
                serializer.serialize_newtype_variant("LexicalChunkMutation", 1, "Delete", payload)
            }
        }
    }
}

struct LexicalChunkMutationVisitor;

impl<'de> Visitor<'de> for LexicalChunkMutationVisitor {
    type Value = LexicalChunkMutation;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LexicalChunkMutation enum")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::EnumAccess<'de>,
    {
        let (tag, variant) = data.variant::<String>()?;
        match tag.as_str() {
            "Upsert" => Ok(LexicalChunkMutation::Upsert(variant.newtype_variant()?)),
            "Delete" => Ok(LexicalChunkMutation::Delete(variant.newtype_variant()?)),
            other => Err(de::Error::unknown_variant(
                other,
                LEXICAL_CHUNK_MUTATION_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for LexicalChunkMutation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_enum(
            "LexicalChunkMutation",
            LEXICAL_CHUNK_MUTATION_VARIANTS,
            LexicalChunkMutationVisitor,
        )
    }
}

// =============================================================================
// Lexical symbol mutation
// =============================================================================

/// Upsert payload for a single lexical symbol inside a [`LexicalIngestBatch`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalSymbolUpsert {
    pub symbol_id: SymbolId,
    pub record: SymbolRecord,
}

const LEXICAL_SYMBOL_UPSERT_FIELDS: &[&str] = &["symbol_id", "record"];

impl Serialize for LexicalSymbolUpsert {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LexicalSymbolUpsert", 2)?;
        state.serialize_field("symbol_id", &self.symbol_id)?;
        state.serialize_field("record", &self.record)?;
        state.end()
    }
}

struct LexicalSymbolUpsertVisitor;

impl<'de> Visitor<'de> for LexicalSymbolUpsertVisitor {
    type Value = LexicalSymbolUpsert;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LexicalSymbolUpsert map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut symbol_id: Option<SymbolId> = None;
        let mut record: Option<SymbolRecord> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "symbol_id" => {
                    if symbol_id.is_some() {
                        return Err(de::Error::duplicate_field("symbol_id"));
                    }
                    symbol_id = Some(map.next_value()?);
                }
                "record" => {
                    if record.is_some() {
                        return Err(de::Error::duplicate_field("record"));
                    }
                    record = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        LEXICAL_SYMBOL_UPSERT_FIELDS,
                    ));
                }
            }
        }
        Ok(LexicalSymbolUpsert {
            symbol_id: symbol_id.ok_or_else(|| de::Error::missing_field("symbol_id"))?,
            record: record.ok_or_else(|| de::Error::missing_field("record"))?,
        })
    }
}

impl<'de> Deserialize<'de> for LexicalSymbolUpsert {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "LexicalSymbolUpsert",
            LEXICAL_SYMBOL_UPSERT_FIELDS,
            LexicalSymbolUpsertVisitor,
        )
    }
}

/// Delete payload for a single lexical symbol inside a [`LexicalIngestBatch`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalSymbolDelete {
    pub symbol_id: SymbolId,
}

const LEXICAL_SYMBOL_DELETE_FIELDS: &[&str] = &["symbol_id"];

impl Serialize for LexicalSymbolDelete {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LexicalSymbolDelete", 1)?;
        state.serialize_field("symbol_id", &self.symbol_id)?;
        state.end()
    }
}

struct LexicalSymbolDeleteVisitor;

impl<'de> Visitor<'de> for LexicalSymbolDeleteVisitor {
    type Value = LexicalSymbolDelete;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LexicalSymbolDelete map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut symbol_id: Option<SymbolId> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "symbol_id" => {
                    if symbol_id.is_some() {
                        return Err(de::Error::duplicate_field("symbol_id"));
                    }
                    symbol_id = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        LEXICAL_SYMBOL_DELETE_FIELDS,
                    ));
                }
            }
        }
        Ok(LexicalSymbolDelete {
            symbol_id: symbol_id.ok_or_else(|| de::Error::missing_field("symbol_id"))?,
        })
    }
}

impl<'de> Deserialize<'de> for LexicalSymbolDelete {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "LexicalSymbolDelete",
            LEXICAL_SYMBOL_DELETE_FIELDS,
            LexicalSymbolDeleteVisitor,
        )
    }
}

/// One mutation against the lexical symbol surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LexicalSymbolMutation {
    Upsert(LexicalSymbolUpsert),
    Delete(LexicalSymbolDelete),
}

const LEXICAL_SYMBOL_MUTATION_VARIANTS: &[&str] = &["Upsert", "Delete"];

impl Serialize for LexicalSymbolMutation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Upsert(payload) => {
                serializer.serialize_newtype_variant("LexicalSymbolMutation", 0, "Upsert", payload)
            }
            Self::Delete(payload) => {
                serializer.serialize_newtype_variant("LexicalSymbolMutation", 1, "Delete", payload)
            }
        }
    }
}

struct LexicalSymbolMutationVisitor;

impl<'de> Visitor<'de> for LexicalSymbolMutationVisitor {
    type Value = LexicalSymbolMutation;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LexicalSymbolMutation enum")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::EnumAccess<'de>,
    {
        let (tag, variant) = data.variant::<String>()?;
        match tag.as_str() {
            "Upsert" => Ok(LexicalSymbolMutation::Upsert(variant.newtype_variant()?)),
            "Delete" => Ok(LexicalSymbolMutation::Delete(variant.newtype_variant()?)),
            other => Err(de::Error::unknown_variant(
                other,
                LEXICAL_SYMBOL_MUTATION_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for LexicalSymbolMutation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_enum(
            "LexicalSymbolMutation",
            LEXICAL_SYMBOL_MUTATION_VARIANTS,
            LexicalSymbolMutationVisitor,
        )
    }
}

// =============================================================================
// Semantic embedding mutation
// =============================================================================

/// Upsert payload for a single semantic embedding inside a
/// [`SemanticIngestBatch`].
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticEmbeddingUpsert {
    pub embedding_id: EmbeddingId,
    pub record: EmbeddingRecord,
}

const SEMANTIC_EMBEDDING_UPSERT_FIELDS: &[&str] = &["embedding_id", "record"];

impl Serialize for SemanticEmbeddingUpsert {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticEmbeddingUpsert", 2)?;
        state.serialize_field("embedding_id", &self.embedding_id)?;
        state.serialize_field("record", &self.record)?;
        state.end()
    }
}

struct SemanticEmbeddingUpsertVisitor;

impl<'de> Visitor<'de> for SemanticEmbeddingUpsertVisitor {
    type Value = SemanticEmbeddingUpsert;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticEmbeddingUpsert map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut embedding_id: Option<EmbeddingId> = None;
        let mut record: Option<EmbeddingRecord> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "embedding_id" => {
                    if embedding_id.is_some() {
                        return Err(de::Error::duplicate_field("embedding_id"));
                    }
                    embedding_id = Some(map.next_value()?);
                }
                "record" => {
                    if record.is_some() {
                        return Err(de::Error::duplicate_field("record"));
                    }
                    record = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_EMBEDDING_UPSERT_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticEmbeddingUpsert {
            embedding_id: embedding_id.ok_or_else(|| de::Error::missing_field("embedding_id"))?,
            record: record.ok_or_else(|| de::Error::missing_field("record"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticEmbeddingUpsert {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticEmbeddingUpsert",
            SEMANTIC_EMBEDDING_UPSERT_FIELDS,
            SemanticEmbeddingUpsertVisitor,
        )
    }
}

/// Delete payload for a single semantic embedding inside a
/// [`SemanticIngestBatch`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticEmbeddingDelete {
    pub embedding_id: EmbeddingId,
}

const SEMANTIC_EMBEDDING_DELETE_FIELDS: &[&str] = &["embedding_id"];

impl Serialize for SemanticEmbeddingDelete {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticEmbeddingDelete", 1)?;
        state.serialize_field("embedding_id", &self.embedding_id)?;
        state.end()
    }
}

struct SemanticEmbeddingDeleteVisitor;

impl<'de> Visitor<'de> for SemanticEmbeddingDeleteVisitor {
    type Value = SemanticEmbeddingDelete;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticEmbeddingDelete map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut embedding_id: Option<EmbeddingId> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "embedding_id" => {
                    if embedding_id.is_some() {
                        return Err(de::Error::duplicate_field("embedding_id"));
                    }
                    embedding_id = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_EMBEDDING_DELETE_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticEmbeddingDelete {
            embedding_id: embedding_id.ok_or_else(|| de::Error::missing_field("embedding_id"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticEmbeddingDelete {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticEmbeddingDelete",
            SEMANTIC_EMBEDDING_DELETE_FIELDS,
            SemanticEmbeddingDeleteVisitor,
        )
    }
}

/// One mutation against the semantic embedding surface.
#[derive(Clone, Debug, PartialEq)]
pub enum SemanticEmbeddingMutation {
    Upsert(SemanticEmbeddingUpsert),
    Delete(SemanticEmbeddingDelete),
}

const SEMANTIC_EMBEDDING_MUTATION_VARIANTS: &[&str] = &["Upsert", "Delete"];

impl Serialize for SemanticEmbeddingMutation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Upsert(payload) => serializer.serialize_newtype_variant(
                "SemanticEmbeddingMutation",
                0,
                "Upsert",
                payload,
            ),
            Self::Delete(payload) => serializer.serialize_newtype_variant(
                "SemanticEmbeddingMutation",
                1,
                "Delete",
                payload,
            ),
        }
    }
}

struct SemanticEmbeddingMutationVisitor;

impl<'de> Visitor<'de> for SemanticEmbeddingMutationVisitor {
    type Value = SemanticEmbeddingMutation;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticEmbeddingMutation enum")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::EnumAccess<'de>,
    {
        let (tag, variant) = data.variant::<String>()?;
        match tag.as_str() {
            "Upsert" => Ok(SemanticEmbeddingMutation::Upsert(
                variant.newtype_variant()?,
            )),
            "Delete" => Ok(SemanticEmbeddingMutation::Delete(
                variant.newtype_variant()?,
            )),
            other => Err(de::Error::unknown_variant(
                other,
                SEMANTIC_EMBEDDING_MUTATION_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for SemanticEmbeddingMutation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_enum(
            "SemanticEmbeddingMutation",
            SEMANTIC_EMBEDDING_MUTATION_VARIANTS,
            SemanticEmbeddingMutationVisitor,
        )
    }
}

// =============================================================================
// Lexical / Semantic ingest batches
// =============================================================================

/// Typed lexical batch the producer sends to searchd's ingest dispatcher.
///
/// `manifest_payload` is opaque producer-side bookkeeping and is forwarded
/// into `LexicalFullBundle::payload` when the dispatcher fans out to channel
/// ops. Same opaqueness convention as the existing `LexicalFullBundle`
/// channel field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub mode: BatchIngestMode,
    pub manifest_payload: Vec<u8>,
    pub chunks: Vec<LexicalChunkMutation>,
    pub symbols: Vec<LexicalSymbolMutation>,
    pub seal: bool,
}

const LEXICAL_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "mode",
    "manifest_payload",
    "chunks",
    "symbols",
    "seal",
];

impl Serialize for LexicalIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LexicalIngestBatch", 8)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("manifest_payload", &Bytes::new(&self.manifest_payload))?;
        state.serialize_field("chunks", &self.chunks)?;
        state.serialize_field("symbols", &self.symbols)?;
        state.serialize_field("seal", &self.seal)?;
        state.end()
    }
}

struct LexicalIngestBatchVisitor;

impl<'de> Visitor<'de> for LexicalIngestBatchVisitor {
    type Value = LexicalIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LexicalIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut mode: Option<BatchIngestMode> = None;
        let mut manifest_payload: Option<Vec<u8>> = None;
        let mut chunks: Option<Vec<LexicalChunkMutation>> = None;
        let mut symbols: Option<Vec<LexicalSymbolMutation>> = None;
        let mut seal: Option<bool> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "mode" => {
                    if mode.is_some() {
                        return Err(de::Error::duplicate_field("mode"));
                    }
                    mode = Some(map.next_value()?);
                }
                "manifest_payload" => {
                    if manifest_payload.is_some() {
                        return Err(de::Error::duplicate_field("manifest_payload"));
                    }
                    let bytes: ByteBuf = map.next_value()?;
                    manifest_payload = Some(bytes.into_vec());
                }
                "chunks" => {
                    if chunks.is_some() {
                        return Err(de::Error::duplicate_field("chunks"));
                    }
                    chunks = Some(map.next_value()?);
                }
                "symbols" => {
                    if symbols.is_some() {
                        return Err(de::Error::duplicate_field("symbols"));
                    }
                    symbols = Some(map.next_value()?);
                }
                "seal" => {
                    if seal.is_some() {
                        return Err(de::Error::duplicate_field("seal"));
                    }
                    seal = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, LEXICAL_INGEST_BATCH_FIELDS));
                }
            }
        }
        Ok(LexicalIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            mode: mode.ok_or_else(|| de::Error::missing_field("mode"))?,
            manifest_payload: manifest_payload
                .ok_or_else(|| de::Error::missing_field("manifest_payload"))?,
            chunks: chunks.ok_or_else(|| de::Error::missing_field("chunks"))?,
            symbols: symbols.ok_or_else(|| de::Error::missing_field("symbols"))?,
            seal: seal.ok_or_else(|| de::Error::missing_field("seal"))?,
        })
    }
}

impl<'de> Deserialize<'de> for LexicalIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "LexicalIngestBatch",
            LEXICAL_INGEST_BATCH_FIELDS,
            LexicalIngestBatchVisitor,
        )
    }
}

/// Typed semantic batch the producer sends to searchd's ingest dispatcher.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub mode: BatchIngestMode,
    pub manifest_payload: Vec<u8>,
    pub embeddings: Vec<SemanticEmbeddingMutation>,
    pub seal: bool,
}

const SEMANTIC_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "mode",
    "manifest_payload",
    "embeddings",
    "seal",
];

impl Serialize for SemanticIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticIngestBatch", 7)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("manifest_payload", &Bytes::new(&self.manifest_payload))?;
        state.serialize_field("embeddings", &self.embeddings)?;
        state.serialize_field("seal", &self.seal)?;
        state.end()
    }
}

struct SemanticIngestBatchVisitor;

impl<'de> Visitor<'de> for SemanticIngestBatchVisitor {
    type Value = SemanticIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut mode: Option<BatchIngestMode> = None;
        let mut manifest_payload: Option<Vec<u8>> = None;
        let mut embeddings: Option<Vec<SemanticEmbeddingMutation>> = None;
        let mut seal: Option<bool> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "mode" => {
                    if mode.is_some() {
                        return Err(de::Error::duplicate_field("mode"));
                    }
                    mode = Some(map.next_value()?);
                }
                "manifest_payload" => {
                    if manifest_payload.is_some() {
                        return Err(de::Error::duplicate_field("manifest_payload"));
                    }
                    let bytes: ByteBuf = map.next_value()?;
                    manifest_payload = Some(bytes.into_vec());
                }
                "embeddings" => {
                    if embeddings.is_some() {
                        return Err(de::Error::duplicate_field("embeddings"));
                    }
                    embeddings = Some(map.next_value()?);
                }
                "seal" => {
                    if seal.is_some() {
                        return Err(de::Error::duplicate_field("seal"));
                    }
                    seal = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            mode: mode.ok_or_else(|| de::Error::missing_field("mode"))?,
            manifest_payload: manifest_payload
                .ok_or_else(|| de::Error::missing_field("manifest_payload"))?,
            embeddings: embeddings.ok_or_else(|| de::Error::missing_field("embeddings"))?,
            seal: seal.ok_or_else(|| de::Error::missing_field("seal"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticIngestBatch",
            SEMANTIC_INGEST_BATCH_FIELDS,
            SemanticIngestBatchVisitor,
        )
    }
}

// =============================================================================
// BatchPublishReceipt
// =============================================================================

/// Server-side receipt for a successful batch publish.
///
/// `first_seq` / `last_seq` are the inclusive channel sequence range assigned
/// by searchd's channel publisher; `sealed` is true when the batch included a
/// closing seal op.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BatchPublishReceipt {
    pub first_seq: Option<ChannelSeq>,
    pub last_seq: Option<ChannelSeq>,
    pub sealed: bool,
}

const BATCH_PUBLISH_RECEIPT_FIELDS: &[&str] = &["first_seq", "last_seq", "sealed"];

impl Serialize for BatchPublishReceipt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("BatchPublishReceipt", 3)?;
        state.serialize_field("first_seq", &self.first_seq)?;
        state.serialize_field("last_seq", &self.last_seq)?;
        state.serialize_field("sealed", &self.sealed)?;
        state.end()
    }
}

struct BatchPublishReceiptVisitor;

impl<'de> Visitor<'de> for BatchPublishReceiptVisitor {
    type Value = BatchPublishReceipt;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BatchPublishReceipt map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut first_seq: Option<Option<ChannelSeq>> = None;
        let mut last_seq: Option<Option<ChannelSeq>> = None;
        let mut sealed: Option<bool> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "first_seq" => {
                    if first_seq.is_some() {
                        return Err(de::Error::duplicate_field("first_seq"));
                    }
                    first_seq = Some(map.next_value()?);
                }
                "last_seq" => {
                    if last_seq.is_some() {
                        return Err(de::Error::duplicate_field("last_seq"));
                    }
                    last_seq = Some(map.next_value()?);
                }
                "sealed" => {
                    if sealed.is_some() {
                        return Err(de::Error::duplicate_field("sealed"));
                    }
                    sealed = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        BATCH_PUBLISH_RECEIPT_FIELDS,
                    ));
                }
            }
        }
        Ok(BatchPublishReceipt {
            first_seq: first_seq.ok_or_else(|| de::Error::missing_field("first_seq"))?,
            last_seq: last_seq.ok_or_else(|| de::Error::missing_field("last_seq"))?,
            sealed: sealed.ok_or_else(|| de::Error::missing_field("sealed"))?,
        })
    }
}

impl<'de> Deserialize<'de> for BatchPublishReceipt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "BatchPublishReceipt",
            BATCH_PUBLISH_RECEIPT_FIELDS,
            BatchPublishReceiptVisitor,
        )
    }
}

impl BatchPublishReceipt {
    /// Record one channel sequence into the receipt. Used by ingest dispatchers
    /// after each successful channel `publish()`. Sequences are appended in
    /// channel order; `first_seq` captures the earliest, `last_seq` the most
    /// recent.
    pub fn record(&mut self, seq: ChannelSeq) {
        if self.first_seq.is_none() {
            self.first_seq = Some(seq);
        }
        self.last_seq = Some(seq);
    }

    /// Flag the receipt as carrying a closing seal op.
    pub fn mark_sealed(&mut self) {
        self.sealed = true;
    }
}

// =============================================================================
// Top-level request / response enums
// =============================================================================

/// Typed ingest request payload sent over `ingest.sock`.
#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::enum_variant_names,
    reason = "wire protocol keeps explicit publish prefixes for request-kind clarity"
)]
pub enum SearchPlaneIngestIpcRequest {
    PublishLexicalBatch(LexicalIngestBatch),
    PublishSemanticBatch(SemanticIngestBatch),
    PublishRepoMapBundle(RepoMapSourceBundle),
}

const SEARCH_PLANE_INGEST_REQUEST_VARIANTS: &[&str] = &[
    "PublishLexicalBatch",
    "PublishSemanticBatch",
    "PublishRepoMapBundle",
];

impl Serialize for SearchPlaneIngestIpcRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::PublishLexicalBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                0,
                "PublishLexicalBatch",
                payload,
            ),
            Self::PublishSemanticBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                1,
                "PublishSemanticBatch",
                payload,
            ),
            Self::PublishRepoMapBundle(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                2,
                "PublishRepoMapBundle",
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
            "PublishLexicalBatch" => Ok(SearchPlaneIngestIpcRequest::PublishLexicalBatch(
                variant.newtype_variant()?,
            )),
            "PublishSemanticBatch" => Ok(SearchPlaneIngestIpcRequest::PublishSemanticBatch(
                variant.newtype_variant()?,
            )),
            "PublishRepoMapBundle" => Ok(SearchPlaneIngestIpcRequest::PublishRepoMapBundle(
                variant.newtype_variant()?,
            )),
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
pub enum SearchPlaneIngestIpcResponse {
    LexicalReceipt(BatchPublishReceipt),
    SemanticReceipt(BatchPublishReceipt),
    RepoMapReceipt(RepoMapMutationAck),
    Error(SearchPlaneIpcError),
}

const SEARCH_PLANE_INGEST_RESPONSE_VARIANTS: &[&str] = &[
    "LexicalReceipt",
    "SemanticReceipt",
    "RepoMapReceipt",
    "Error",
];

impl Serialize for SearchPlaneIngestIpcResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::LexicalReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                0,
                "LexicalReceipt",
                payload,
            ),
            Self::SemanticReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                1,
                "SemanticReceipt",
                payload,
            ),
            Self::RepoMapReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                2,
                "RepoMapReceipt",
                payload,
            ),
            Self::Error(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                3,
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
            "LexicalReceipt" => Ok(SearchPlaneIngestIpcResponse::LexicalReceipt(
                variant.newtype_variant()?,
            )),
            "SemanticReceipt" => Ok(SearchPlaneIngestIpcResponse::SemanticReceipt(
                variant.newtype_variant()?,
            )),
            "RepoMapReceipt" => Ok(SearchPlaneIngestIpcResponse::RepoMapReceipt(
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

// =============================================================================
// Local byte-buffer helpers
// =============================================================================
//
// `manifest_payload` is a `Vec<u8>` blob. Default serde on `Vec<u8>` emits a
// CBOR array of integers; we want CBOR major type 2 (byte string) instead so
// the wire stays tight and matches the existing channel-op payload encoding
// (see `crate::channel::ops::serde_bytes_helper`). The two byte-buffer types
// below produce that shape without pulling in the `serde_bytes` crate.

struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    const fn new(value: &'a [u8]) -> Self {
        Self(value)
    }
}

impl Serialize for Bytes<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(self.0)
    }
}

struct ByteBuf(Vec<u8>);

impl ByteBuf {
    fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

struct ByteBufVisitor;

impl<'de> Visitor<'de> for ByteBufVisitor {
    type Value = ByteBuf;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a byte buffer")
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(ByteBuf(v.to_vec()))
    }

    fn visit_borrowed_bytes<E>(self, v: &'de [u8]) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(ByteBuf(v.to_vec()))
    }

    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(ByteBuf(v))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        // Tolerate decoders that surface `bytes` as a sequence of small
        // integers (CBOR major type 4). Mirrors the existing
        // `serde_bytes_helper` impl in `crate::channel::ops`.
        let mut out: Vec<u8> = seq.size_hint().map_or_else(Vec::new, Vec::with_capacity);
        while let Some(elem) = seq.next_element::<u16>()? {
            let byte = u8::try_from(elem).map_err(|_err| {
                <A::Error as de::Error>::invalid_value(
                    de::Unexpected::Unsigned(u64::from(elem)),
                    &"a byte value in [0, 255]",
                )
            })?;
            out.push(byte);
        }
        Ok(ByteBuf(out))
    }
}

impl<'de> Deserialize<'de> for ByteBuf {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_byte_buf(ByteBufVisitor)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "serde roundtrip tests use assert_eq! for compact proof"
)]
#[expect(
    clippy::unwrap_used,
    reason = "unknown-tag unit test asserts the deserialize error path directly"
)]
mod tests {
    use super::*;
    use crate::lex::LangId;
    use crate::{ChunkRecord, EmbeddingRecord, RepoRelativePath};

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut buf: Vec<u8> = Vec::new();
        ciborium::into_writer(value, &mut buf)?;
        Ok(buf)
    }

    fn decode<T>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>>
    where
        T: for<'de> Deserialize<'de>,
    {
        Ok(ciborium::from_reader(bytes)?)
    }

    fn fixture_repo_id() -> RepoId {
        RepoId::new("repo")
    }

    fn fixture_revision_id() -> RevisionId {
        RevisionId::new("rev")
    }

    fn fixture_generation() -> ManifestGeneration {
        ManifestGeneration::new(7)
    }

    fn fixture_chunk_id() -> ChunkId {
        ChunkId::new("chunk-1")
    }

    fn fixture_embedding_id() -> EmbeddingId {
        EmbeddingId::new("embedding-1")
    }

    fn fixture_chunk_record() -> ChunkRecord {
        ChunkRecord {
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: "rust".to_string().into_boxed_str(),
            start_line: 1,
            end_line: 10,
            snippet: "fn main() {}".to_string().into_boxed_str(),
        }
    }

    fn fixture_embedding_record() -> EmbeddingRecord {
        EmbeddingRecord {
            owner_kind: "Function".to_string().into_boxed_str(),
            owner_id: "main".to_string().into_boxed_str(),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: LangId::Rust,
            symbol_kind: None,
            start_line: 1,
            end_line: 10,
            snippet: "fn main() {}".to_string().into_boxed_str(),
            vector: vec![0.1, 0.2, 0.3],
        }
    }

    fn fixture_lexical_batch() -> LexicalIngestBatch {
        LexicalIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            mode: BatchIngestMode::ReplaceGeneration,
            manifest_payload: vec![0xCA, 0xFE, 0xBA, 0xBE],
            chunks: vec![
                LexicalChunkMutation::Upsert(LexicalChunkUpsert {
                    chunk_id: fixture_chunk_id(),
                    record: fixture_chunk_record(),
                }),
                LexicalChunkMutation::Delete(LexicalChunkDelete {
                    chunk_id: fixture_chunk_id(),
                }),
            ],
            symbols: vec![],
            seal: true,
        }
    }

    fn fixture_semantic_batch() -> SemanticIngestBatch {
        SemanticIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            mode: BatchIngestMode::Delta,
            manifest_payload: vec![],
            embeddings: vec![
                SemanticEmbeddingMutation::Upsert(SemanticEmbeddingUpsert {
                    embedding_id: fixture_embedding_id(),
                    record: fixture_embedding_record(),
                }),
                SemanticEmbeddingMutation::Delete(SemanticEmbeddingDelete {
                    embedding_id: fixture_embedding_id(),
                }),
            ],
            seal: false,
        }
    }

    #[test]
    fn batch_ingest_mode_round_trip() -> TestRes {
        for mode in [BatchIngestMode::ReplaceGeneration, BatchIngestMode::Delta] {
            let bytes = encode(&mode)?;
            let decoded: BatchIngestMode = decode(&bytes)?;
            assert_eq!(decoded, mode);
        }
        Ok(())
    }

    #[test]
    fn lexical_chunk_mutation_round_trip() -> TestRes {
        let mutation = LexicalChunkMutation::Upsert(LexicalChunkUpsert {
            chunk_id: fixture_chunk_id(),
            record: fixture_chunk_record(),
        });
        let bytes = encode(&mutation)?;
        let decoded: LexicalChunkMutation = decode(&bytes)?;
        assert_eq!(decoded, mutation);

        let mutation = LexicalChunkMutation::Delete(LexicalChunkDelete {
            chunk_id: fixture_chunk_id(),
        });
        let bytes = encode(&mutation)?;
        let decoded: LexicalChunkMutation = decode(&bytes)?;
        assert_eq!(decoded, mutation);
        Ok(())
    }

    #[test]
    fn semantic_embedding_mutation_round_trip() -> TestRes {
        let mutation = SemanticEmbeddingMutation::Upsert(SemanticEmbeddingUpsert {
            embedding_id: fixture_embedding_id(),
            record: fixture_embedding_record(),
        });
        let bytes = encode(&mutation)?;
        let decoded: SemanticEmbeddingMutation = decode(&bytes)?;
        assert_eq!(decoded, mutation);
        Ok(())
    }

    #[test]
    fn lexical_ingest_batch_round_trip() -> TestRes {
        let batch = fixture_lexical_batch();
        let bytes = encode(&batch)?;
        let decoded: LexicalIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    fn semantic_ingest_batch_round_trip() -> TestRes {
        let batch = fixture_semantic_batch();
        let bytes = encode(&batch)?;
        let decoded: SemanticIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    fn batch_publish_receipt_round_trip() -> TestRes {
        let receipt = BatchPublishReceipt {
            first_seq: Some(ChannelSeq::new(1)),
            last_seq: Some(ChannelSeq::new(42)),
            sealed: true,
        };
        let bytes = encode(&receipt)?;
        let decoded: BatchPublishReceipt = decode(&bytes)?;
        assert_eq!(decoded, receipt);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_request_envelope_round_trip_lexical() -> TestRes {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 1,
            payload: SearchPlaneIngestIpcRequest::PublishLexicalBatch(fixture_lexical_batch()),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_request_envelope_round_trip_semantic() -> TestRes {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 2,
            payload: SearchPlaneIngestIpcRequest::PublishSemanticBatch(fixture_semantic_batch()),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_response_envelope_round_trip_receipt() -> TestRes {
        let envelope = SearchPlaneIngestIpcResponseEnvelope {
            request_id: 3,
            payload: SearchPlaneIngestIpcResponse::LexicalReceipt(BatchPublishReceipt {
                first_seq: Some(ChannelSeq::new(0)),
                last_seq: Some(ChannelSeq::new(2)),
                sealed: true,
            }),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_response_envelope_round_trip_error() -> TestRes {
        let envelope = SearchPlaneIngestIpcResponseEnvelope {
            request_id: 4,
            payload: SearchPlaneIngestIpcResponse::Error(SearchPlaneIpcError {
                code: "lexical_publish_failed".to_string(),
                message: "channel write rejected".to_string(),
            }),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn unknown_batch_ingest_mode_tag_rejected() {
        let bad = serde_json::json!("Unknown");
        let err = BatchIngestMode::deserialize(bad).unwrap_err();
        assert!(err.to_string().contains("unknown variant"));
    }
}
