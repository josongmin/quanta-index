//! Typed ingest IPC contract (QI-ING-01).
//!
//! Producer / search-plane integration surface for batch publishes. The
//! producer sends a [`SearchPlaneIngestIpcRequestEnvelope`] over UDS
//! `ingest.sock`; searchd's ingest dispatcher applies the typed batch through
//! direct authority stores / builders and forwards repo-map bundles to the
//! repo-map owner. The producer never opens an internal transport adapter
//! directly.
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

use crate::lex::{
    CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, ParseTreeRecord, SymbolRecord,
};
use crate::{
    ChunkId, ChunkRecord, EmbeddingRecord, ManifestGeneration, RepoId, RepoMapMutationAck,
    RepoMapSourceBundle, RepoRelativePath, RevisionId,
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
// Search scope / lexical / semantic ingest batches
// =============================================================================

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SearchScopeSurface {
    File,
    Module,
    Chunk,
    Symbol,
}

const SEARCH_SCOPE_SURFACE_VARIANTS: &[&str] = &["File", "Module", "Chunk", "Symbol"];

impl Serialize for SearchScopeSurface {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::File => "File",
            Self::Module => "Module",
            Self::Chunk => "Chunk",
            Self::Symbol => "Symbol",
        })
    }
}

struct SearchScopeSurfaceVisitor;

impl Visitor<'_> for SearchScopeSurfaceVisitor {
    type Value = SearchScopeSurface;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchScopeSurface tag")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "File" => Ok(SearchScopeSurface::File),
            "Module" => Ok(SearchScopeSurface::Module),
            "Chunk" => Ok(SearchScopeSurface::Chunk),
            "Symbol" => Ok(SearchScopeSurface::Symbol),
            other => Err(de::Error::unknown_variant(
                other,
                SEARCH_SCOPE_SURFACE_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for SearchScopeSurface {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SearchScopeSurfaceVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SearchScopeKey {
    pub doc_surface: SearchScopeSurface,
    pub repo_relative_path: RepoRelativePath,
}

const SEARCH_SCOPE_KEY_FIELDS: &[&str] = &["doc_surface", "repo_relative_path"];

impl Serialize for SearchScopeKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchScopeKey", 2)?;
        state.serialize_field("doc_surface", &self.doc_surface)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.end()
    }
}

struct SearchScopeKeyVisitor;

impl<'de> Visitor<'de> for SearchScopeKeyVisitor {
    type Value = SearchScopeKey;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchScopeKey map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut doc_surface: Option<SearchScopeSurface> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "doc_surface" => {
                    if doc_surface.is_some() {
                        return Err(de::Error::duplicate_field("doc_surface"));
                    }
                    doc_surface = Some(map.next_value()?);
                }
                "repo_relative_path" => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, SEARCH_SCOPE_KEY_FIELDS)),
            }
        }
        Ok(SearchScopeKey {
            doc_surface: doc_surface.ok_or_else(|| de::Error::missing_field("doc_surface"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchScopeKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchScopeKey",
            SEARCH_SCOPE_KEY_FIELDS,
            SearchScopeKeyVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalReplaceScope {
    pub scope: SearchScopeKey,
    pub scope_digest: String,
    pub chunks: Vec<ChunkRecord>,
    pub symbols: Vec<SymbolRecord>,
}

const LEXICAL_REPLACE_SCOPE_FIELDS: &[&str] = &["scope", "scope_digest", "chunks", "symbols"];

impl Serialize for LexicalReplaceScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LexicalReplaceScope", 4)?;
        state.serialize_field("scope", &self.scope)?;
        state.serialize_field("scope_digest", &self.scope_digest)?;
        state.serialize_field("chunks", &self.chunks)?;
        state.serialize_field("symbols", &self.symbols)?;
        state.end()
    }
}

struct LexicalReplaceScopeVisitor;

impl<'de> Visitor<'de> for LexicalReplaceScopeVisitor {
    type Value = LexicalReplaceScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LexicalReplaceScope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut scope: Option<SearchScopeKey> = None;
        let mut scope_digest: Option<String> = None;
        let mut chunks: Option<Vec<ChunkRecord>> = None;
        let mut symbols: Option<Vec<SymbolRecord>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "scope" => {
                    if scope.is_some() {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope = Some(map.next_value()?);
                }
                "scope_digest" => {
                    if scope_digest.is_some() {
                        return Err(de::Error::duplicate_field("scope_digest"));
                    }
                    scope_digest = Some(map.next_value()?);
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
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        LEXICAL_REPLACE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(LexicalReplaceScope {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
            scope_digest: scope_digest.ok_or_else(|| de::Error::missing_field("scope_digest"))?,
            chunks: chunks.ok_or_else(|| de::Error::missing_field("chunks"))?,
            symbols: symbols.ok_or_else(|| de::Error::missing_field("symbols"))?,
        })
    }
}

impl<'de> Deserialize<'de> for LexicalReplaceScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "LexicalReplaceScope",
            LEXICAL_REPLACE_SCOPE_FIELDS,
            LexicalReplaceScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalTombstoneScope {
    pub scope: SearchScopeKey,
}

const LEXICAL_TOMBSTONE_SCOPE_FIELDS: &[&str] = &["scope"];

impl Serialize for LexicalTombstoneScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LexicalTombstoneScope", 1)?;
        state.serialize_field("scope", &self.scope)?;
        state.end()
    }
}

struct LexicalTombstoneScopeVisitor;

impl<'de> Visitor<'de> for LexicalTombstoneScopeVisitor {
    type Value = LexicalTombstoneScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LexicalTombstoneScope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut scope: Option<SearchScopeKey> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "scope" => {
                    if scope.is_some() {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        LEXICAL_TOMBSTONE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(LexicalTombstoneScope {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
        })
    }
}

impl<'de> Deserialize<'de> for LexicalTombstoneScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "LexicalTombstoneScope",
            LEXICAL_TOMBSTONE_SCOPE_FIELDS,
            LexicalTombstoneScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub base_generation: Option<ManifestGeneration>,
    pub manifest_digest: String,
    pub batch_digest: String,
    pub mode: BatchIngestMode,
    pub bundle_payload: Option<Vec<u8>>,
    pub replace_scopes: Vec<LexicalReplaceScope>,
    pub tombstone_scopes: Vec<LexicalTombstoneScope>,
    pub seal: bool,
}

const LEXICAL_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "base_generation",
    "manifest_digest",
    "batch_digest",
    "mode",
    "bundle_payload",
    "replace_scopes",
    "tombstone_scopes",
    "seal",
];

impl Serialize for LexicalIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LexicalIngestBatch", 11)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("base_generation", &self.base_generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("bundle_payload", &self.bundle_payload)?;
        state.serialize_field("replace_scopes", &self.replace_scopes)?;
        state.serialize_field("tombstone_scopes", &self.tombstone_scopes)?;
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
        let mut base_generation: Option<Option<ManifestGeneration>> = None;
        let mut manifest_digest: Option<String> = None;
        let mut batch_digest: Option<String> = None;
        let mut mode: Option<BatchIngestMode> = None;
        let mut bundle_payload: Option<Option<Vec<u8>>> = None;
        let mut replace_scopes: Option<Vec<LexicalReplaceScope>> = None;
        let mut tombstone_scopes: Option<Vec<LexicalTombstoneScope>> = None;
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
                "base_generation" => {
                    if base_generation.is_some() {
                        return Err(de::Error::duplicate_field("base_generation"));
                    }
                    base_generation = Some(map.next_value()?);
                }
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                "batch_digest" => {
                    if batch_digest.is_some() {
                        return Err(de::Error::duplicate_field("batch_digest"));
                    }
                    batch_digest = Some(map.next_value()?);
                }
                "mode" => {
                    if mode.is_some() {
                        return Err(de::Error::duplicate_field("mode"));
                    }
                    mode = Some(map.next_value()?);
                }
                "bundle_payload" => {
                    if bundle_payload.is_some() {
                        return Err(de::Error::duplicate_field("bundle_payload"));
                    }
                    bundle_payload = Some(map.next_value()?);
                }
                "replace_scopes" => {
                    if replace_scopes.is_some() {
                        return Err(de::Error::duplicate_field("replace_scopes"));
                    }
                    replace_scopes = Some(map.next_value()?);
                }
                "tombstone_scopes" => {
                    if tombstone_scopes.is_some() {
                        return Err(de::Error::duplicate_field("tombstone_scopes"));
                    }
                    tombstone_scopes = Some(map.next_value()?);
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
            base_generation: base_generation
                .ok_or_else(|| de::Error::missing_field("base_generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            mode: mode.ok_or_else(|| de::Error::missing_field("mode"))?,
            bundle_payload: bundle_payload.unwrap_or(None),
            replace_scopes: replace_scopes
                .ok_or_else(|| de::Error::missing_field("replace_scopes"))?,
            tombstone_scopes: tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("tombstone_scopes"))?,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum EmbeddingNormalization {
    None,
    L2Unit,
}

const EMBEDDING_NORMALIZATION_VARIANTS: &[&str] = &["None", "L2Unit"];

impl Serialize for EmbeddingNormalization {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::None => "None",
            Self::L2Unit => "L2Unit",
        })
    }
}

struct EmbeddingNormalizationVisitor;

impl Visitor<'_> for EmbeddingNormalizationVisitor {
    type Value = EmbeddingNormalization;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an EmbeddingNormalization tag")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "None" => Ok(EmbeddingNormalization::None),
            "L2Unit" => Ok(EmbeddingNormalization::L2Unit),
            other => Err(de::Error::unknown_variant(
                other,
                EMBEDDING_NORMALIZATION_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for EmbeddingNormalization {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(EmbeddingNormalizationVisitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum EmbeddingDistanceMetric {
    Cosine,
    Dot,
    Euclidean,
}

const EMBEDDING_DISTANCE_METRIC_VARIANTS: &[&str] = &["Cosine", "Dot", "Euclidean"];

impl Serialize for EmbeddingDistanceMetric {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Cosine => "Cosine",
            Self::Dot => "Dot",
            Self::Euclidean => "Euclidean",
        })
    }
}

struct EmbeddingDistanceMetricVisitor;

impl Visitor<'_> for EmbeddingDistanceMetricVisitor {
    type Value = EmbeddingDistanceMetric;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an EmbeddingDistanceMetric tag")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "Cosine" => Ok(EmbeddingDistanceMetric::Cosine),
            "Dot" => Ok(EmbeddingDistanceMetric::Dot),
            "Euclidean" => Ok(EmbeddingDistanceMetric::Euclidean),
            other => Err(de::Error::unknown_variant(
                other,
                EMBEDDING_DISTANCE_METRIC_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for EmbeddingDistanceMetric {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(EmbeddingDistanceMetricVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddingModelContract {
    pub model_id: Box<str>,
    pub model_version: Option<Box<str>>,
    pub dimension: u32,
    pub normalization: EmbeddingNormalization,
    pub distance_metric: EmbeddingDistanceMetric,
    pub policy_digest: Box<str>,
    pub view_policy_digest: Option<Box<str>>,
}

const EMBEDDING_MODEL_CONTRACT_FIELDS: &[&str] = &[
    "model_id",
    "model_version",
    "dimension",
    "normalization",
    "distance_metric",
    "policy_digest",
    "view_policy_digest",
];

impl Serialize for EmbeddingModelContract {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("EmbeddingModelContract", 7)?;
        state.serialize_field("model_id", self.model_id.as_ref())?;
        state.serialize_field("model_version", &self.model_version.as_deref())?;
        state.serialize_field("dimension", &self.dimension)?;
        state.serialize_field("normalization", &self.normalization)?;
        state.serialize_field("distance_metric", &self.distance_metric)?;
        state.serialize_field("policy_digest", self.policy_digest.as_ref())?;
        state.serialize_field("view_policy_digest", &self.view_policy_digest.as_deref())?;
        state.end()
    }
}

struct EmbeddingModelContractVisitor;

impl<'de> Visitor<'de> for EmbeddingModelContractVisitor {
    type Value = EmbeddingModelContract;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an EmbeddingModelContract map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut model_id: Option<String> = None;
        let mut model_version: Option<Option<String>> = None;
        let mut dimension: Option<u32> = None;
        let mut normalization: Option<EmbeddingNormalization> = None;
        let mut distance_metric: Option<EmbeddingDistanceMetric> = None;
        let mut policy_digest: Option<String> = None;
        let mut view_policy_digest: Option<Option<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "model_id" => {
                    if model_id.is_some() {
                        return Err(de::Error::duplicate_field("model_id"));
                    }
                    model_id = Some(map.next_value()?);
                }
                "model_version" => {
                    if model_version.is_some() {
                        return Err(de::Error::duplicate_field("model_version"));
                    }
                    model_version = Some(map.next_value()?);
                }
                "dimension" => {
                    if dimension.is_some() {
                        return Err(de::Error::duplicate_field("dimension"));
                    }
                    dimension = Some(map.next_value()?);
                }
                "normalization" => {
                    if normalization.is_some() {
                        return Err(de::Error::duplicate_field("normalization"));
                    }
                    normalization = Some(map.next_value()?);
                }
                "distance_metric" => {
                    if distance_metric.is_some() {
                        return Err(de::Error::duplicate_field("distance_metric"));
                    }
                    distance_metric = Some(map.next_value()?);
                }
                "policy_digest" => {
                    if policy_digest.is_some() {
                        return Err(de::Error::duplicate_field("policy_digest"));
                    }
                    policy_digest = Some(map.next_value()?);
                }
                "view_policy_digest" => {
                    if view_policy_digest.is_some() {
                        return Err(de::Error::duplicate_field("view_policy_digest"));
                    }
                    view_policy_digest = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        EMBEDDING_MODEL_CONTRACT_FIELDS,
                    ));
                }
            }
        }
        let dimension = dimension.ok_or_else(|| de::Error::missing_field("dimension"))?;
        if dimension == 0 {
            return Err(de::Error::invalid_value(
                de::Unexpected::Unsigned(0),
                &"a non-zero embedding dimension",
            ));
        }
        Ok(EmbeddingModelContract {
            model_id: model_id
                .ok_or_else(|| de::Error::missing_field("model_id"))?
                .into_boxed_str(),
            model_version: model_version
                .ok_or_else(|| de::Error::missing_field("model_version"))?
                .map(String::into_boxed_str),
            dimension,
            normalization: normalization
                .ok_or_else(|| de::Error::missing_field("normalization"))?,
            distance_metric: distance_metric
                .ok_or_else(|| de::Error::missing_field("distance_metric"))?,
            policy_digest: policy_digest
                .ok_or_else(|| de::Error::missing_field("policy_digest"))?
                .into_boxed_str(),
            view_policy_digest: view_policy_digest
                .ok_or_else(|| de::Error::missing_field("view_policy_digest"))?
                .map(String::into_boxed_str),
        })
    }
}

impl<'de> Deserialize<'de> for EmbeddingModelContract {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "EmbeddingModelContract",
            EMBEDDING_MODEL_CONTRACT_FIELDS,
            EmbeddingModelContractVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticReplaceScope {
    pub scope: SearchScopeKey,
    pub scope_digest: String,
    pub embeddings: Vec<EmbeddingRecord>,
}

const SEMANTIC_REPLACE_SCOPE_FIELDS: &[&str] = &["scope", "scope_digest", "embeddings"];

impl Serialize for SemanticReplaceScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticReplaceScope", 3)?;
        state.serialize_field("scope", &self.scope)?;
        state.serialize_field("scope_digest", &self.scope_digest)?;
        state.serialize_field("embeddings", &self.embeddings)?;
        state.end()
    }
}

struct SemanticReplaceScopeVisitor;

impl<'de> Visitor<'de> for SemanticReplaceScopeVisitor {
    type Value = SemanticReplaceScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticReplaceScope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut scope: Option<SearchScopeKey> = None;
        let mut scope_digest: Option<String> = None;
        let mut embeddings: Option<Vec<EmbeddingRecord>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "scope" => {
                    if scope.is_some() {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope = Some(map.next_value()?);
                }
                "scope_digest" => {
                    if scope_digest.is_some() {
                        return Err(de::Error::duplicate_field("scope_digest"));
                    }
                    scope_digest = Some(map.next_value()?);
                }
                "embeddings" => {
                    if embeddings.is_some() {
                        return Err(de::Error::duplicate_field("embeddings"));
                    }
                    embeddings = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_REPLACE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticReplaceScope {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
            scope_digest: scope_digest.ok_or_else(|| de::Error::missing_field("scope_digest"))?,
            embeddings: embeddings.ok_or_else(|| de::Error::missing_field("embeddings"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticReplaceScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticReplaceScope",
            SEMANTIC_REPLACE_SCOPE_FIELDS,
            SemanticReplaceScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticTombstoneScope {
    pub scope: SearchScopeKey,
}

const SEMANTIC_TOMBSTONE_SCOPE_FIELDS: &[&str] = &["scope"];

impl Serialize for SemanticTombstoneScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticTombstoneScope", 1)?;
        state.serialize_field("scope", &self.scope)?;
        state.end()
    }
}

struct SemanticTombstoneScopeVisitor;

impl<'de> Visitor<'de> for SemanticTombstoneScopeVisitor {
    type Value = SemanticTombstoneScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticTombstoneScope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut scope: Option<SearchScopeKey> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "scope" => {
                    if scope.is_some() {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_TOMBSTONE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticTombstoneScope {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticTombstoneScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticTombstoneScope",
            SEMANTIC_TOMBSTONE_SCOPE_FIELDS,
            SemanticTombstoneScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub base_generation: Option<ManifestGeneration>,
    pub manifest_digest: String,
    pub batch_digest: String,
    pub mode: BatchIngestMode,
    pub model_contract: EmbeddingModelContract,
    pub replace_scopes: Vec<SemanticReplaceScope>,
    pub tombstone_scopes: Vec<SemanticTombstoneScope>,
    pub seal: bool,
}

const SEMANTIC_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "base_generation",
    "manifest_digest",
    "batch_digest",
    "mode",
    "model_contract",
    "replace_scopes",
    "tombstone_scopes",
    "seal",
];

impl Serialize for SemanticIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticIngestBatch", 11)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("base_generation", &self.base_generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("model_contract", &self.model_contract)?;
        state.serialize_field("replace_scopes", &self.replace_scopes)?;
        state.serialize_field("tombstone_scopes", &self.tombstone_scopes)?;
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
        let mut base_generation: Option<Option<ManifestGeneration>> = None;
        let mut manifest_digest: Option<String> = None;
        let mut batch_digest: Option<String> = None;
        let mut mode: Option<BatchIngestMode> = None;
        let mut model_contract: Option<EmbeddingModelContract> = None;
        let mut replace_scopes: Option<Vec<SemanticReplaceScope>> = None;
        let mut tombstone_scopes: Option<Vec<SemanticTombstoneScope>> = None;
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
                "base_generation" => {
                    if base_generation.is_some() {
                        return Err(de::Error::duplicate_field("base_generation"));
                    }
                    base_generation = Some(map.next_value()?);
                }
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                "batch_digest" => {
                    if batch_digest.is_some() {
                        return Err(de::Error::duplicate_field("batch_digest"));
                    }
                    batch_digest = Some(map.next_value()?);
                }
                "mode" => {
                    if mode.is_some() {
                        return Err(de::Error::duplicate_field("mode"));
                    }
                    mode = Some(map.next_value()?);
                }
                "model_contract" => {
                    if model_contract.is_some() {
                        return Err(de::Error::duplicate_field("model_contract"));
                    }
                    model_contract = Some(map.next_value()?);
                }
                "replace_scopes" => {
                    if replace_scopes.is_some() {
                        return Err(de::Error::duplicate_field("replace_scopes"));
                    }
                    replace_scopes = Some(map.next_value()?);
                }
                "tombstone_scopes" => {
                    if tombstone_scopes.is_some() {
                        return Err(de::Error::duplicate_field("tombstone_scopes"));
                    }
                    tombstone_scopes = Some(map.next_value()?);
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
            base_generation: base_generation
                .ok_or_else(|| de::Error::missing_field("base_generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            mode: mode.ok_or_else(|| de::Error::missing_field("mode"))?,
            model_contract: model_contract
                .ok_or_else(|| de::Error::missing_field("model_contract"))?,
            replace_scopes: replace_scopes
                .ok_or_else(|| de::Error::missing_field("replace_scopes"))?,
            tombstone_scopes: tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("tombstone_scopes"))?,
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
// History ingest batch
// =============================================================================

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryRefUpsert {
    pub name: Box<str>,
    pub sha: CommitSha,
}

const HISTORY_REF_UPSERT_FIELDS: &[&str] = &["name", "sha"];

impl Serialize for HistoryRefUpsert {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HistoryRefUpsert", 2)?;
        state.serialize_field("name", self.name.as_ref())?;
        state.serialize_field("sha", &self.sha)?;
        state.end()
    }
}

struct HistoryRefUpsertVisitor;

impl<'de> Visitor<'de> for HistoryRefUpsertVisitor {
    type Value = HistoryRefUpsert;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryRefUpsert map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut name: Option<String> = None;
        let mut sha: Option<CommitSha> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "name" => {
                    if name.is_some() {
                        return Err(de::Error::duplicate_field("name"));
                    }
                    name = Some(map.next_value()?);
                }
                "sha" => {
                    if sha.is_some() {
                        return Err(de::Error::duplicate_field("sha"));
                    }
                    sha = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, HISTORY_REF_UPSERT_FIELDS)),
            }
        }
        Ok(HistoryRefUpsert {
            name: name
                .ok_or_else(|| de::Error::missing_field("name"))?
                .into_boxed_str(),
            sha: sha.ok_or_else(|| de::Error::missing_field("sha"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HistoryRefUpsert {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HistoryRefUpsert",
            HISTORY_REF_UPSERT_FIELDS,
            HistoryRefUpsertVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryRefDelete {
    pub name: Box<str>,
}

const HISTORY_REF_DELETE_FIELDS: &[&str] = &["name"];

impl Serialize for HistoryRefDelete {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HistoryRefDelete", 1)?;
        state.serialize_field("name", self.name.as_ref())?;
        state.end()
    }
}

struct HistoryRefDeleteVisitor;

impl<'de> Visitor<'de> for HistoryRefDeleteVisitor {
    type Value = HistoryRefDelete;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryRefDelete map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut name: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "name" => {
                    if name.is_some() {
                        return Err(de::Error::duplicate_field("name"));
                    }
                    name = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, HISTORY_REF_DELETE_FIELDS)),
            }
        }
        Ok(HistoryRefDelete {
            name: name
                .ok_or_else(|| de::Error::missing_field("name"))?
                .into_boxed_str(),
        })
    }
}

impl<'de> Deserialize<'de> for HistoryRefDelete {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HistoryRefDelete",
            HISTORY_REF_DELETE_FIELDS,
            HistoryRefDeleteVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistoryRefMutation {
    Upsert(HistoryRefUpsert),
    Delete(HistoryRefDelete),
}

const HISTORY_REF_MUTATION_VARIANTS: &[&str] = &["Upsert", "Delete"];

impl Serialize for HistoryRefMutation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Upsert(payload) => {
                serializer.serialize_newtype_variant("HistoryRefMutation", 0, "Upsert", payload)
            }
            Self::Delete(payload) => {
                serializer.serialize_newtype_variant("HistoryRefMutation", 1, "Delete", payload)
            }
        }
    }
}

struct HistoryRefMutationVisitor;

impl<'de> Visitor<'de> for HistoryRefMutationVisitor {
    type Value = HistoryRefMutation;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryRefMutation enum")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::EnumAccess<'de>,
    {
        let (tag, variant) = data.variant::<String>()?;
        match tag.as_str() {
            "Upsert" => Ok(HistoryRefMutation::Upsert(variant.newtype_variant()?)),
            "Delete" => Ok(HistoryRefMutation::Delete(variant.newtype_variant()?)),
            other => Err(de::Error::unknown_variant(
                other,
                HISTORY_REF_MUTATION_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for HistoryRefMutation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_enum(
            "HistoryRefMutation",
            HISTORY_REF_MUTATION_VARIANTS,
            HistoryRefMutationVisitor,
        )
    }
}

pub type HistoryTagUpsert = HistoryRefUpsert;
pub type HistoryTagDelete = HistoryRefDelete;
pub type HistoryTagMutation = HistoryRefMutation;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryDiffHunkUpsert {
    pub commit_sha: CommitSha,
    pub file_path: Box<str>,
    pub record: DiffHunkRecord,
}

const HISTORY_DIFF_HUNK_UPSERT_FIELDS: &[&str] = &["commit_sha", "file_path", "record"];

impl Serialize for HistoryDiffHunkUpsert {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HistoryDiffHunkUpsert", 3)?;
        state.serialize_field("commit_sha", &self.commit_sha)?;
        state.serialize_field("file_path", self.file_path.as_ref())?;
        state.serialize_field("record", &self.record)?;
        state.end()
    }
}

struct HistoryDiffHunkUpsertVisitor;

impl<'de> Visitor<'de> for HistoryDiffHunkUpsertVisitor {
    type Value = HistoryDiffHunkUpsert;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryDiffHunkUpsert map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut commit_sha: Option<CommitSha> = None;
        let mut file_path: Option<String> = None;
        let mut record: Option<DiffHunkRecord> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "commit_sha" => {
                    if commit_sha.is_some() {
                        return Err(de::Error::duplicate_field("commit_sha"));
                    }
                    commit_sha = Some(map.next_value()?);
                }
                "file_path" => {
                    if file_path.is_some() {
                        return Err(de::Error::duplicate_field("file_path"));
                    }
                    file_path = Some(map.next_value()?);
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
                        HISTORY_DIFF_HUNK_UPSERT_FIELDS,
                    ));
                }
            }
        }
        Ok(HistoryDiffHunkUpsert {
            commit_sha: commit_sha.ok_or_else(|| de::Error::missing_field("commit_sha"))?,
            file_path: file_path
                .ok_or_else(|| de::Error::missing_field("file_path"))?
                .into_boxed_str(),
            record: record.ok_or_else(|| de::Error::missing_field("record"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HistoryDiffHunkUpsert {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HistoryDiffHunkUpsert",
            HISTORY_DIFF_HUNK_UPSERT_FIELDS,
            HistoryDiffHunkUpsertVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub manifest_digest: Option<String>,
    pub batch_digest: String,
    pub commits: Vec<CommitRecord>,
    pub refs: Vec<HistoryRefMutation>,
    pub tags: Vec<HistoryTagMutation>,
    pub diff_hunks: Vec<HistoryDiffHunkUpsert>,
}

const HISTORY_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "manifest_digest",
    "batch_digest",
    "commits",
    "refs",
    "tags",
    "diff_hunks",
];

impl Serialize for HistoryIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HistoryIngestBatch", 9)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("commits", &self.commits)?;
        state.serialize_field("refs", &self.refs)?;
        state.serialize_field("tags", &self.tags)?;
        state.serialize_field("diff_hunks", &self.diff_hunks)?;
        state.end()
    }
}

struct HistoryIngestBatchVisitor;

impl<'de> Visitor<'de> for HistoryIngestBatchVisitor {
    type Value = HistoryIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut manifest_digest: Option<Option<String>> = None;
        let mut batch_digest: Option<String> = None;
        let mut commits: Option<Vec<CommitRecord>> = None;
        let mut refs: Option<Vec<HistoryRefMutation>> = None;
        let mut tags: Option<Vec<HistoryTagMutation>> = None;
        let mut diff_hunks: Option<Vec<HistoryDiffHunkUpsert>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "manifest_digest" => manifest_digest = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "commits" => commits = Some(map.next_value()?),
                "refs" => refs = Some(map.next_value()?),
                "tags" => tags = Some(map.next_value()?),
                "diff_hunks" => diff_hunks = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, HISTORY_INGEST_BATCH_FIELDS)),
            }
        }
        Ok(HistoryIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            commits: commits.ok_or_else(|| de::Error::missing_field("commits"))?,
            refs: refs.ok_or_else(|| de::Error::missing_field("refs"))?,
            tags: tags.ok_or_else(|| de::Error::missing_field("tags"))?,
            diff_hunks: diff_hunks.ok_or_else(|| de::Error::missing_field("diff_hunks"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HistoryIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HistoryIngestBatch",
            HISTORY_INGEST_BATCH_FIELDS,
            HistoryIngestBatchVisitor,
        )
    }
}

// =============================================================================
// Runtime dirty ingest batch
// =============================================================================

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyDelete {
    pub doc_id: ChunkId,
}

const DIRTY_DELETE_FIELDS: &[&str] = &["doc_id"];

impl Serialize for DirtyDelete {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DirtyDelete", 1)?;
        state.serialize_field("doc_id", &self.doc_id)?;
        state.end()
    }
}

struct DirtyDeleteVisitor;

impl<'de> Visitor<'de> for DirtyDeleteVisitor {
    type Value = DirtyDelete;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DirtyDelete map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut doc_id: Option<ChunkId> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "doc_id" => doc_id = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, DIRTY_DELETE_FIELDS)),
            }
        }
        Ok(DirtyDelete {
            doc_id: doc_id.ok_or_else(|| de::Error::missing_field("doc_id"))?,
        })
    }
}

impl<'de> Deserialize<'de> for DirtyDelete {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("DirtyDelete", DIRTY_DELETE_FIELDS, DirtyDeleteVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirtyMutation {
    Upsert(DirtyRecord),
    Delete(DirtyDelete),
}

const DIRTY_MUTATION_VARIANTS: &[&str] = &["Upsert", "Delete"];

impl Serialize for DirtyMutation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Upsert(payload) => {
                serializer.serialize_newtype_variant("DirtyMutation", 0, "Upsert", payload)
            }
            Self::Delete(payload) => {
                serializer.serialize_newtype_variant("DirtyMutation", 1, "Delete", payload)
            }
        }
    }
}

struct DirtyMutationVisitor;

impl<'de> Visitor<'de> for DirtyMutationVisitor {
    type Value = DirtyMutation;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DirtyMutation enum")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::EnumAccess<'de>,
    {
        let (tag, variant) = data.variant::<String>()?;
        match tag.as_str() {
            "Upsert" => Ok(DirtyMutation::Upsert(variant.newtype_variant()?)),
            "Delete" => Ok(DirtyMutation::Delete(variant.newtype_variant()?)),
            other => Err(de::Error::unknown_variant(other, DIRTY_MUTATION_VARIANTS)),
        }
    }
}

impl<'de> Deserialize<'de> for DirtyMutation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_enum(
            "DirtyMutation",
            DIRTY_MUTATION_VARIANTS,
            DirtyMutationVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub overlay_epoch_ms: u64,
    pub batch_digest: String,
    pub entries: Vec<DirtyMutation>,
}

const DIRTY_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "overlay_epoch_ms",
    "batch_digest",
    "entries",
];

impl Serialize for DirtyIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DirtyIngestBatch", 6)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("overlay_epoch_ms", &self.overlay_epoch_ms)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct DirtyIngestBatchVisitor;

impl<'de> Visitor<'de> for DirtyIngestBatchVisitor {
    type Value = DirtyIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DirtyIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut overlay_epoch_ms: Option<u64> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<DirtyMutation>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "overlay_epoch_ms" => overlay_epoch_ms = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, DIRTY_INGEST_BATCH_FIELDS)),
            }
        }
        Ok(DirtyIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            overlay_epoch_ms: overlay_epoch_ms
                .ok_or_else(|| de::Error::missing_field("overlay_epoch_ms"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for DirtyIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "DirtyIngestBatch",
            DIRTY_INGEST_BATCH_FIELDS,
            DirtyIngestBatchVisitor,
        )
    }
}

// =============================================================================
// Structural ingest batch
// =============================================================================

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralTreeRecord {
    pub chunk_id: ChunkId,
    pub record: ParseTreeRecord,
}

const STRUCTURAL_TREE_RECORD_FIELDS: &[&str] = &["chunk_id", "record"];

impl Serialize for StructuralTreeRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralTreeRecord", 2)?;
        state.serialize_field("chunk_id", &self.chunk_id)?;
        state.serialize_field("record", &self.record)?;
        state.end()
    }
}

struct StructuralTreeRecordVisitor;

impl<'de> Visitor<'de> for StructuralTreeRecordVisitor {
    type Value = StructuralTreeRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a StructuralTreeRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut chunk_id: Option<ChunkId> = None;
        let mut record: Option<ParseTreeRecord> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "chunk_id" => chunk_id = Some(map.next_value()?),
                "record" => record = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        STRUCTURAL_TREE_RECORD_FIELDS,
                    ));
                }
            }
        }
        Ok(StructuralTreeRecord {
            chunk_id: chunk_id.ok_or_else(|| de::Error::missing_field("chunk_id"))?,
            record: record.ok_or_else(|| de::Error::missing_field("record"))?,
        })
    }
}

impl<'de> Deserialize<'de> for StructuralTreeRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "StructuralTreeRecord",
            STRUCTURAL_TREE_RECORD_FIELDS,
            StructuralTreeRecordVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralReplaceScope {
    pub scope: SearchScopeKey,
    pub scope_digest: String,
    pub trees: Vec<StructuralTreeRecord>,
}

const STRUCTURAL_REPLACE_SCOPE_FIELDS: &[&str] = &["scope", "scope_digest", "trees"];

impl Serialize for StructuralReplaceScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralReplaceScope", 3)?;
        state.serialize_field("scope", &self.scope)?;
        state.serialize_field("scope_digest", &self.scope_digest)?;
        state.serialize_field("trees", &self.trees)?;
        state.end()
    }
}

struct StructuralReplaceScopeVisitor;

impl<'de> Visitor<'de> for StructuralReplaceScopeVisitor {
    type Value = StructuralReplaceScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a StructuralReplaceScope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut scope: Option<SearchScopeKey> = None;
        let mut scope_digest: Option<String> = None;
        let mut trees: Option<Vec<StructuralTreeRecord>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "scope" => {
                    if scope.is_some() {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope = Some(map.next_value()?);
                }
                "scope_digest" => {
                    if scope_digest.is_some() {
                        return Err(de::Error::duplicate_field("scope_digest"));
                    }
                    scope_digest = Some(map.next_value()?);
                }
                "trees" => {
                    if trees.is_some() {
                        return Err(de::Error::duplicate_field("trees"));
                    }
                    trees = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        STRUCTURAL_REPLACE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(StructuralReplaceScope {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
            scope_digest: scope_digest.ok_or_else(|| de::Error::missing_field("scope_digest"))?,
            trees: trees.ok_or_else(|| de::Error::missing_field("trees"))?,
        })
    }
}

impl<'de> Deserialize<'de> for StructuralReplaceScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "StructuralReplaceScope",
            STRUCTURAL_REPLACE_SCOPE_FIELDS,
            StructuralReplaceScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralTombstoneScope {
    pub scope: SearchScopeKey,
}

const STRUCTURAL_TOMBSTONE_SCOPE_FIELDS: &[&str] = &["scope"];

impl Serialize for StructuralTombstoneScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralTombstoneScope", 1)?;
        state.serialize_field("scope", &self.scope)?;
        state.end()
    }
}

struct StructuralTombstoneScopeVisitor;

impl<'de> Visitor<'de> for StructuralTombstoneScopeVisitor {
    type Value = StructuralTombstoneScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a StructuralTombstoneScope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut scope: Option<SearchScopeKey> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "scope" => {
                    if scope.is_some() {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        STRUCTURAL_TOMBSTONE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(StructuralTombstoneScope {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
        })
    }
}

impl<'de> Deserialize<'de> for StructuralTombstoneScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "StructuralTombstoneScope",
            STRUCTURAL_TOMBSTONE_SCOPE_FIELDS,
            StructuralTombstoneScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub base_generation: Option<ManifestGeneration>,
    pub manifest_digest: String,
    pub batch_digest: String,
    pub mode: BatchIngestMode,
    pub replace_scopes: Vec<StructuralReplaceScope>,
    pub tombstone_scopes: Vec<StructuralTombstoneScope>,
    pub seal: bool,
}

const STRUCTURAL_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "base_generation",
    "manifest_digest",
    "batch_digest",
    "mode",
    "replace_scopes",
    "tombstone_scopes",
    "seal",
];

impl Serialize for StructuralIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralIngestBatch", 10)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("base_generation", &self.base_generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("replace_scopes", &self.replace_scopes)?;
        state.serialize_field("tombstone_scopes", &self.tombstone_scopes)?;
        state.serialize_field("seal", &self.seal)?;
        state.end()
    }
}

struct StructuralIngestBatchVisitor;

impl<'de> Visitor<'de> for StructuralIngestBatchVisitor {
    type Value = StructuralIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a StructuralIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut base_generation: Option<Option<ManifestGeneration>> = None;
        let mut manifest_digest: Option<String> = None;
        let mut batch_digest: Option<String> = None;
        let mut mode: Option<BatchIngestMode> = None;
        let mut replace_scopes: Option<Vec<StructuralReplaceScope>> = None;
        let mut tombstone_scopes: Option<Vec<StructuralTombstoneScope>> = None;
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
                "base_generation" => {
                    if base_generation.is_some() {
                        return Err(de::Error::duplicate_field("base_generation"));
                    }
                    base_generation = Some(map.next_value()?);
                }
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                "batch_digest" => {
                    if batch_digest.is_some() {
                        return Err(de::Error::duplicate_field("batch_digest"));
                    }
                    batch_digest = Some(map.next_value()?);
                }
                "mode" => {
                    if mode.is_some() {
                        return Err(de::Error::duplicate_field("mode"));
                    }
                    mode = Some(map.next_value()?);
                }
                "replace_scopes" => {
                    if replace_scopes.is_some() {
                        return Err(de::Error::duplicate_field("replace_scopes"));
                    }
                    replace_scopes = Some(map.next_value()?);
                }
                "tombstone_scopes" => {
                    if tombstone_scopes.is_some() {
                        return Err(de::Error::duplicate_field("tombstone_scopes"));
                    }
                    tombstone_scopes = Some(map.next_value()?);
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
                        STRUCTURAL_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(StructuralIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            base_generation: base_generation
                .ok_or_else(|| de::Error::missing_field("base_generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            mode: mode.ok_or_else(|| de::Error::missing_field("mode"))?,
            replace_scopes: replace_scopes
                .ok_or_else(|| de::Error::missing_field("replace_scopes"))?,
            tombstone_scopes: tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("tombstone_scopes"))?,
            seal: seal.ok_or_else(|| de::Error::missing_field("seal"))?,
        })
    }
}

impl<'de> Deserialize<'de> for StructuralIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "StructuralIngestBatch",
            STRUCTURAL_INGEST_BATCH_FIELDS,
            StructuralIngestBatchVisitor,
        )
    }
}

// =============================================================================
// BatchPublishReceipt
// =============================================================================

/// Server-side receipt for a successful batch publish.
///
/// Receipt truth is generation/materialization scoped, not channel-sequence
/// scoped. The ingest path may internally fan out to multiple storage writes,
/// but the producer-facing ack reports the generation and how many scope
/// mutations were accepted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchPublishReceipt {
    pub generation: ManifestGeneration,
    pub manifest_digest: String,
    pub accepted_replace_scopes: u32,
    pub accepted_tombstone_scopes: u32,
    pub sealed: bool,
}

const BATCH_PUBLISH_RECEIPT_FIELDS: &[&str] = &[
    "generation",
    "manifest_digest",
    "accepted_replace_scopes",
    "accepted_tombstone_scopes",
    "sealed",
];

impl Serialize for BatchPublishReceipt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("BatchPublishReceipt", 5)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("accepted_replace_scopes", &self.accepted_replace_scopes)?;
        state.serialize_field("accepted_tombstone_scopes", &self.accepted_tombstone_scopes)?;
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
        let mut generation: Option<ManifestGeneration> = None;
        let mut manifest_digest: Option<String> = None;
        let mut accepted_replace_scopes: Option<u32> = None;
        let mut accepted_tombstone_scopes: Option<u32> = None;
        let mut sealed: Option<bool> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                "accepted_replace_scopes" => {
                    if accepted_replace_scopes.is_some() {
                        return Err(de::Error::duplicate_field("accepted_replace_scopes"));
                    }
                    accepted_replace_scopes = Some(map.next_value()?);
                }
                "accepted_tombstone_scopes" => {
                    if accepted_tombstone_scopes.is_some() {
                        return Err(de::Error::duplicate_field("accepted_tombstone_scopes"));
                    }
                    accepted_tombstone_scopes = Some(map.next_value()?);
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
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            accepted_replace_scopes: accepted_replace_scopes
                .ok_or_else(|| de::Error::missing_field("accepted_replace_scopes"))?,
            accepted_tombstone_scopes: accepted_tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("accepted_tombstone_scopes"))?,
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
    #[must_use]
    pub fn empty_for(generation: ManifestGeneration, manifest_digest: impl Into<String>) -> Self {
        Self {
            generation,
            manifest_digest: manifest_digest.into(),
            accepted_replace_scopes: 0,
            accepted_tombstone_scopes: 0,
            sealed: false,
        }
    }

    pub fn accept_replace_scope(&mut self) {
        self.accepted_replace_scopes = self.accepted_replace_scopes.saturating_add(1);
    }

    pub fn accept_tombstone_scope(&mut self) {
        self.accepted_tombstone_scopes = self.accepted_tombstone_scopes.saturating_add(1);
    }

    pub fn mark_sealed(&mut self) {
        self.sealed = true;
    }
}

impl Default for BatchPublishReceipt {
    fn default() -> Self {
        Self::empty_for(ManifestGeneration::ZERO, String::new())
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
    PublishHistoryBatch(HistoryIngestBatch),
    PublishDirtyBatch(DirtyIngestBatch),
    PublishStructuralBatch(StructuralIngestBatch),
    PublishRepoMapBundle(RepoMapSourceBundle),
}

const SEARCH_PLANE_INGEST_REQUEST_VARIANTS: &[&str] = &[
    "PublishLexicalBatch",
    "PublishSemanticBatch",
    "PublishHistoryBatch",
    "PublishDirtyBatch",
    "PublishStructuralBatch",
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
            Self::PublishHistoryBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                2,
                "PublishHistoryBatch",
                payload,
            ),
            Self::PublishDirtyBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                3,
                "PublishDirtyBatch",
                payload,
            ),
            Self::PublishStructuralBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                4,
                "PublishStructuralBatch",
                payload,
            ),
            Self::PublishRepoMapBundle(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                5,
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
            "PublishHistoryBatch" => Ok(SearchPlaneIngestIpcRequest::PublishHistoryBatch(
                variant.newtype_variant()?,
            )),
            "PublishDirtyBatch" => Ok(SearchPlaneIngestIpcRequest::PublishDirtyBatch(
                variant.newtype_variant()?,
            )),
            "PublishStructuralBatch" => Ok(SearchPlaneIngestIpcRequest::PublishStructuralBatch(
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
    HistoryReceipt(BatchPublishReceipt),
    DirtyReceipt(BatchPublishReceipt),
    StructuralReceipt(BatchPublishReceipt),
    RepoMapReceipt(RepoMapMutationAck),
    Error(SearchPlaneIpcError),
}

const SEARCH_PLANE_INGEST_RESPONSE_VARIANTS: &[&str] = &[
    "LexicalReceipt",
    "SemanticReceipt",
    "HistoryReceipt",
    "DirtyReceipt",
    "StructuralReceipt",
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
            Self::HistoryReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                2,
                "HistoryReceipt",
                payload,
            ),
            Self::DirtyReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                3,
                "DirtyReceipt",
                payload,
            ),
            Self::StructuralReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                4,
                "StructuralReceipt",
                payload,
            ),
            Self::RepoMapReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                5,
                "RepoMapReceipt",
                payload,
            ),
            Self::Error(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                6,
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
            "HistoryReceipt" => Ok(SearchPlaneIngestIpcResponse::HistoryReceipt(
                variant.newtype_variant()?,
            )),
            "DirtyReceipt" => Ok(SearchPlaneIngestIpcResponse::DirtyReceipt(
                variant.newtype_variant()?,
            )),
            "StructuralReceipt" => Ok(SearchPlaneIngestIpcResponse::StructuralReceipt(
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
    use crate::lex::{
        CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, LanguageCode, ParseNode,
        ParseRoleTag, ParseTreeRecord, compute_parse_tree_source_hash,
    };
    use crate::{ChunkRecord, EmbeddingId, EmbeddingRecord, RepoRelativePath};

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
            chunk_id: fixture_chunk_id(),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: LanguageCode::new("rust").unwrap(),
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 10,
            snippet: "fn main() {}".to_string().into_boxed_str(),
            indexed_text: "fn main() {}".to_string().into_boxed_str(),
            text_digest: "text:feed".to_string().into_boxed_str(),
            shape_digest: "shape:feed".to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
        }
    }

    fn fixture_embedding_record() -> EmbeddingRecord {
        EmbeddingRecord {
            embedding_id: fixture_embedding_id(),
            owner_kind: crate::OwnerDocKind::Chunk,
            owner_id: "main".to_string().into_boxed_str(),
            source_doc_id: "doc-1".to_string().into_boxed_str(),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: LanguageCode::new("rust").unwrap(),
            symbol_kind: None,
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 10,
            snippet: "fn main() {}".to_string().into_boxed_str(),
            embedding_input_digest: "input:feed".to_string().into_boxed_str(),
            vector_digest: "vector:feed".to_string().into_boxed_str(),
            view_kind: "raw_chunk".to_string().into_boxed_str(),
            vector: vec![0.1, 0.2, 0.3],
        }
    }

    fn fixture_commit_sha() -> CommitSha {
        CommitSha::from_bytes([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef, 0x01, 0x23, 0x45, 0x67,
        ])
    }

    fn fixture_commit_record() -> CommitRecord {
        CommitRecord {
            wire_version: 1,
            sha: fixture_commit_sha(),
            parents: Vec::new(),
            author_time_ms: 11,
            committer_time_ms: 12,
            applied_at_ms: 13,
            author: "alice".to_string().into_boxed_str(),
            committer: "alice".to_string().into_boxed_str(),
            message: "fix: sample".to_string().into_boxed_str(),
            is_merge: false,
            tags: vec!["v1.0.0".to_string().into_boxed_str()],
        }
    }

    fn fixture_diff_record() -> DiffHunkRecord {
        DiffHunkRecord {
            wire_version: 1,
            hunk_header: "@@ -1,1 +1,2 @@".to_string().into_boxed_str(),
            side: crate::DiffHunkSide::After,
            added_text: "todo!".to_string().into_boxed_str(),
            removed_text: String::new().into_boxed_str(),
            touched_text: "todo!".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: 5,
        }
    }

    fn fixture_dirty_record() -> DirtyRecord {
        DirtyRecord {
            wire_version: 1,
            doc_id: fixture_chunk_id(),
            applied_at_ms: 55,
            payload_hash: [7; 32],
        }
    }

    fn fixture_parse_tree_record() -> ParseTreeRecord {
        ParseTreeRecord {
            wire_version: 1,
            lang: LanguageCode::new("rust").unwrap(),
            root: ParseNode {
                kind: "function_item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 10,
                children: Vec::new(),
            },
            source_hash: compute_parse_tree_source_hash("fn main() {}"),
            role_tag_schema_version: 1,
            role_tags: vec![ParseRoleTag {
                role: "expr".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 4,
            }],
        }
    }

    fn fixture_scope_key() -> SearchScopeKey {
        SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
        }
    }

    fn fixture_model_contract() -> EmbeddingModelContract {
        EmbeddingModelContract {
            model_id: "text-embed".to_string().into_boxed_str(),
            model_version: Some("1".to_string().into_boxed_str()),
            dimension: 3,
            normalization: EmbeddingNormalization::L2Unit,
            distance_metric: EmbeddingDistanceMetric::Cosine,
            policy_digest: "policy:feed".to_string().into_boxed_str(),
            view_policy_digest: Some("view:feed".to_string().into_boxed_str()),
        }
    }

    fn fixture_lexical_batch() -> LexicalIngestBatch {
        LexicalIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            base_generation: None,
            manifest_digest: "manifest:feed".to_string(),
            batch_digest: "batch:feed".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            replace_scopes: vec![LexicalReplaceScope {
                scope: fixture_scope_key(),
                scope_digest: "scope:feed".to_string(),
                chunks: vec![fixture_chunk_record()],
                symbols: vec![],
            }],
            tombstone_scopes: vec![LexicalTombstoneScope {
                scope: SearchScopeKey {
                    doc_surface: SearchScopeSurface::Symbol,
                    repo_relative_path: RepoRelativePath::new("src/main.rs"),
                },
            }],
            seal: true,
        }
    }

    fn fixture_semantic_batch() -> SemanticIngestBatch {
        SemanticIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            base_generation: Some(ManifestGeneration::new(6)),
            manifest_digest: "manifest:feed".to_string(),
            batch_digest: "batch:feed".to_string(),
            mode: BatchIngestMode::Delta,
            model_contract: fixture_model_contract(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: fixture_scope_key(),
                scope_digest: "scope:feed".to_string(),
                embeddings: vec![fixture_embedding_record()],
            }],
            tombstone_scopes: vec![SemanticTombstoneScope {
                scope: SearchScopeKey {
                    doc_surface: SearchScopeSurface::Chunk,
                    repo_relative_path: RepoRelativePath::new("src/old.rs"),
                },
            }],
            seal: false,
        }
    }

    fn fixture_history_batch() -> HistoryIngestBatch {
        HistoryIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            manifest_digest: Some("manifest:feed".to_string()),
            batch_digest: "batch:feed".to_string(),
            commits: vec![fixture_commit_record()],
            refs: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: "refs/heads/main".to_string().into_boxed_str(),
                sha: fixture_commit_sha(),
            })],
            tags: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: "v1.0.0".to_string().into_boxed_str(),
                sha: fixture_commit_sha(),
            })],
            diff_hunks: vec![HistoryDiffHunkUpsert {
                commit_sha: fixture_commit_sha(),
                file_path: "src/lib.rs".to_string().into_boxed_str(),
                record: fixture_diff_record(),
            }],
        }
    }

    fn fixture_dirty_batch() -> DirtyIngestBatch {
        DirtyIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            overlay_epoch_ms: 1_717_171_717_000,
            batch_digest: "dirty-batch:feed".to_string(),
            entries: vec![
                DirtyMutation::Upsert(fixture_dirty_record()),
                DirtyMutation::Delete(DirtyDelete {
                    doc_id: ChunkId::new("chunk-evict"),
                }),
            ],
        }
    }

    fn fixture_structural_batch() -> StructuralIngestBatch {
        StructuralIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            base_generation: Some(ManifestGeneration::new(6)),
            manifest_digest: "sha256:structural-manifest".to_string(),
            batch_digest: "sha256:structural-batch".to_string(),
            mode: BatchIngestMode::Delta,
            replace_scopes: vec![StructuralReplaceScope {
                scope: fixture_scope_key(),
                scope_digest: "scope:structural-1".to_string(),
                trees: vec![StructuralTreeRecord {
                    chunk_id: fixture_chunk_id(),
                    record: fixture_parse_tree_record(),
                }],
            }],
            tombstone_scopes: vec![StructuralTombstoneScope {
                scope: SearchScopeKey {
                    doc_surface: SearchScopeSurface::Chunk,
                    repo_relative_path: RepoRelativePath::new("src/old.rs"),
                },
            }],
            seal: true,
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
    fn lexical_ingest_batch_round_trip() -> TestRes {
        let batch = fixture_lexical_batch();
        let bytes = encode(&batch)?;
        let decoded: LexicalIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "ciborium f16 path uses aarch64 inline asm that Miri cannot execute; native f32 vec serde is exercised in stable tests + fuzz"
    )]
    fn semantic_ingest_batch_round_trip() -> TestRes {
        let batch = fixture_semantic_batch();
        let bytes = encode(&batch)?;
        let decoded: SemanticIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    fn history_ingest_batch_round_trip() -> TestRes {
        let batch = fixture_history_batch();
        let bytes = encode(&batch)?;
        let decoded: HistoryIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    fn dirty_ingest_batch_round_trip() -> TestRes {
        let batch = fixture_dirty_batch();
        let bytes = encode(&batch)?;
        let decoded: DirtyIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    fn structural_ingest_batch_round_trip() -> TestRes {
        let batch = fixture_structural_batch();
        let bytes = encode(&batch)?;
        let decoded: StructuralIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    fn batch_publish_receipt_round_trip() -> TestRes {
        let receipt = BatchPublishReceipt {
            generation: ManifestGeneration::new(9),
            manifest_digest: "sha256:feed".to_string(),
            accepted_replace_scopes: 2,
            accepted_tombstone_scopes: 1,
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
    #[cfg_attr(
        miri,
        ignore = "ciborium f16 path uses aarch64 inline asm that Miri cannot execute; native f32 vec serde is exercised in stable tests + fuzz"
    )]
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
    fn search_plane_ingest_request_envelope_round_trip_history() -> TestRes {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 3,
            payload: SearchPlaneIngestIpcRequest::PublishHistoryBatch(fixture_history_batch()),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_request_envelope_round_trip_dirty() -> TestRes {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 4,
            payload: SearchPlaneIngestIpcRequest::PublishDirtyBatch(fixture_dirty_batch()),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_request_envelope_round_trip_structural() -> TestRes {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 5,
            payload: SearchPlaneIngestIpcRequest::PublishStructuralBatch(fixture_structural_batch()),
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
                generation: ManifestGeneration::new(1),
                manifest_digest: "digest-lex".to_string(),
                accepted_replace_scopes: 1,
                accepted_tombstone_scopes: 0,
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
    fn search_plane_ingest_response_envelope_round_trip_history_receipt() -> TestRes {
        let envelope = SearchPlaneIngestIpcResponseEnvelope {
            request_id: 5,
            payload: SearchPlaneIngestIpcResponse::HistoryReceipt(BatchPublishReceipt {
                generation: ManifestGeneration::new(3),
                manifest_digest: "digest-hist".to_string(),
                accepted_replace_scopes: 4,
                accepted_tombstone_scopes: 0,
                sealed: false,
            }),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_response_envelope_round_trip_dirty_receipt() -> TestRes {
        let envelope = SearchPlaneIngestIpcResponseEnvelope {
            request_id: 6,
            payload: SearchPlaneIngestIpcResponse::DirtyReceipt(BatchPublishReceipt {
                generation: ManifestGeneration::new(4),
                manifest_digest: "digest-dirty".to_string(),
                accepted_replace_scopes: 1,
                accepted_tombstone_scopes: 1,
                sealed: false,
            }),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_response_envelope_round_trip_structural_receipt() -> TestRes {
        let envelope = SearchPlaneIngestIpcResponseEnvelope {
            request_id: 7,
            payload: SearchPlaneIngestIpcResponse::StructuralReceipt(BatchPublishReceipt {
                generation: ManifestGeneration::new(5),
                manifest_digest: "digest-struct".to_string(),
                accepted_replace_scopes: 1,
                accepted_tombstone_scopes: 0,
                sealed: false,
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
