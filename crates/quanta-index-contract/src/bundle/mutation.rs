use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{ManifestGeneration, RepoId, RevisionId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchBundleMutationDelta {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub operations: Vec<SearchBundleMutationOp>,
}

const SEARCH_BUNDLE_MUTATION_DELTA_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "operations",
];

impl Serialize for SearchBundleMutationDelta {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchBundleMutationDelta", 4)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("operations", &self.operations)?;
        state.end()
    }
}

struct SearchBundleMutationDeltaVisitor;

impl<'de> Visitor<'de> for SearchBundleMutationDeltaVisitor {
    type Value = SearchBundleMutationDelta;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchBundleMutationDelta map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut operations: Option<Vec<SearchBundleMutationOp>> = None;
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
                "manifest_generation" => {
                    if manifest_generation.is_some() {
                        return Err(de::Error::duplicate_field("manifest_generation"));
                    }
                    manifest_generation = Some(map.next_value()?);
                }
                "operations" => {
                    if operations.is_some() {
                        return Err(de::Error::duplicate_field("operations"));
                    }
                    operations = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_BUNDLE_MUTATION_DELTA_FIELDS,
                    ));
                }
            }
        }
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let manifest_generation =
            manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?;
        let operations = operations.ok_or_else(|| de::Error::missing_field("operations"))?;
        Ok(SearchBundleMutationDelta {
            repo_id,
            revision_id,
            manifest_generation,
            operations,
        })
    }
}

impl<'de> Deserialize<'de> for SearchBundleMutationDelta {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchBundleMutationDelta",
            SEARCH_BUNDLE_MUTATION_DELTA_FIELDS,
            SearchBundleMutationDeltaVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchBundleMutationOp {
    UpsertChunk {
        chunk_identity: String,
        text_digest: String,
    },
    DeleteChunk {
        chunk_identity: String,
    },
    UpsertSymbol {
        symbol_id: String,
        symbol_digest: String,
    },
    DeleteSymbol {
        symbol_id: String,
    },
    UpsertEmbedding {
        entity_id: String,
        input_digest: String,
    },
    DeleteEmbedding {
        entity_id: String,
    },
}

impl SearchBundleMutationOp {
    const VARIANTS: &'static [&'static str] = &[
        "UpsertChunk",
        "DeleteChunk",
        "UpsertSymbol",
        "DeleteSymbol",
        "UpsertEmbedding",
        "DeleteEmbedding",
    ];

    const fn kind(&self) -> &'static str {
        match self {
            Self::UpsertChunk { .. } => "UpsertChunk",
            Self::DeleteChunk { .. } => "DeleteChunk",
            Self::UpsertSymbol { .. } => "UpsertSymbol",
            Self::DeleteSymbol { .. } => "DeleteSymbol",
            Self::UpsertEmbedding { .. } => "UpsertEmbedding",
            Self::DeleteEmbedding { .. } => "DeleteEmbedding",
        }
    }
}

struct UpsertChunkPayloadSer<'a> {
    chunk_identity: &'a str,
    text_digest: &'a str,
}

impl Serialize for UpsertChunkPayloadSer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("UpsertChunkPayload", 2)?;
        state.serialize_field("chunk_identity", self.chunk_identity)?;
        state.serialize_field("text_digest", self.text_digest)?;
        state.end()
    }
}

struct DeleteChunkPayloadSer<'a> {
    chunk_identity: &'a str,
}

impl Serialize for DeleteChunkPayloadSer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DeleteChunkPayload", 1)?;
        state.serialize_field("chunk_identity", self.chunk_identity)?;
        state.end()
    }
}

struct UpsertSymbolPayloadSer<'a> {
    symbol_id: &'a str,
    symbol_digest: &'a str,
}

impl Serialize for UpsertSymbolPayloadSer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("UpsertSymbolPayload", 2)?;
        state.serialize_field("symbol_id", self.symbol_id)?;
        state.serialize_field("symbol_digest", self.symbol_digest)?;
        state.end()
    }
}

struct DeleteSymbolPayloadSer<'a> {
    symbol_id: &'a str,
}

impl Serialize for DeleteSymbolPayloadSer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DeleteSymbolPayload", 1)?;
        state.serialize_field("symbol_id", self.symbol_id)?;
        state.end()
    }
}

struct UpsertEmbeddingPayloadSer<'a> {
    entity_id: &'a str,
    input_digest: &'a str,
}

impl Serialize for UpsertEmbeddingPayloadSer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("UpsertEmbeddingPayload", 2)?;
        state.serialize_field("entity_id", self.entity_id)?;
        state.serialize_field("input_digest", self.input_digest)?;
        state.end()
    }
}

struct DeleteEmbeddingPayloadSer<'a> {
    entity_id: &'a str,
}

impl Serialize for DeleteEmbeddingPayloadSer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DeleteEmbeddingPayload", 1)?;
        state.serialize_field("entity_id", self.entity_id)?;
        state.end()
    }
}

impl Serialize for SearchBundleMutationOp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchBundleMutationOp", 2)?;
        state.serialize_field("kind", self.kind())?;
        match self {
            Self::UpsertChunk {
                chunk_identity,
                text_digest,
            } => {
                state.serialize_field(
                    "payload",
                    &UpsertChunkPayloadSer {
                        chunk_identity,
                        text_digest,
                    },
                )?;
            }
            Self::DeleteChunk { chunk_identity } => {
                state.serialize_field("payload", &DeleteChunkPayloadSer { chunk_identity })?;
            }
            Self::UpsertSymbol {
                symbol_id,
                symbol_digest,
            } => {
                state.serialize_field(
                    "payload",
                    &UpsertSymbolPayloadSer {
                        symbol_id,
                        symbol_digest,
                    },
                )?;
            }
            Self::DeleteSymbol { symbol_id } => {
                state.serialize_field("payload", &DeleteSymbolPayloadSer { symbol_id })?;
            }
            Self::UpsertEmbedding {
                entity_id,
                input_digest,
            } => {
                state.serialize_field(
                    "payload",
                    &UpsertEmbeddingPayloadSer {
                        entity_id,
                        input_digest,
                    },
                )?;
            }
            Self::DeleteEmbedding { entity_id } => {
                state.serialize_field("payload", &DeleteEmbeddingPayloadSer { entity_id })?;
            }
        }
        state.end()
    }
}

const UPSERT_CHUNK_FIELDS: &[&str] = &["chunk_identity", "text_digest"];
const DELETE_CHUNK_FIELDS: &[&str] = &["chunk_identity"];
const UPSERT_SYMBOL_FIELDS: &[&str] = &["symbol_id", "symbol_digest"];
const DELETE_SYMBOL_FIELDS: &[&str] = &["symbol_id"];
const UPSERT_EMBEDDING_FIELDS: &[&str] = &["entity_id", "input_digest"];
const DELETE_EMBEDDING_FIELDS: &[&str] = &["entity_id"];

#[derive(Clone, Debug, Eq, PartialEq)]
struct UpsertChunkPayloadDe {
    chunk_identity: String,
    text_digest: String,
}

struct UpsertChunkPayloadVisitor;

impl<'de> Visitor<'de> for UpsertChunkPayloadVisitor {
    type Value = UpsertChunkPayloadDe;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an UpsertChunk payload map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut chunk_identity: Option<String> = None;
        let mut text_digest: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "chunk_identity" => {
                    if chunk_identity.is_some() {
                        return Err(de::Error::duplicate_field("chunk_identity"));
                    }
                    chunk_identity = Some(map.next_value()?);
                }
                "text_digest" => {
                    if text_digest.is_some() {
                        return Err(de::Error::duplicate_field("text_digest"));
                    }
                    text_digest = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, UPSERT_CHUNK_FIELDS)),
            }
        }
        let chunk_identity =
            chunk_identity.ok_or_else(|| de::Error::missing_field("chunk_identity"))?;
        let text_digest = text_digest.ok_or_else(|| de::Error::missing_field("text_digest"))?;
        Ok(UpsertChunkPayloadDe {
            chunk_identity,
            text_digest,
        })
    }
}

impl<'de> Deserialize<'de> for UpsertChunkPayloadDe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "UpsertChunkPayload",
            UPSERT_CHUNK_FIELDS,
            UpsertChunkPayloadVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DeleteChunkPayloadDe {
    chunk_identity: String,
}

struct DeleteChunkPayloadVisitor;

impl<'de> Visitor<'de> for DeleteChunkPayloadVisitor {
    type Value = DeleteChunkPayloadDe;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DeleteChunk payload map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut chunk_identity: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "chunk_identity" => {
                    if chunk_identity.is_some() {
                        return Err(de::Error::duplicate_field("chunk_identity"));
                    }
                    chunk_identity = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, DELETE_CHUNK_FIELDS)),
            }
        }
        let chunk_identity =
            chunk_identity.ok_or_else(|| de::Error::missing_field("chunk_identity"))?;
        Ok(DeleteChunkPayloadDe { chunk_identity })
    }
}

impl<'de> Deserialize<'de> for DeleteChunkPayloadDe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "DeleteChunkPayload",
            DELETE_CHUNK_FIELDS,
            DeleteChunkPayloadVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UpsertSymbolPayloadDe {
    symbol_id: String,
    symbol_digest: String,
}

struct UpsertSymbolPayloadVisitor;

impl<'de> Visitor<'de> for UpsertSymbolPayloadVisitor {
    type Value = UpsertSymbolPayloadDe;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an UpsertSymbol payload map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut symbol_id: Option<String> = None;
        let mut symbol_digest: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "symbol_id" => {
                    if symbol_id.is_some() {
                        return Err(de::Error::duplicate_field("symbol_id"));
                    }
                    symbol_id = Some(map.next_value()?);
                }
                "symbol_digest" => {
                    if symbol_digest.is_some() {
                        return Err(de::Error::duplicate_field("symbol_digest"));
                    }
                    symbol_digest = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, UPSERT_SYMBOL_FIELDS)),
            }
        }
        let symbol_id = symbol_id.ok_or_else(|| de::Error::missing_field("symbol_id"))?;
        let symbol_digest =
            symbol_digest.ok_or_else(|| de::Error::missing_field("symbol_digest"))?;
        Ok(UpsertSymbolPayloadDe {
            symbol_id,
            symbol_digest,
        })
    }
}

impl<'de> Deserialize<'de> for UpsertSymbolPayloadDe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "UpsertSymbolPayload",
            UPSERT_SYMBOL_FIELDS,
            UpsertSymbolPayloadVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DeleteSymbolPayloadDe {
    symbol_id: String,
}

struct DeleteSymbolPayloadVisitor;

impl<'de> Visitor<'de> for DeleteSymbolPayloadVisitor {
    type Value = DeleteSymbolPayloadDe;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DeleteSymbol payload map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut symbol_id: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "symbol_id" => {
                    if symbol_id.is_some() {
                        return Err(de::Error::duplicate_field("symbol_id"));
                    }
                    symbol_id = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, DELETE_SYMBOL_FIELDS)),
            }
        }
        let symbol_id = symbol_id.ok_or_else(|| de::Error::missing_field("symbol_id"))?;
        Ok(DeleteSymbolPayloadDe { symbol_id })
    }
}

impl<'de> Deserialize<'de> for DeleteSymbolPayloadDe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "DeleteSymbolPayload",
            DELETE_SYMBOL_FIELDS,
            DeleteSymbolPayloadVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UpsertEmbeddingPayloadDe {
    entity_id: String,
    input_digest: String,
}

struct UpsertEmbeddingPayloadVisitor;

impl<'de> Visitor<'de> for UpsertEmbeddingPayloadVisitor {
    type Value = UpsertEmbeddingPayloadDe;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an UpsertEmbedding payload map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut entity_id: Option<String> = None;
        let mut input_digest: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "entity_id" => {
                    if entity_id.is_some() {
                        return Err(de::Error::duplicate_field("entity_id"));
                    }
                    entity_id = Some(map.next_value()?);
                }
                "input_digest" => {
                    if input_digest.is_some() {
                        return Err(de::Error::duplicate_field("input_digest"));
                    }
                    input_digest = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, UPSERT_EMBEDDING_FIELDS)),
            }
        }
        let entity_id = entity_id.ok_or_else(|| de::Error::missing_field("entity_id"))?;
        let input_digest = input_digest.ok_or_else(|| de::Error::missing_field("input_digest"))?;
        Ok(UpsertEmbeddingPayloadDe {
            entity_id,
            input_digest,
        })
    }
}

impl<'de> Deserialize<'de> for UpsertEmbeddingPayloadDe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "UpsertEmbeddingPayload",
            UPSERT_EMBEDDING_FIELDS,
            UpsertEmbeddingPayloadVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DeleteEmbeddingPayloadDe {
    entity_id: String,
}

struct DeleteEmbeddingPayloadVisitor;

impl<'de> Visitor<'de> for DeleteEmbeddingPayloadVisitor {
    type Value = DeleteEmbeddingPayloadDe;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DeleteEmbedding payload map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut entity_id: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "entity_id" => {
                    if entity_id.is_some() {
                        return Err(de::Error::duplicate_field("entity_id"));
                    }
                    entity_id = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, DELETE_EMBEDDING_FIELDS)),
            }
        }
        let entity_id = entity_id.ok_or_else(|| de::Error::missing_field("entity_id"))?;
        Ok(DeleteEmbeddingPayloadDe { entity_id })
    }
}

impl<'de> Deserialize<'de> for DeleteEmbeddingPayloadDe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "DeleteEmbeddingPayload",
            DELETE_EMBEDDING_FIELDS,
            DeleteEmbeddingPayloadVisitor,
        )
    }
}

const SEARCH_BUNDLE_MUTATION_OP_FIELDS: &[&str] = &["kind", "payload"];

struct SearchBundleMutationOpVisitor;

impl<'de> Visitor<'de> for SearchBundleMutationOpVisitor {
    type Value = SearchBundleMutationOp;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchBundleMutationOp map with kind and payload fields")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut value: Option<SearchBundleMutationOp> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "payload" => {
                    if value.is_some() {
                        return Err(de::Error::duplicate_field("payload"));
                    }
                    let Some(current_kind) = kind.as_deref() else {
                        return Err(de::Error::custom(
                            "`kind` must appear before `payload` in SearchBundleMutationOp",
                        ));
                    };
                    let parsed = match current_kind {
                        "UpsertChunk" => {
                            let payload: UpsertChunkPayloadDe = map.next_value()?;
                            SearchBundleMutationOp::UpsertChunk {
                                chunk_identity: payload.chunk_identity,
                                text_digest: payload.text_digest,
                            }
                        }
                        "DeleteChunk" => {
                            let payload: DeleteChunkPayloadDe = map.next_value()?;
                            SearchBundleMutationOp::DeleteChunk {
                                chunk_identity: payload.chunk_identity,
                            }
                        }
                        "UpsertSymbol" => {
                            let payload: UpsertSymbolPayloadDe = map.next_value()?;
                            SearchBundleMutationOp::UpsertSymbol {
                                symbol_id: payload.symbol_id,
                                symbol_digest: payload.symbol_digest,
                            }
                        }
                        "DeleteSymbol" => {
                            let payload: DeleteSymbolPayloadDe = map.next_value()?;
                            SearchBundleMutationOp::DeleteSymbol {
                                symbol_id: payload.symbol_id,
                            }
                        }
                        "UpsertEmbedding" => {
                            let payload: UpsertEmbeddingPayloadDe = map.next_value()?;
                            SearchBundleMutationOp::UpsertEmbedding {
                                entity_id: payload.entity_id,
                                input_digest: payload.input_digest,
                            }
                        }
                        "DeleteEmbedding" => {
                            let payload: DeleteEmbeddingPayloadDe = map.next_value()?;
                            SearchBundleMutationOp::DeleteEmbedding {
                                entity_id: payload.entity_id,
                            }
                        }
                        other => {
                            return Err(de::Error::unknown_variant(
                                other,
                                SearchBundleMutationOp::VARIANTS,
                            ));
                        }
                    };
                    value = Some(parsed);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_BUNDLE_MUTATION_OP_FIELDS,
                    ));
                }
            }
        }
        value.ok_or_else(|| de::Error::missing_field("payload"))
    }
}

impl<'de> Deserialize<'de> for SearchBundleMutationOp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchBundleMutationOp",
            SEARCH_BUNDLE_MUTATION_OP_FIELDS,
            SearchBundleMutationOpVisitor,
        )
    }
}
