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
use std::collections::BTreeSet;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, VariantAccess, Visitor},
    ser::SerializeStruct,
};

use crate::lex::{
    CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, ParseTreeRecord, SymbolRecord,
};
use crate::{
    ChunkId, ChunkRecord, EmbeddingRecord, ManifestGeneration, OwnerDocKind, RepoId,
    RepoMapMutationAck, RepoMapSourceBundle, RepoRelativePath, RevisionId,
};

use super::{
    batch_body::{BATCH_DIGEST_TOKEN_LEN_V1, is_canonical_batch_digest_token_v1},
    control::SemanticContentRootsV1,
    error::SearchPlaneIpcError,
    semantic_source::{
        ClusterMembershipReplaceV1, SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1,
    },
};
use crate::semantic_kinds::SemanticCorpusKindV1;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum SearchScopeSurface {
    File,
    Module,
    Chunk,
    Symbol,
}

impl SearchScopeSurface {
    /// Canonical semantic-owner projection used by derivation and durable
    /// surface deletion. Keeping this mapping in the shared contract prevents
    /// producer, dispatcher, and semantic-adapter policy drift.
    #[must_use]
    pub const fn for_semantic_owner_v1(
        owner_kind: OwnerDocKind,
        corpus_kind: SemanticCorpusKindV1,
    ) -> Self {
        match owner_kind {
            OwnerDocKind::File => Self::File,
            OwnerDocKind::Module => Self::Module,
            OwnerDocKind::Chunk => Self::Chunk,
            OwnerDocKind::Symbol => Self::Symbol,
            OwnerDocKind::Callsite
            | OwnerDocKind::GraphEdge
            | OwnerDocKind::Dataflow
            | OwnerDocKind::Risk
            | OwnerDocKind::Test
            | OwnerDocKind::RepoMap
            | OwnerDocKind::ServiceMap
            | OwnerDocKind::OwnerMap => match corpus_kind {
                SemanticCorpusKindV1::SymbolCard | SemanticCorpusKindV1::RawCodeFallback => {
                    Self::Symbol
                }
                SemanticCorpusKindV1::ModuleCard | SemanticCorpusKindV1::ClusterCard => {
                    Self::Module
                }
                SemanticCorpusKindV1::DocumentLeaf
                | SemanticCorpusKindV1::DocumentSection
                | SemanticCorpusKindV1::DocumentSummary
                | SemanticCorpusKindV1::TestBehavior
                | SemanticCorpusKindV1::RepositorySummary => Self::File,
            },
        }
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchCorpusSurfaceMutationConflictV1 {
    DuplicateClear(SearchScopeSurface),
    NonCanonicalClearOrder,
    DuplicateReplaceScope(SearchScopeSurface),
    DuplicateTombstoneScope(SearchScopeSurface),
    ReplaceAndTombstone(SearchScopeSurface),
    ClearAndReplace(SearchScopeSurface),
    ClearAndTombstone(SearchScopeSurface),
}

impl fmt::Display for SearchCorpusSurfaceMutationConflictV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateClear(surface) => {
                write!(formatter, "duplicate clear for search surface {surface:?}")
            }
            Self::NonCanonicalClearOrder => {
                formatter.write_str("search surface clears must use canonical ascending order")
            }
            Self::DuplicateReplaceScope(surface) => write!(
                formatter,
                "duplicate replace scope on search surface {surface:?}"
            ),
            Self::DuplicateTombstoneScope(surface) => write!(
                formatter,
                "duplicate tombstone scope on search surface {surface:?}"
            ),
            Self::ReplaceAndTombstone(surface) => write!(
                formatter,
                "search scope on surface {surface:?} cannot be replaced and tombstoned in one batch"
            ),
            Self::ClearAndReplace(surface) => write!(
                formatter,
                "search surface {surface:?} cannot be cleared and replaced in one batch"
            ),
            Self::ClearAndTombstone(surface) => write!(
                formatter,
                "search surface {surface:?} cannot be cleared and tombstoned in one batch"
            ),
        }
    }
}

impl std::error::Error for SearchCorpusSurfaceMutationConflictV1 {}

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
pub struct SearchCorpusReplaceScope {
    pub scope: SearchScopeKey,
    pub scope_digest: String,
    pub chunks: Vec<ChunkRecord>,
    pub symbols: Vec<SymbolRecord>,
}

const SEARCH_CORPUS_REPLACE_SCOPE_FIELDS: &[&str] = &["scope", "scope_digest", "chunks", "symbols"];

impl Serialize for SearchCorpusReplaceScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchCorpusReplaceScope", 4)?;
        state.serialize_field("scope", &self.scope)?;
        state.serialize_field("scope_digest", &self.scope_digest)?;
        state.serialize_field("chunks", &self.chunks)?;
        state.serialize_field("symbols", &self.symbols)?;
        state.end()
    }
}

struct SearchCorpusReplaceScopeVisitor;

impl<'de> Visitor<'de> for SearchCorpusReplaceScopeVisitor {
    type Value = SearchCorpusReplaceScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchCorpusReplaceScope map")
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
                        SEARCH_CORPUS_REPLACE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchCorpusReplaceScope {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
            scope_digest: scope_digest.ok_or_else(|| de::Error::missing_field("scope_digest"))?,
            chunks: chunks.ok_or_else(|| de::Error::missing_field("chunks"))?,
            symbols: symbols.ok_or_else(|| de::Error::missing_field("symbols"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchCorpusReplaceScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchCorpusReplaceScope",
            SEARCH_CORPUS_REPLACE_SCOPE_FIELDS,
            SearchCorpusReplaceScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusTombstoneScope {
    pub scope: SearchScopeKey,
}

const SEARCH_CORPUS_TOMBSTONE_SCOPE_FIELDS: &[&str] = &["scope"];

impl Serialize for SearchCorpusTombstoneScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchCorpusTombstoneScope", 1)?;
        state.serialize_field("scope", &self.scope)?;
        state.end()
    }
}

struct SearchCorpusTombstoneScopeVisitor;

impl<'de> Visitor<'de> for SearchCorpusTombstoneScopeVisitor {
    type Value = SearchCorpusTombstoneScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchCorpusTombstoneScope map")
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
                        SEARCH_CORPUS_TOMBSTONE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchCorpusTombstoneScope {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchCorpusTombstoneScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchCorpusTombstoneScope",
            SEARCH_CORPUS_TOMBSTONE_SCOPE_FIELDS,
            SearchCorpusTombstoneScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub base_generation: Option<ManifestGeneration>,
    pub manifest_digest: String,
    pub batch_digest: String,
    pub mode: BatchIngestMode,
    pub bundle_payload: Option<Vec<u8>>,
    /// Canonical whole-surface deletion intents applied to the target
    /// generation before scope-level mutations. The vector must be sorted,
    /// duplicate-free, and disjoint from every scope mutation in this batch.
    pub clear_surfaces: Vec<SearchScopeSurface>,
    pub replace_scopes: Vec<SearchCorpusReplaceScope>,
    pub tombstone_scopes: Vec<SearchCorpusTombstoneScope>,
    pub semantic_replace_scopes: Vec<SemanticSourceReplaceScopeV1>,
    pub semantic_tombstone_scopes: Vec<SemanticSourceScopeKeyV1>,
    pub seal: bool,
}

const SEARCH_CORPUS_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "base_generation",
    "manifest_digest",
    "batch_digest",
    "mode",
    "bundle_payload",
    "clear_surfaces",
    "replace_scopes",
    "tombstone_scopes",
    "semantic_replace_scopes",
    "semantic_tombstone_scopes",
    "seal",
];

impl Serialize for SearchCorpusIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchCorpusIngestBatch", 14)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("base_generation", &self.base_generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("bundle_payload", &self.bundle_payload)?;
        state.serialize_field("clear_surfaces", &self.clear_surfaces)?;
        state.serialize_field("replace_scopes", &self.replace_scopes)?;
        state.serialize_field("tombstone_scopes", &self.tombstone_scopes)?;
        state.serialize_field("semantic_replace_scopes", &self.semantic_replace_scopes)?;
        state.serialize_field("semantic_tombstone_scopes", &self.semantic_tombstone_scopes)?;
        state.serialize_field("seal", &self.seal)?;
        state.end()
    }
}

struct SearchCorpusIngestBatchVisitor;

impl<'de> Visitor<'de> for SearchCorpusIngestBatchVisitor {
    type Value = SearchCorpusIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchCorpusIngestBatch map")
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
        let mut clear_surfaces: Option<Vec<SearchScopeSurface>> = None;
        let mut replace_scopes: Option<Vec<SearchCorpusReplaceScope>> = None;
        let mut tombstone_scopes: Option<Vec<SearchCorpusTombstoneScope>> = None;
        let mut semantic_replace_scopes: Option<Vec<SemanticSourceReplaceScopeV1>> = None;
        let mut semantic_tombstone_scopes: Option<Vec<SemanticSourceScopeKeyV1>> = None;
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
                "clear_surfaces" => {
                    if clear_surfaces.is_some() {
                        return Err(de::Error::duplicate_field("clear_surfaces"));
                    }
                    clear_surfaces = Some(map.next_value()?);
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
                "semantic_replace_scopes" => {
                    if semantic_replace_scopes.is_some() {
                        return Err(de::Error::duplicate_field("semantic_replace_scopes"));
                    }
                    semantic_replace_scopes = Some(map.next_value()?);
                }
                "semantic_tombstone_scopes" => {
                    if semantic_tombstone_scopes.is_some() {
                        return Err(de::Error::duplicate_field("semantic_tombstone_scopes"));
                    }
                    semantic_tombstone_scopes = Some(map.next_value()?);
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
                        SEARCH_CORPUS_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchCorpusIngestBatch {
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
            // Bounded legacy migration: pre-clear persisted batches did not
            // carry this field and therefore decode as the empty clear set.
            // New serializers always emit it explicitly.
            clear_surfaces: clear_surfaces.unwrap_or_default(),
            replace_scopes: replace_scopes
                .ok_or_else(|| de::Error::missing_field("replace_scopes"))?,
            tombstone_scopes: tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("tombstone_scopes"))?,
            semantic_replace_scopes: semantic_replace_scopes.unwrap_or_default(),
            semantic_tombstone_scopes: semantic_tombstone_scopes.unwrap_or_default(),
            seal: seal.ok_or_else(|| de::Error::missing_field("seal"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchCorpusIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchCorpusIngestBatch",
            SEARCH_CORPUS_INGEST_BATCH_FIELDS,
            SearchCorpusIngestBatchVisitor,
        )
    }
}

/// A batch whose shape the contract refuses before any adapter observes it
/// (QI-BB-029).
///
/// These are the defects that used to be discovered one track at a time,
/// after the other track had already mutated: a mode/base pairing that one
/// backend accepts and the other refuses, a digest the activation identity
/// can never carry, a base that cannot precede its target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchCorpusBatchShapeErrorV1 {
    /// `ReplaceGeneration` names a base, or `Delta` names none.
    ModeBaseMismatch {
        mode: BatchIngestMode,
        base_generation: Option<ManifestGeneration>,
    },
    /// A delta base must be strictly older than its target.
    BaseNotOlderThanTarget {
        base_generation: ManifestGeneration,
        generation: ManifestGeneration,
    },
    /// `manifest_digest` is empty or not a bare printable ASCII token; the
    /// activation identity and the retention record both key on it and
    /// neither can carry such a value.
    DigestNotCanonical { field: &'static str },
    /// `batch_digest` does not have the shape of a canonical batch digest
    /// (64 lowercase hex characters, see
    /// [`is_canonical_batch_digest_token_v1`]); the idempotency record keys
    /// on it and the search plane recomputes it from the body.
    BatchDigestNotCanonical,
}

impl fmt::Display for SearchCorpusBatchShapeErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModeBaseMismatch {
                mode,
                base_generation,
            } => write!(
                formatter,
                "batch mode {mode:?} does not admit base_generation={base_generation:?}: ReplaceGeneration takes no base, Delta requires one"
            ),
            Self::BaseNotOlderThanTarget {
                base_generation,
                generation,
            } => write!(
                formatter,
                "delta base generation {} must be older than target generation {}",
                base_generation.get(),
                generation.get()
            ),
            Self::DigestNotCanonical { field } => write!(
                formatter,
                "{field} must be a non-empty printable ASCII token without whitespace"
            ),
            Self::BatchDigestNotCanonical => write!(
                formatter,
                "batch_digest must be the canonical batch digest: {BATCH_DIGEST_TOKEN_LEN_V1} lowercase hex characters of SHA-256 over the canonical body"
            ),
        }
    }
}

impl std::error::Error for SearchCorpusBatchShapeErrorV1 {}

fn is_canonical_digest_token(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_graphic())
}

impl SearchCorpusIngestBatch {
    /// Validate everything about the batch that needs no storage access:
    /// mode/base shape, base ordering and the digests' shapes. The
    /// materializer runs this before taking any lock and the ingest
    /// dispatcher before it records durable intent, so a malformed batch
    /// changes zero bytes on either track and leaves no idempotency record.
    /// Whether `batch_digest` is *the* digest of this body is the
    /// dispatcher's recomputation to prove; this only checks its shape.
    pub fn validate_v1(&self) -> Result<(), SearchCorpusBatchShapeErrorV1> {
        match (self.mode, self.base_generation) {
            (BatchIngestMode::ReplaceGeneration, None) => {}
            (BatchIngestMode::Delta, Some(base_generation)) => {
                if base_generation >= self.generation {
                    return Err(SearchCorpusBatchShapeErrorV1::BaseNotOlderThanTarget {
                        base_generation,
                        generation: self.generation,
                    });
                }
            }
            (mode, base_generation) => {
                return Err(SearchCorpusBatchShapeErrorV1::ModeBaseMismatch {
                    mode,
                    base_generation,
                });
            }
        }
        if !is_canonical_digest_token(&self.manifest_digest) {
            return Err(SearchCorpusBatchShapeErrorV1::DigestNotCanonical {
                field: "manifest_digest",
            });
        }
        if !is_canonical_batch_digest_token_v1(&self.batch_digest) {
            return Err(SearchCorpusBatchShapeErrorV1::BatchDigestNotCanonical);
        }
        Ok(())
    }

    /// Validate the mutation authority before any adapter observes the batch.
    /// A whole-surface clear and a scope mutation on that surface cannot be
    /// ordered safely without creating producer-dependent semantics.
    pub fn validate_surface_mutations_v1(
        &self,
    ) -> Result<(), SearchCorpusSurfaceMutationConflictV1> {
        let clear_surfaces = validated_search_corpus_clear_surfaces_v1(self)?;
        validate_search_corpus_lexical_scope_mutations_v1(self)?;
        validate_search_corpus_semantic_scope_mutations_v1(self)?;
        validate_search_corpus_clear_disjoint_v1(self, &clear_surfaces)
    }
}

fn validated_search_corpus_clear_surfaces_v1(
    batch: &SearchCorpusIngestBatch,
) -> Result<BTreeSet<SearchScopeSurface>, SearchCorpusSurfaceMutationConflictV1> {
    let mut clear_surfaces = BTreeSet::new();
    for surface in &batch.clear_surfaces {
        if !clear_surfaces.insert(*surface) {
            return Err(SearchCorpusSurfaceMutationConflictV1::DuplicateClear(
                *surface,
            ));
        }
    }
    if !batch
        .clear_surfaces
        .windows(2)
        .all(|pair| matches!(pair, [left, right] if left < right))
    {
        return Err(SearchCorpusSurfaceMutationConflictV1::NonCanonicalClearOrder);
    }
    Ok(clear_surfaces)
}

fn validate_search_corpus_lexical_scope_mutations_v1(
    batch: &SearchCorpusIngestBatch,
) -> Result<(), SearchCorpusSurfaceMutationConflictV1> {
    let mut replace_scope_keys = BTreeSet::new();
    for scope in &batch.replace_scopes {
        let key = (
            scope.scope.doc_surface,
            scope.scope.repo_relative_path.as_str(),
        );
        if !replace_scope_keys.insert(key) {
            return Err(
                SearchCorpusSurfaceMutationConflictV1::DuplicateReplaceScope(
                    scope.scope.doc_surface,
                ),
            );
        }
    }
    let mut tombstone_scope_keys = BTreeSet::new();
    for scope in &batch.tombstone_scopes {
        let key = (
            scope.scope.doc_surface,
            scope.scope.repo_relative_path.as_str(),
        );
        if !tombstone_scope_keys.insert(key) {
            return Err(
                SearchCorpusSurfaceMutationConflictV1::DuplicateTombstoneScope(
                    scope.scope.doc_surface,
                ),
            );
        }
        if replace_scope_keys.contains(&key) {
            return Err(SearchCorpusSurfaceMutationConflictV1::ReplaceAndTombstone(
                scope.scope.doc_surface,
            ));
        }
    }
    Ok(())
}

fn validate_search_corpus_semantic_scope_mutations_v1(
    batch: &SearchCorpusIngestBatch,
) -> Result<(), SearchCorpusSurfaceMutationConflictV1> {
    let mut replace_scope_keys = BTreeSet::new();
    for scope in &batch.semantic_replace_scopes {
        let key = (
            scope.scope.corpus_kind.as_code_str(),
            scope.scope.owner_kind.as_code_str(),
            scope.scope.owner_id.as_str(),
        );
        let surface = SearchScopeSurface::for_semantic_owner_v1(
            scope.scope.owner_kind,
            scope.scope.corpus_kind,
        );
        if !replace_scope_keys.insert(key) {
            return Err(SearchCorpusSurfaceMutationConflictV1::DuplicateReplaceScope(surface));
        }
    }
    let mut tombstone_scope_keys = BTreeSet::new();
    for scope in &batch.semantic_tombstone_scopes {
        let key = (
            scope.corpus_kind.as_code_str(),
            scope.owner_kind.as_code_str(),
            scope.owner_id.as_str(),
        );
        let surface =
            SearchScopeSurface::for_semantic_owner_v1(scope.owner_kind, scope.corpus_kind);
        if !tombstone_scope_keys.insert(key) {
            return Err(SearchCorpusSurfaceMutationConflictV1::DuplicateTombstoneScope(surface));
        }
        if replace_scope_keys.contains(&key) {
            return Err(SearchCorpusSurfaceMutationConflictV1::ReplaceAndTombstone(
                surface,
            ));
        }
    }
    Ok(())
}

fn validate_search_corpus_clear_disjoint_v1(
    batch: &SearchCorpusIngestBatch,
    clear_surfaces: &BTreeSet<SearchScopeSurface>,
) -> Result<(), SearchCorpusSurfaceMutationConflictV1> {
    for surface in batch
        .replace_scopes
        .iter()
        .map(|scope| scope.scope.doc_surface)
        .chain(batch.semantic_replace_scopes.iter().map(|scope| {
            SearchScopeSurface::for_semantic_owner_v1(
                scope.scope.owner_kind,
                scope.scope.corpus_kind,
            )
        }))
    {
        if clear_surfaces.contains(&surface) {
            return Err(SearchCorpusSurfaceMutationConflictV1::ClearAndReplace(
                surface,
            ));
        }
    }
    for surface in batch
        .tombstone_scopes
        .iter()
        .map(|scope| scope.scope.doc_surface)
        .chain(batch.semantic_tombstone_scopes.iter().map(|scope| {
            SearchScopeSurface::for_semantic_owner_v1(scope.owner_kind, scope.corpus_kind)
        }))
    {
        if clear_surfaces.contains(&surface) {
            return Err(SearchCorpusSurfaceMutationConflictV1::ClearAndTombstone(
                surface,
            ));
        }
    }
    Ok(())
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
    pub cluster_memberships: Vec<ClusterMembershipReplaceV1>,
}

const SEMANTIC_REPLACE_SCOPE_FIELDS: &[&str] =
    &["scope", "scope_digest", "embeddings", "cluster_memberships"];

impl Serialize for SemanticReplaceScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticReplaceScope", 4)?;
        state.serialize_field("scope", &self.scope)?;
        state.serialize_field("scope_digest", &self.scope_digest)?;
        state.serialize_field("embeddings", &self.embeddings)?;
        state.serialize_field("cluster_memberships", &self.cluster_memberships)?;
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
        let mut cluster_memberships: Option<Vec<ClusterMembershipReplaceV1>> = None;
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
                "cluster_memberships" => {
                    if cluster_memberships.is_some() {
                        return Err(de::Error::duplicate_field("cluster_memberships"));
                    }
                    cluster_memberships = Some(map.next_value()?);
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
            cluster_memberships: cluster_memberships.unwrap_or_default(),
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
    /// Legacy path-scoped deletion authority. New semantic-source producers
    /// leave this unset and address deletion through `semantic_scope`.
    pub scope: Option<SearchScopeKey>,
    pub semantic_scope: Option<SemanticSourceScopeKeyV1>,
}

const SEMANTIC_TOMBSTONE_SCOPE_FIELDS: &[&str] = &["scope", "semantic_scope"];

impl Serialize for SemanticTombstoneScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = if self.scope.is_some() { 2 } else { 1 };
        let mut state = serializer.serialize_struct("SemanticTombstoneScope", field_count)?;
        if let Some(scope) = &self.scope {
            state.serialize_field("scope", scope)?;
        }
        state.serialize_field("semantic_scope", &self.semantic_scope)?;
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
        let mut scope_seen = false;
        let mut semantic_scope: Option<Option<SemanticSourceScopeKeyV1>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "scope" => {
                    if scope_seen {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope_seen = true;
                    scope = Some(map.next_value()?);
                }
                "semantic_scope" => {
                    if semantic_scope.is_some() {
                        return Err(de::Error::duplicate_field("semantic_scope"));
                    }
                    semantic_scope = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_TOMBSTONE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        let semantic_scope = semantic_scope.unwrap_or(None);
        if scope.is_none() && semantic_scope.is_none() {
            return Err(de::Error::custom(
                "semantic tombstone requires scope or semantic_scope",
            ));
        }
        Ok(SemanticTombstoneScope {
            scope,
            semantic_scope,
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
    pub required_corpora: Vec<SemanticCorpusKindV1>,
    pub corpus_policy_digest: Option<String>,
    pub clear_surfaces: Vec<SearchScopeSurface>,
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
    "required_corpora",
    "corpus_policy_digest",
    "clear_surfaces",
    "replace_scopes",
    "tombstone_scopes",
    "seal",
];

impl Serialize for SemanticIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticIngestBatch", 14)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("base_generation", &self.base_generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("model_contract", &self.model_contract)?;
        state.serialize_field("required_corpora", &self.required_corpora)?;
        state.serialize_field("corpus_policy_digest", &self.corpus_policy_digest)?;
        state.serialize_field("clear_surfaces", &self.clear_surfaces)?;
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
        let mut required_corpora: Option<Vec<SemanticCorpusKindV1>> = None;
        let mut corpus_policy_digest: Option<Option<String>> = None;
        let mut clear_surfaces: Option<Vec<SearchScopeSurface>> = None;
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
                "required_corpora" => {
                    if required_corpora.is_some() {
                        return Err(de::Error::duplicate_field("required_corpora"));
                    }
                    required_corpora = Some(map.next_value()?);
                }
                "corpus_policy_digest" => {
                    if corpus_policy_digest.is_some() {
                        return Err(de::Error::duplicate_field("corpus_policy_digest"));
                    }
                    corpus_policy_digest = Some(map.next_value()?);
                }
                "clear_surfaces" => {
                    if clear_surfaces.is_some() {
                        return Err(de::Error::duplicate_field("clear_surfaces"));
                    }
                    clear_surfaces = Some(map.next_value()?);
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
            required_corpora: required_corpora.unwrap_or_default(),
            corpus_policy_digest: corpus_policy_digest.unwrap_or(None),
            // Bounded legacy migration for semantic batches persisted before
            // whole-surface clear was part of the wire contract.
            clear_surfaces: clear_surfaces.unwrap_or_default(),
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoCommitRecencyEntry {
    pub source_repo_id: RepoId,
    pub latest_committer_time_ms: u64,
}

const REPO_COMMIT_RECENCY_ENTRY_FIELDS: &[&str] = &["source_repo_id", "latest_committer_time_ms"];

impl Serialize for RepoCommitRecencyEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoCommitRecencyEntry", 2)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("latest_committer_time_ms", &self.latest_committer_time_ms)?;
        state.end()
    }
}

struct RepoCommitRecencyEntryVisitor;

impl<'de> Visitor<'de> for RepoCommitRecencyEntryVisitor {
    type Value = RepoCommitRecencyEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoCommitRecencyEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_repo_id: Option<RepoId> = None;
        let mut latest_committer_time_ms: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "source_repo_id" => source_repo_id = Some(map.next_value()?),
                "latest_committer_time_ms" => latest_committer_time_ms = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_COMMIT_RECENCY_ENTRY_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoCommitRecencyEntry {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            latest_committer_time_ms: latest_committer_time_ms
                .ok_or_else(|| de::Error::missing_field("latest_committer_time_ms"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoCommitRecencyEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoCommitRecencyEntry",
            REPO_COMMIT_RECENCY_ENTRY_FIELDS,
            RepoCommitRecencyEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoCommitRecencyIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub entries: Vec<RepoCommitRecencyEntry>,
}

const REPO_COMMIT_RECENCY_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "batch_digest",
    "entries",
];

impl Serialize for RepoCommitRecencyIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoCommitRecencyIngestBatch", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct RepoCommitRecencyIngestBatchVisitor;

impl<'de> Visitor<'de> for RepoCommitRecencyIngestBatchVisitor {
    type Value = RepoCommitRecencyIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoCommitRecencyIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<RepoCommitRecencyEntry>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_COMMIT_RECENCY_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoCommitRecencyIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoCommitRecencyIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoCommitRecencyIngestBatch",
            REPO_COMMIT_RECENCY_INGEST_BATCH_FIELDS,
            RepoCommitRecencyIngestBatchVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMetaEntry {
    pub source_repo_id: RepoId,
    pub key: String,
    pub value: String,
}

const REPO_META_ENTRY_FIELDS: &[&str] = &["source_repo_id", "key", "value"];

impl Serialize for RepoMetaEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMetaEntry", 3)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("key", &self.key)?;
        state.serialize_field("value", &self.value)?;
        state.end()
    }
}

struct RepoMetaEntryVisitor;

impl<'de> Visitor<'de> for RepoMetaEntryVisitor {
    type Value = RepoMetaEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMetaEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_repo_id: Option<RepoId> = None;
        let mut key: Option<String> = None;
        let mut value: Option<String> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "source_repo_id" => source_repo_id = Some(map.next_value()?),
                "key" => key = Some(map.next_value()?),
                "value" => value = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(other, REPO_META_ENTRY_FIELDS));
                }
            }
        }
        Ok(RepoMetaEntry {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            key: key.ok_or_else(|| de::Error::missing_field("key"))?,
            value: value.ok_or_else(|| de::Error::missing_field("value"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMetaEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMetaEntry",
            REPO_META_ENTRY_FIELDS,
            RepoMetaEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMetaIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub entries: Vec<RepoMetaEntry>,
}

const REPO_META_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "batch_digest",
    "entries",
];

impl Serialize for RepoMetaIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMetaIngestBatch", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct RepoMetaIngestBatchVisitor;

impl<'de> Visitor<'de> for RepoMetaIngestBatchVisitor {
    type Value = RepoMetaIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMetaIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<RepoMetaEntry>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_META_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoMetaIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMetaIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMetaIngestBatch",
            REPO_META_INGEST_BATCH_FIELDS,
            RepoMetaIngestBatchVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoTopicEntry {
    pub source_repo_id: RepoId,
    pub topic: String,
}

const REPO_TOPIC_ENTRY_FIELDS: &[&str] = &["source_repo_id", "topic"];

impl Serialize for RepoTopicEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoTopicEntry", 2)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("topic", &self.topic)?;
        state.end()
    }
}

struct RepoTopicEntryVisitor;

impl<'de> Visitor<'de> for RepoTopicEntryVisitor {
    type Value = RepoTopicEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoTopicEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_repo_id: Option<RepoId> = None;
        let mut topic: Option<String> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "source_repo_id" => source_repo_id = Some(map.next_value()?),
                "topic" => topic = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(other, REPO_TOPIC_ENTRY_FIELDS));
                }
            }
        }
        Ok(RepoTopicEntry {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            topic: topic.ok_or_else(|| de::Error::missing_field("topic"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoTopicEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoTopicEntry",
            REPO_TOPIC_ENTRY_FIELDS,
            RepoTopicEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoTopicIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub entries: Vec<RepoTopicEntry>,
}

const REPO_TOPIC_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "batch_digest",
    "entries",
];

impl Serialize for RepoTopicIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoTopicIngestBatch", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct RepoTopicIngestBatchVisitor;

impl<'de> Visitor<'de> for RepoTopicIngestBatchVisitor {
    type Value = RepoTopicIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoTopicIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<RepoTopicEntry>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_TOPIC_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoTopicIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoTopicIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoTopicIngestBatch",
            REPO_TOPIC_INGEST_BATCH_FIELDS,
            RepoTopicIngestBatchVisitor,
        )
    }
}

/// One producer-published repo description, keyed by source repo.
///
/// The producer is the sole authority for the description string (e.g. the
/// code-host repo description); the search plane stores it verbatim and matches
/// it as a regex at `repo:has.description(<pattern>)` query time. Distinct from
/// [`RepoMetaEntry`] (key/value tags) and [`RepoTopicEntry`] (topic set) — the
/// description is a single free-text string per repo.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoDescriptionEntry {
    pub source_repo_id: RepoId,
    pub description: String,
}

const REPO_DESCRIPTION_ENTRY_FIELDS: &[&str] = &["source_repo_id", "description"];

impl Serialize for RepoDescriptionEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoDescriptionEntry", 2)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("description", &self.description)?;
        state.end()
    }
}

struct RepoDescriptionEntryVisitor;

impl<'de> Visitor<'de> for RepoDescriptionEntryVisitor {
    type Value = RepoDescriptionEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoDescriptionEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_repo_id: Option<RepoId> = None;
        let mut description: Option<String> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "source_repo_id" => {
                    if source_repo_id.is_some() {
                        return Err(de::Error::duplicate_field("source_repo_id"));
                    }
                    source_repo_id = Some(map.next_value()?);
                }
                "description" => {
                    if description.is_some() {
                        return Err(de::Error::duplicate_field("description"));
                    }
                    description = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_DESCRIPTION_ENTRY_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoDescriptionEntry {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            description: description.ok_or_else(|| de::Error::missing_field("description"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoDescriptionEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoDescriptionEntry",
            REPO_DESCRIPTION_ENTRY_FIELDS,
            RepoDescriptionEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoDescriptionIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub entries: Vec<RepoDescriptionEntry>,
}

const REPO_DESCRIPTION_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "batch_digest",
    "entries",
];

impl Serialize for RepoDescriptionIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoDescriptionIngestBatch", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct RepoDescriptionIngestBatchVisitor;

impl<'de> Visitor<'de> for RepoDescriptionIngestBatchVisitor {
    type Value = RepoDescriptionIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoDescriptionIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<RepoDescriptionEntry>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
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
                "batch_digest" => {
                    if batch_digest.is_some() {
                        return Err(de::Error::duplicate_field("batch_digest"));
                    }
                    batch_digest = Some(map.next_value()?);
                }
                "entries" => {
                    if entries.is_some() {
                        return Err(de::Error::duplicate_field("entries"));
                    }
                    entries = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_DESCRIPTION_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoDescriptionIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoDescriptionIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoDescriptionIngestBatch",
            REPO_DESCRIPTION_INGEST_BATCH_FIELDS,
            RepoDescriptionIngestBatchVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileOwnershipEntry {
    pub source_repo_id: RepoId,
    pub repo_relative_path: RepoRelativePath,
    pub owners: Vec<String>,
}

const FILE_OWNERSHIP_ENTRY_FIELDS: &[&str] = &["source_repo_id", "repo_relative_path", "owners"];

impl Serialize for FileOwnershipEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("FileOwnershipEntry", 3)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("owners", &self.owners)?;
        state.end()
    }
}

struct FileOwnershipEntryVisitor;

impl<'de> Visitor<'de> for FileOwnershipEntryVisitor {
    type Value = FileOwnershipEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileOwnershipEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_repo_id: Option<RepoId> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut owners: Option<Vec<String>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "source_repo_id" => source_repo_id = Some(map.next_value()?),
                "repo_relative_path" => repo_relative_path = Some(map.next_value()?),
                "owners" => owners = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(other, FILE_OWNERSHIP_ENTRY_FIELDS));
                }
            }
        }
        Ok(FileOwnershipEntry {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            owners: owners.ok_or_else(|| de::Error::missing_field("owners"))?,
        })
    }
}

impl<'de> Deserialize<'de> for FileOwnershipEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileOwnershipEntry",
            FILE_OWNERSHIP_ENTRY_FIELDS,
            FileOwnershipEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileOwnershipIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub entries: Vec<FileOwnershipEntry>,
}

const FILE_OWNERSHIP_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "batch_digest",
    "entries",
];

impl Serialize for FileOwnershipIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("FileOwnershipIngestBatch", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct FileOwnershipIngestBatchVisitor;

impl<'de> Visitor<'de> for FileOwnershipIngestBatchVisitor {
    type Value = FileOwnershipIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileOwnershipIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<FileOwnershipEntry>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        FILE_OWNERSHIP_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(FileOwnershipIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for FileOwnershipIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileOwnershipIngestBatch",
            FILE_OWNERSHIP_INGEST_BATCH_FIELDS,
            FileOwnershipIngestBatchVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FileContributorIdentityEntry {
    pub canonical: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

const FILE_CONTRIBUTOR_IDENTITY_ENTRY_FIELDS: &[&str] = &["canonical", "name", "email"];

impl Serialize for FileContributorIdentityEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("FileContributorIdentityEntry", 3)?;
        state.serialize_field("canonical", &self.canonical)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("email", &self.email)?;
        state.end()
    }
}

struct FileContributorIdentityEntryVisitor;

impl<'de> Visitor<'de> for FileContributorIdentityEntryVisitor {
    type Value = FileContributorIdentityEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileContributorIdentityEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut canonical: Option<String> = None;
        let mut name: Option<Option<String>> = None;
        let mut email: Option<Option<String>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "canonical" => {
                    if canonical.is_some() {
                        return Err(de::Error::duplicate_field("canonical"));
                    }
                    canonical = Some(map.next_value()?);
                }
                "name" => {
                    if name.is_some() {
                        return Err(de::Error::duplicate_field("name"));
                    }
                    name = Some(map.next_value()?);
                }
                "email" => {
                    if email.is_some() {
                        return Err(de::Error::duplicate_field("email"));
                    }
                    email = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        FILE_CONTRIBUTOR_IDENTITY_ENTRY_FIELDS,
                    ));
                }
            }
        }
        Ok(FileContributorIdentityEntry {
            canonical: canonical.ok_or_else(|| de::Error::missing_field("canonical"))?,
            name: name.unwrap_or(None),
            email: email.unwrap_or(None),
        })
    }
}

impl<'de> Deserialize<'de> for FileContributorIdentityEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileContributorIdentityEntry",
            FILE_CONTRIBUTOR_IDENTITY_ENTRY_FIELDS,
            FileContributorIdentityEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileContributorEntry {
    pub source_repo_id: RepoId,
    pub repo_relative_path: RepoRelativePath,
    pub contributors: Vec<FileContributorIdentityEntry>,
}

const FILE_CONTRIBUTOR_ENTRY_FIELDS: &[&str] =
    &["source_repo_id", "repo_relative_path", "contributors"];

impl Serialize for FileContributorEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("FileContributorEntry", 3)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("contributors", &self.contributors)?;
        state.end()
    }
}

struct FileContributorEntryVisitor;

impl<'de> Visitor<'de> for FileContributorEntryVisitor {
    type Value = FileContributorEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileContributorEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_repo_id: Option<RepoId> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut contributors: Option<Vec<FileContributorIdentityEntry>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "source_repo_id" => source_repo_id = Some(map.next_value()?),
                "repo_relative_path" => repo_relative_path = Some(map.next_value()?),
                "contributors" => contributors = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        FILE_CONTRIBUTOR_ENTRY_FIELDS,
                    ));
                }
            }
        }
        Ok(FileContributorEntry {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            contributors: contributors.ok_or_else(|| de::Error::missing_field("contributors"))?,
        })
    }
}

impl<'de> Deserialize<'de> for FileContributorEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileContributorEntry",
            FILE_CONTRIBUTOR_ENTRY_FIELDS,
            FileContributorEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileContributorIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub entries: Vec<FileContributorEntry>,
}

const FILE_CONTRIBUTOR_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "batch_digest",
    "entries",
];

impl Serialize for FileContributorIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("FileContributorIngestBatch", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct FileContributorIngestBatchVisitor;

impl<'de> Visitor<'de> for FileContributorIngestBatchVisitor {
    type Value = FileContributorIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileContributorIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<FileContributorEntry>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        FILE_CONTRIBUTOR_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(FileContributorIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for FileContributorIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileContributorIngestBatch",
            FILE_CONTRIBUTOR_INGEST_BATCH_FIELDS,
            FileContributorIngestBatchVisitor,
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
// Runtime catalog ingest batch
// =============================================================================

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeChangedRecord {
    pub doc_id: ChunkId,
    pub applied_at_ms: u64,
    pub payload_hash: [u8; 32],
}

const RUNTIME_CHANGED_RECORD_FIELDS: &[&str] = &["doc_id", "applied_at_ms", "payload_hash"];

impl Serialize for RuntimeChangedRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RuntimeChangedRecord", 3)?;
        state.serialize_field("doc_id", &self.doc_id)?;
        state.serialize_field("applied_at_ms", &self.applied_at_ms)?;
        state.serialize_field("payload_hash", &self.payload_hash)?;
        state.end()
    }
}

struct RuntimeChangedRecordVisitor;

impl<'de> Visitor<'de> for RuntimeChangedRecordVisitor {
    type Value = RuntimeChangedRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RuntimeChangedRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut doc_id: Option<ChunkId> = None;
        let mut applied_at_ms: Option<u64> = None;
        let mut payload_hash: Option<[u8; 32]> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "doc_id" => doc_id = Some(map.next_value()?),
                "applied_at_ms" => applied_at_ms = Some(map.next_value()?),
                "payload_hash" => payload_hash = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        RUNTIME_CHANGED_RECORD_FIELDS,
                    ));
                }
            }
        }
        Ok(RuntimeChangedRecord {
            doc_id: doc_id.ok_or_else(|| de::Error::missing_field("doc_id"))?,
            applied_at_ms: applied_at_ms
                .ok_or_else(|| de::Error::missing_field("applied_at_ms"))?,
            payload_hash: payload_hash.ok_or_else(|| de::Error::missing_field("payload_hash"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RuntimeChangedRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RuntimeChangedRecord",
            RUNTIME_CHANGED_RECORD_FIELDS,
            RuntimeChangedRecordVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeDocFacetRecord {
    pub doc_id: ChunkId,
    pub owner: Option<String>,
    pub service: Option<String>,
    pub layer: Option<String>,
    pub surface: Option<String>,
}

const RUNTIME_DOC_FACET_RECORD_FIELDS: &[&str] =
    &["doc_id", "owner", "service", "layer", "surface"];

impl Serialize for RuntimeDocFacetRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RuntimeDocFacetRecord", 5)?;
        state.serialize_field("doc_id", &self.doc_id)?;
        state.serialize_field("owner", &self.owner)?;
        state.serialize_field("service", &self.service)?;
        state.serialize_field("layer", &self.layer)?;
        state.serialize_field("surface", &self.surface)?;
        state.end()
    }
}

struct RuntimeDocFacetRecordVisitor;

impl<'de> Visitor<'de> for RuntimeDocFacetRecordVisitor {
    type Value = RuntimeDocFacetRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RuntimeDocFacetRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut doc_id: Option<ChunkId> = None;
        let mut owner: Option<String> = None;
        let mut service: Option<String> = None;
        let mut layer: Option<String> = None;
        let mut surface: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "doc_id" => doc_id = Some(map.next_value()?),
                "owner" => owner = Some(map.next_value()?),
                "service" => service = Some(map.next_value()?),
                "layer" => layer = Some(map.next_value()?),
                "surface" => surface = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        RUNTIME_DOC_FACET_RECORD_FIELDS,
                    ));
                }
            }
        }
        Ok(RuntimeDocFacetRecord {
            doc_id: doc_id.ok_or_else(|| de::Error::missing_field("doc_id"))?,
            owner,
            service,
            layer,
            surface,
        })
    }
}

impl<'de> Deserialize<'de> for RuntimeDocFacetRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RuntimeDocFacetRecord",
            RUNTIME_DOC_FACET_RECORD_FIELDS,
            RuntimeDocFacetRecordVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeSnapshotRecord {
    pub name: String,
    pub doc_ids: Vec<ChunkId>,
}

const RUNTIME_SNAPSHOT_RECORD_FIELDS: &[&str] = &["name", "doc_ids"];

impl Serialize for RuntimeSnapshotRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RuntimeSnapshotRecord", 2)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("doc_ids", &self.doc_ids)?;
        state.end()
    }
}

struct RuntimeSnapshotRecordVisitor;

impl<'de> Visitor<'de> for RuntimeSnapshotRecordVisitor {
    type Value = RuntimeSnapshotRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RuntimeSnapshotRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut name: Option<String> = None;
        let mut doc_ids: Option<Vec<ChunkId>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "name" => name = Some(map.next_value()?),
                "doc_ids" => doc_ids = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        RUNTIME_SNAPSHOT_RECORD_FIELDS,
                    ));
                }
            }
        }
        Ok(RuntimeSnapshotRecord {
            name: name.ok_or_else(|| de::Error::missing_field("name"))?,
            doc_ids: doc_ids.ok_or_else(|| de::Error::missing_field("doc_ids"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RuntimeSnapshotRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RuntimeSnapshotRecord",
            RUNTIME_SNAPSHOT_RECORD_FIELDS,
            RuntimeSnapshotRecordVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeEdgeAuthorityRecord {
    pub key: String,
    pub doc_ids: Vec<ChunkId>,
}

const RUNTIME_EDGE_AUTHORITY_RECORD_FIELDS: &[&str] = &["key", "doc_ids"];

impl Serialize for RuntimeEdgeAuthorityRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RuntimeEdgeAuthorityRecord", 2)?;
        state.serialize_field("key", &self.key)?;
        state.serialize_field("doc_ids", &self.doc_ids)?;
        state.end()
    }
}

struct RuntimeEdgeAuthorityRecordVisitor;

impl<'de> Visitor<'de> for RuntimeEdgeAuthorityRecordVisitor {
    type Value = RuntimeEdgeAuthorityRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RuntimeEdgeAuthorityRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut key: Option<String> = None;
        let mut doc_ids: Option<Vec<ChunkId>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "key" => key = Some(map.next_value()?),
                "doc_ids" => doc_ids = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        RUNTIME_EDGE_AUTHORITY_RECORD_FIELDS,
                    ));
                }
            }
        }
        Ok(RuntimeEdgeAuthorityRecord {
            key: key.ok_or_else(|| de::Error::missing_field("key"))?,
            doc_ids: doc_ids.ok_or_else(|| de::Error::missing_field("doc_ids"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RuntimeEdgeAuthorityRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RuntimeEdgeAuthorityRecord",
            RUNTIME_EDGE_AUTHORITY_RECORD_FIELDS,
            RuntimeEdgeAuthorityRecordVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeCatalogIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub overlay_epoch_ms: u64,
    pub batch_digest: String,
    pub producer_head_applied_at_ms: u64,
    pub generation_materialized_at_ms: u64,
    pub changed_entries: Vec<RuntimeChangedRecord>,
    pub facet_entries: Vec<RuntimeDocFacetRecord>,
    pub snapshot_entries: Vec<RuntimeSnapshotRecord>,
    pub affected_entries: Vec<RuntimeEdgeAuthorityRecord>,
    pub invalidated_by_entries: Vec<RuntimeEdgeAuthorityRecord>,
}

const RUNTIME_CATALOG_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "overlay_epoch_ms",
    "batch_digest",
    "producer_head_applied_at_ms",
    "generation_materialized_at_ms",
    "changed_entries",
    "facet_entries",
    "snapshot_entries",
    "affected_entries",
    "invalidated_by_entries",
];

impl Serialize for RuntimeCatalogIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RuntimeCatalogIngestBatch", 12)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("overlay_epoch_ms", &self.overlay_epoch_ms)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field(
            "producer_head_applied_at_ms",
            &self.producer_head_applied_at_ms,
        )?;
        state.serialize_field(
            "generation_materialized_at_ms",
            &self.generation_materialized_at_ms,
        )?;
        state.serialize_field("changed_entries", &self.changed_entries)?;
        state.serialize_field("facet_entries", &self.facet_entries)?;
        state.serialize_field("snapshot_entries", &self.snapshot_entries)?;
        state.serialize_field("affected_entries", &self.affected_entries)?;
        state.serialize_field("invalidated_by_entries", &self.invalidated_by_entries)?;
        state.end()
    }
}

struct RuntimeCatalogIngestBatchVisitor;

impl<'de> Visitor<'de> for RuntimeCatalogIngestBatchVisitor {
    type Value = RuntimeCatalogIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RuntimeCatalogIngestBatch map")
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
        let mut producer_head_applied_at_ms: Option<u64> = None;
        let mut generation_materialized_at_ms: Option<u64> = None;
        let mut changed_entries: Option<Vec<RuntimeChangedRecord>> = None;
        let mut facet_entries: Option<Vec<RuntimeDocFacetRecord>> = None;
        let mut snapshot_entries: Option<Vec<RuntimeSnapshotRecord>> = None;
        let mut affected_entries: Option<Vec<RuntimeEdgeAuthorityRecord>> = None;
        let mut invalidated_by_entries: Option<Vec<RuntimeEdgeAuthorityRecord>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "overlay_epoch_ms" => overlay_epoch_ms = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "producer_head_applied_at_ms" => {
                    producer_head_applied_at_ms = Some(map.next_value()?);
                }
                "generation_materialized_at_ms" => {
                    generation_materialized_at_ms = Some(map.next_value()?);
                }
                "changed_entries" => changed_entries = Some(map.next_value()?),
                "facet_entries" => facet_entries = Some(map.next_value()?),
                "snapshot_entries" => snapshot_entries = Some(map.next_value()?),
                "affected_entries" => affected_entries = Some(map.next_value()?),
                "invalidated_by_entries" => invalidated_by_entries = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        RUNTIME_CATALOG_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(RuntimeCatalogIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            overlay_epoch_ms: overlay_epoch_ms
                .ok_or_else(|| de::Error::missing_field("overlay_epoch_ms"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            producer_head_applied_at_ms: producer_head_applied_at_ms
                .ok_or_else(|| de::Error::missing_field("producer_head_applied_at_ms"))?,
            generation_materialized_at_ms: generation_materialized_at_ms
                .ok_or_else(|| de::Error::missing_field("generation_materialized_at_ms"))?,
            changed_entries: changed_entries
                .ok_or_else(|| de::Error::missing_field("changed_entries"))?,
            facet_entries: facet_entries
                .ok_or_else(|| de::Error::missing_field("facet_entries"))?,
            snapshot_entries: snapshot_entries
                .ok_or_else(|| de::Error::missing_field("snapshot_entries"))?,
            affected_entries: affected_entries
                .ok_or_else(|| de::Error::missing_field("affected_entries"))?,
            invalidated_by_entries: invalidated_by_entries
                .ok_or_else(|| de::Error::missing_field("invalidated_by_entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RuntimeCatalogIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RuntimeCatalogIngestBatch",
            RUNTIME_CATALOG_INGEST_BATCH_FIELDS,
            RuntimeCatalogIngestBatchVisitor,
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

/// The canonical-CBOR format version of a persisted
/// [`BatchPublishReceipt`] in the operation journal (SEP-21 P02B).
///
/// A journal-persisted receipt is stored as this version tag followed by
/// the receipt's canonical CBOR. A reader that finds any other version
/// refuses before any mutation (typed refusal — old receipt / new runtime
/// and new receipt / old runtime are incompatible by design); there is no
/// boot-time dual decoder and no live migration. Receipts of another
/// version are offline-migration input only.
pub const BATCH_PUBLISH_RECEIPT_FORMAT_VERSION: u32 = 1;

/// Server-side receipt for a successful batch publish.
///
/// Receipt truth is generation/materialization scoped, not channel-sequence
/// scoped. The ingest path may internally fan out to multiple storage writes,
/// but the producer-facing ack reports the generation, how many scope
/// mutations were accepted, and — since QI-BB-032 — which idempotency key it
/// answers, whether this call applied the batch or is replaying a durable
/// earlier apply, and the catalog's durable sequence of that apply.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchPublishReceipt {
    pub generation: ManifestGeneration,
    /// The generation's manifest digest for routes that carry one; `None`
    /// for auxiliary routes that name no manifest. Never the batch digest.
    pub manifest_digest: Option<String>,
    /// The batch digest the publish named: the idempotency key this receipt
    /// answers.
    pub batch_digest: String,
    pub accepted_replace_scopes: u32,
    pub accepted_tombstone_scopes: u32,
    /// Search-corpus semantic mutations accepted by the same durable apply.
    /// Auxiliary routes always report zero.
    pub accepted_semantic_replace_scopes: u32,
    pub accepted_semantic_tombstone_scopes: u32,
    pub accepted_clear_surfaces: u32,
    pub sealed: bool,
    /// `true` when this call applied the batch; `false` when the same body
    /// had already been applied and this is the durable receipt of that
    /// apply (a replay ack that mutated nothing).
    pub applied: bool,
    /// The catalog's durable sequence of the apply, unique and monotonic
    /// across the state root. A replay carries the original apply's
    /// sequence, so a producer can prove two receipts describe one apply.
    pub durable_sequence: u64,
    /// The content roots the semantic generation sealed (QI-BB-028):
    /// present exactly when this is a sealed search-corpus receipt, so the
    /// producer can name them when it activates; `None` for an unsealed
    /// publish and for every auxiliary route.
    pub semantic_content: Option<SemanticContentRootsV1>,
}

const BATCH_PUBLISH_RECEIPT_FIELDS: &[&str] = &[
    "generation",
    "manifest_digest",
    "batch_digest",
    "accepted_replace_scopes",
    "accepted_tombstone_scopes",
    "accepted_semantic_replace_scopes",
    "accepted_semantic_tombstone_scopes",
    "accepted_clear_surfaces",
    "sealed",
    "applied",
    "durable_sequence",
    "semantic_content",
];

impl Serialize for BatchPublishReceipt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("BatchPublishReceipt", 12)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("accepted_replace_scopes", &self.accepted_replace_scopes)?;
        state.serialize_field("accepted_tombstone_scopes", &self.accepted_tombstone_scopes)?;
        state.serialize_field(
            "accepted_semantic_replace_scopes",
            &self.accepted_semantic_replace_scopes,
        )?;
        state.serialize_field(
            "accepted_semantic_tombstone_scopes",
            &self.accepted_semantic_tombstone_scopes,
        )?;
        state.serialize_field("accepted_clear_surfaces", &self.accepted_clear_surfaces)?;
        state.serialize_field("sealed", &self.sealed)?;
        state.serialize_field("applied", &self.applied)?;
        state.serialize_field("durable_sequence", &self.durable_sequence)?;
        state.serialize_field("semantic_content", &self.semantic_content)?;
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
        let mut semantic_content: Option<Option<SemanticContentRootsV1>> = None;
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
                "batch_digest" => {
                    if batch_digest.is_some() {
                        return Err(de::Error::duplicate_field("batch_digest"));
                    }
                    batch_digest = Some(map.next_value()?);
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
                "accepted_semantic_replace_scopes" => {
                    if accepted_semantic_replace_scopes.is_some() {
                        return Err(de::Error::duplicate_field(
                            "accepted_semantic_replace_scopes",
                        ));
                    }
                    accepted_semantic_replace_scopes = Some(map.next_value()?);
                }
                "accepted_semantic_tombstone_scopes" => {
                    if accepted_semantic_tombstone_scopes.is_some() {
                        return Err(de::Error::duplicate_field(
                            "accepted_semantic_tombstone_scopes",
                        ));
                    }
                    accepted_semantic_tombstone_scopes = Some(map.next_value()?);
                }
                "accepted_clear_surfaces" => {
                    if accepted_clear_surfaces.is_some() {
                        return Err(de::Error::duplicate_field("accepted_clear_surfaces"));
                    }
                    accepted_clear_surfaces = Some(map.next_value()?);
                }
                "sealed" => {
                    if sealed.is_some() {
                        return Err(de::Error::duplicate_field("sealed"));
                    }
                    sealed = Some(map.next_value()?);
                }
                "applied" => {
                    if applied.is_some() {
                        return Err(de::Error::duplicate_field("applied"));
                    }
                    applied = Some(map.next_value()?);
                }
                "durable_sequence" => {
                    if durable_sequence.is_some() {
                        return Err(de::Error::duplicate_field("durable_sequence"));
                    }
                    durable_sequence = Some(map.next_value()?);
                }
                "semantic_content" => {
                    if semantic_content.is_some() {
                        return Err(de::Error::duplicate_field("semantic_content"));
                    }
                    semantic_content = Some(map.next_value()?);
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
    /// An empty receipt for `generation` under `batch_digest`, before any
    /// scope is counted. It reads as applied with no durable sequence yet;
    /// the ingest dispatcher stamps the sequence once the catalog has
    /// recorded the apply, and rewrites `applied` on a replay.
    #[must_use]
    pub fn empty_for(
        generation: ManifestGeneration,
        manifest_digest: Option<String>,
        batch_digest: impl Into<String>,
    ) -> Self {
        Self {
            generation,
            manifest_digest,
            batch_digest: batch_digest.into(),
            accepted_replace_scopes: 0,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            accepted_clear_surfaces: 0,
            sealed: false,
            applied: true,
            durable_sequence: 0,
            semantic_content: None,
        }
    }

    /// Attest the content roots the semantic generation sealed.
    pub fn attest_semantic_content(&mut self, roots: SemanticContentRootsV1) {
        self.semantic_content = Some(roots);
    }

    pub fn accept_replace_scope(&mut self) {
        self.accepted_replace_scopes = self.accepted_replace_scopes.saturating_add(1);
    }

    pub fn accept_tombstone_scope(&mut self) {
        self.accepted_tombstone_scopes = self.accepted_tombstone_scopes.saturating_add(1);
    }

    pub fn accept_semantic_replace_scope(&mut self) {
        self.accepted_semantic_replace_scopes =
            self.accepted_semantic_replace_scopes.saturating_add(1);
    }

    pub fn accept_semantic_tombstone_scope(&mut self) {
        self.accepted_semantic_tombstone_scopes =
            self.accepted_semantic_tombstone_scopes.saturating_add(1);
    }

    pub fn accept_clear_surface(&mut self) {
        self.accepted_clear_surfaces = self.accepted_clear_surfaces.saturating_add(1);
    }

    pub fn mark_sealed(&mut self) {
        self.sealed = true;
    }

    /// The receipt of a fresh apply, stamped with the catalog's sequence.
    #[must_use]
    pub fn recorded_at(mut self, durable_sequence: u64) -> Self {
        self.applied = true;
        self.durable_sequence = durable_sequence;
        self
    }

    /// The receipt of an earlier apply, re-issued for a replay of the same
    /// body: the counts and sequence are the original apply's, `applied`
    /// says this call mutated nothing.
    #[must_use]
    pub fn replayed(mut self) -> Self {
        self.applied = false;
        self
    }
}

impl Default for BatchPublishReceipt {
    /// A receipt for no publish at all: generation zero, no manifest, an
    /// empty batch digest and no sequence. Only a scripted transport in a
    /// test answers with it.
    fn default() -> Self {
        Self::empty_for(ManifestGeneration::ZERO, None, String::new())
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
    PublishSearchCorpusBatch(SearchCorpusIngestBatch),
    PublishHistoryBatch(HistoryIngestBatch),
    PublishRepoCommitRecencyBatch(RepoCommitRecencyIngestBatch),
    PublishRepoTopicBatch(RepoTopicIngestBatch),
    PublishFileOwnershipBatch(FileOwnershipIngestBatch),
    PublishFileContributorBatch(FileContributorIngestBatch),
    PublishDirtyBatch(DirtyIngestBatch),
    PublishRuntimeCatalogBatch(RuntimeCatalogIngestBatch),
    PublishStructuralBatch(StructuralIngestBatch),
    PublishRepoMapBundle(RepoMapSourceBundle),
    PublishRepoMetaBatch(RepoMetaIngestBatch),
    PublishRepoDescriptionBatch(RepoDescriptionIngestBatch),
}

const SEARCH_PLANE_INGEST_REQUEST_VARIANTS: &[&str] = &[
    "PublishSearchCorpusBatch",
    "PublishHistoryBatch",
    "PublishRepoCommitRecencyBatch",
    "PublishRepoTopicBatch",
    "PublishFileOwnershipBatch",
    "PublishFileContributorBatch",
    "PublishDirtyBatch",
    "PublishRuntimeCatalogBatch",
    "PublishStructuralBatch",
    "PublishRepoMapBundle",
    "PublishRepoMetaBatch",
    "PublishRepoDescriptionBatch",
];

impl Serialize for SearchPlaneIngestIpcRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::PublishSearchCorpusBatch(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                0,
                "PublishSearchCorpusBatch",
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
            Self::PublishRepoMapBundle(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcRequest",
                9,
                "PublishRepoMapBundle",
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
            "PublishSearchCorpusBatch" => Ok(
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
            "PublishRepoMapBundle" => Ok(SearchPlaneIngestIpcRequest::PublishRepoMapBundle(
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
pub enum SearchPlaneIngestIpcResponse {
    SearchCorpusReceipt(BatchPublishReceipt),
    HistoryReceipt(BatchPublishReceipt),
    RepoCommitRecencyReceipt(BatchPublishReceipt),
    RepoTopicReceipt(BatchPublishReceipt),
    FileOwnershipReceipt(BatchPublishReceipt),
    FileContributorReceipt(BatchPublishReceipt),
    DirtyReceipt(BatchPublishReceipt),
    RuntimeCatalogReceipt(BatchPublishReceipt),
    StructuralReceipt(BatchPublishReceipt),
    RepoMapReceipt(RepoMapMutationAck),
    RepoMetaReceipt(BatchPublishReceipt),
    RepoDescriptionReceipt(BatchPublishReceipt),
    Error(SearchPlaneIpcError),
}

const SEARCH_PLANE_INGEST_RESPONSE_VARIANTS: &[&str] = &[
    "SearchCorpusReceipt",
    "HistoryReceipt",
    "RepoCommitRecencyReceipt",
    "RepoTopicReceipt",
    "FileOwnershipReceipt",
    "FileContributorReceipt",
    "DirtyReceipt",
    "RuntimeCatalogReceipt",
    "StructuralReceipt",
    "RepoMapReceipt",
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
            Self::SearchCorpusReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                0,
                "SearchCorpusReceipt",
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
            Self::RepoMapReceipt(payload) => serializer.serialize_newtype_variant(
                "SearchPlaneIngestIpcResponse",
                9,
                "RepoMapReceipt",
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
            "SearchCorpusReceipt" => Ok(SearchPlaneIngestIpcResponse::SearchCorpusReceipt(
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
            "RepoMapReceipt" => Ok(SearchPlaneIngestIpcResponse::RepoMapReceipt(
                variant.newtype_variant()?,
            )),
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "serde roundtrip tests use assert_eq! for compact proof"
)]
mod tests {
    use super::*;
    use crate::lex::{
        CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, LanguageCode, ParseNode,
        ParseRoleTag, ParseTreeRecord, compute_parse_tree_source_hash,
    };
    use crate::{
        CapabilityStatusV1, ChunkRecord, EmbeddingId, EmbeddingRecord, RepoRelativePath,
        SourceRoleV1,
    };

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
        RepoId::new("repo").expect("static fixture ID satisfies canonical policy")
    }

    fn fixture_revision_id() -> RevisionId {
        RevisionId::new("rev").expect("static fixture ID satisfies canonical policy")
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
            language: LanguageCode::from_code_str("rust").unwrap_or_else(|| std::process::abort()),
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 10,
            text: "fn main() {}".to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }
    }

    fn fixture_embedding_record() -> EmbeddingRecord {
        EmbeddingRecord {
            embedding_id: fixture_embedding_id(),
            record_id: "record-1".to_string().into_boxed_str(),
            owner_kind: crate::OwnerDocKind::Chunk,
            owner_id: "main".to_string().into_boxed_str(),
            corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
            parent_owner_id: Some("file:src/main.rs".to_string().into_boxed_str()),
            source_doc_id: "doc-1".to_string().into_boxed_str(),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: LanguageCode::from_code_str("rust").unwrap_or_else(|| std::process::abort()),
            package: None,
            symbol_kind: None,
            visibility: None,
            source_role: SourceRoleV1::RawFallbackText,
            generated: false,
            capability_status: CapabilityStatusV1::Degraded,
            authority_digest: "auth:feed".to_string().into_boxed_str(),
            render_policy_digest: "render:feed".to_string().into_boxed_str(),
            card_schema_version: 0,
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
            author_name: Some("Alice Example".to_string().into_boxed_str()),
            author_email: Some("alice@example.com".to_string().into_boxed_str()),
            committer: "alice".to_string().into_boxed_str(),
            committer_name: Some("Alice Example".to_string().into_boxed_str()),
            committer_email: Some("alice@example.com".to_string().into_boxed_str()),
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
            lang: LanguageCode::from_code_str("rust").unwrap_or_else(|| std::process::abort()),
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

    /// A token with the canonical batch-digest shape; the contract checks
    /// shape only, the dispatcher proves the value.
    const FIXTURE_BATCH_DIGEST: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn fixture_search_corpus_batch() -> SearchCorpusIngestBatch {
        SearchCorpusIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            base_generation: None,
            manifest_digest: "manifest:feed".to_string(),
            batch_digest: FIXTURE_BATCH_DIGEST.to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SearchCorpusReplaceScope {
                scope: fixture_scope_key(),
                scope_digest: "scope:feed".to_string(),
                chunks: vec![fixture_chunk_record()],
                symbols: vec![],
            }],
            tombstone_scopes: vec![SearchCorpusTombstoneScope {
                scope: SearchScopeKey {
                    doc_surface: SearchScopeSurface::Symbol,
                    repo_relative_path: RepoRelativePath::new("src/main.rs"),
                },
            }],
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
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
            required_corpora: vec![SemanticCorpusKindV1::RawCodeFallback],
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: fixture_scope_key(),
                scope_digest: "scope:feed".to_string(),
                embeddings: vec![fixture_embedding_record()],
                cluster_memberships: Vec::new(),
            }],
            tombstone_scopes: vec![SemanticTombstoneScope {
                scope: Some(SearchScopeKey {
                    doc_surface: SearchScopeSurface::Chunk,
                    repo_relative_path: RepoRelativePath::new("src/old.rs"),
                }),
                semantic_scope: None,
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

    fn fixture_repo_commit_recency_batch() -> RepoCommitRecencyIngestBatch {
        RepoCommitRecencyIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            batch_digest: "batch:repo-commit-recency".to_string(),
            entries: vec![
                RepoCommitRecencyEntry {
                    source_repo_id: RepoId::new("corp-a")
                        .expect("static fixture ID satisfies canonical policy"),
                    latest_committer_time_ms: 1_717_171_717_000,
                },
                RepoCommitRecencyEntry {
                    source_repo_id: RepoId::new("corp-b")
                        .expect("static fixture ID satisfies canonical policy"),
                    latest_committer_time_ms: 1_617_171_717_000,
                },
            ],
        }
    }

    fn fixture_repo_meta_batch() -> RepoMetaIngestBatch {
        RepoMetaIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            batch_digest: "batch:repo-meta".to_string(),
            entries: vec![
                RepoMetaEntry {
                    source_repo_id: RepoId::new("corp-a")
                        .expect("static fixture ID satisfies canonical policy"),
                    key: "license".to_string(),
                    value: "apache-2.0".to_string(),
                },
                RepoMetaEntry {
                    source_repo_id: RepoId::new("corp-b")
                        .expect("static fixture ID satisfies canonical policy"),
                    key: "license".to_string(),
                    value: "gpl-3.0".to_string(),
                },
            ],
        }
    }

    fn fixture_repo_topic_batch() -> RepoTopicIngestBatch {
        RepoTopicIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            batch_digest: "batch:repo-topic".to_string(),
            entries: vec![
                RepoTopicEntry {
                    source_repo_id: RepoId::new("corp-a")
                        .expect("static fixture ID satisfies canonical policy"),
                    topic: "security".to_string(),
                },
                RepoTopicEntry {
                    source_repo_id: RepoId::new("corp-b")
                        .expect("static fixture ID satisfies canonical policy"),
                    topic: "ml".to_string(),
                },
            ],
        }
    }

    fn fixture_repo_description_batch() -> RepoDescriptionIngestBatch {
        RepoDescriptionIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            batch_digest: "batch:repo-description".to_string(),
            entries: vec![
                RepoDescriptionEntry {
                    source_repo_id: RepoId::new("corp-a")
                        .expect("static fixture ID satisfies canonical policy"),
                    description: "Apache distributed systems toolkit".to_string(),
                },
                RepoDescriptionEntry {
                    source_repo_id: RepoId::new("corp-b")
                        .expect("static fixture ID satisfies canonical policy"),
                    description: "Machine-learning training pipelines".to_string(),
                },
            ],
        }
    }

    fn fixture_file_contributor_batch() -> FileContributorIngestBatch {
        FileContributorIngestBatch {
            repo_id: fixture_repo_id(),
            revision_id: fixture_revision_id(),
            generation: fixture_generation(),
            batch_digest: "batch:file-contributor".to_string(),
            entries: vec![
                FileContributorEntry {
                    source_repo_id: RepoId::new("corp-a")
                        .expect("static fixture ID satisfies canonical policy"),
                    repo_relative_path: RepoRelativePath::new("src/gate-a.rs"),
                    contributors: vec![
                        FileContributorIdentityEntry {
                            canonical: "alice".to_string(),
                            name: Some("Alice Example".to_string()),
                            email: Some("alice@example.com".to_string()),
                        },
                        FileContributorIdentityEntry {
                            canonical: "carol".to_string(),
                            name: Some("Carol Example".to_string()),
                            email: Some("carol@example.com".to_string()),
                        },
                    ],
                },
                FileContributorEntry {
                    source_repo_id: RepoId::new("corp-b")
                        .expect("static fixture ID satisfies canonical policy"),
                    repo_relative_path: RepoRelativePath::new("src/gate-b.rs"),
                    contributors: vec![FileContributorIdentityEntry {
                        canonical: "bob".to_string(),
                        name: Some("Bob Example".to_string()),
                        email: Some("bob@example.com".to_string()),
                    }],
                },
            ],
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
    fn search_corpus_ingest_batch_round_trip() -> TestRes {
        let mut batch = fixture_search_corpus_batch();
        batch.clear_surfaces = vec![SearchScopeSurface::Chunk];
        let bytes = encode(&batch)?;
        let decoded: SearchCorpusIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "ciborium f16 path uses aarch64 inline asm that Miri cannot execute; native f32 vec serde is exercised in stable tests + fuzz"
    )]
    fn semantic_ingest_batch_round_trip() -> TestRes {
        let mut batch = fixture_semantic_batch();
        batch.clear_surfaces = vec![SearchScopeSurface::Module];
        let bytes = encode(&batch)?;
        let decoded: SemanticIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    fn legacy_clearless_batches_decode_as_zero_clear_and_receipts_do_not_v1() -> TestRes {
        let mut search_value = serde_json::to_value(fixture_search_corpus_batch())?;
        let _removed_clear_surfaces = search_value
            .as_object_mut()
            .ok_or("search batch fixture must encode as a map")?
            .remove("clear_surfaces");
        let search_batch: SearchCorpusIngestBatch = serde_json::from_value(search_value)?;
        assert!(search_batch.clear_surfaces.is_empty());

        let mut semantic_value = serde_json::to_value(fixture_semantic_batch())?;
        let _removed_clear_surfaces = semantic_value
            .as_object_mut()
            .ok_or("semantic batch fixture must encode as a map")?
            .remove("clear_surfaces");
        let semantic_batch: SemanticIngestBatch = serde_json::from_value(semantic_value)?;
        assert!(semantic_batch.clear_surfaces.is_empty());

        // Receipts are produced only by the search plane and every field is
        // required (QI-BB-032): a receipt without its clear-surface count
        // does not decode instead of reading as zero.
        let mut receipt_value = serde_json::to_value(BatchPublishReceipt::empty_for(
            fixture_generation(),
            Some("manifest:legacy".to_string()),
            "batch:legacy",
        ))?;
        let _removed_accepted_clear_surfaces = receipt_value
            .as_object_mut()
            .ok_or("receipt fixture must encode as a map")?
            .remove("accepted_clear_surfaces");
        assert!(serde_json::from_value::<BatchPublishReceipt>(receipt_value).is_err());

        for field in [
            "accepted_semantic_replace_scopes",
            "accepted_semantic_tombstone_scopes",
        ] {
            let mut receipt_value = serde_json::to_value(BatchPublishReceipt::empty_for(
                fixture_generation(),
                Some("manifest:legacy".to_string()),
                "batch:legacy",
            ))?;
            drop(
                receipt_value
                    .as_object_mut()
                    .ok_or("receipt fixture must encode as a map")?
                    .remove(field),
            );
            assert!(
                serde_json::from_value::<BatchPublishReceipt>(receipt_value).is_err(),
                "missing {field} must fail closed"
            );
        }
        Ok(())
    }

    #[test]
    fn search_corpus_clear_surface_authority_rejects_conflicts_v1() {
        let mut batch = fixture_search_corpus_batch();
        batch.clear_surfaces = vec![SearchScopeSurface::File];
        assert_eq!(
            batch.validate_surface_mutations_v1(),
            Err(SearchCorpusSurfaceMutationConflictV1::ClearAndReplace(
                SearchScopeSurface::File
            ))
        );

        batch.clear_surfaces = vec![SearchScopeSurface::Chunk, SearchScopeSurface::Chunk];
        assert_eq!(
            batch.validate_surface_mutations_v1(),
            Err(SearchCorpusSurfaceMutationConflictV1::DuplicateClear(
                SearchScopeSurface::Chunk
            ))
        );

        batch.clear_surfaces = vec![SearchScopeSurface::Symbol, SearchScopeSurface::Chunk];
        assert_eq!(
            batch.validate_surface_mutations_v1(),
            Err(SearchCorpusSurfaceMutationConflictV1::NonCanonicalClearOrder)
        );

        let mut batch = fixture_search_corpus_batch();
        let duplicate_replace = batch
            .replace_scopes
            .first()
            .expect("fixture must contain one replace scope")
            .clone();
        batch.replace_scopes.push(duplicate_replace);
        assert_eq!(
            batch.validate_surface_mutations_v1(),
            Err(
                SearchCorpusSurfaceMutationConflictV1::DuplicateReplaceScope(
                    SearchScopeSurface::File
                )
            )
        );

        let mut batch = fixture_search_corpus_batch();
        batch
            .tombstone_scopes
            .first_mut()
            .expect("fixture must contain one tombstone scope")
            .scope = fixture_scope_key();
        assert_eq!(
            batch.validate_surface_mutations_v1(),
            Err(SearchCorpusSurfaceMutationConflictV1::ReplaceAndTombstone(
                SearchScopeSurface::File
            ))
        );
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
    fn repo_commit_recency_ingest_batch_round_trip() -> TestRes {
        let batch = fixture_repo_commit_recency_batch();
        let bytes = encode(&batch)?;
        let decoded: RepoCommitRecencyIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    fn repo_meta_ingest_batch_round_trip() -> TestRes {
        let batch = fixture_repo_meta_batch();
        let bytes = encode(&batch)?;
        let decoded: RepoMetaIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    fn repo_topic_ingest_batch_round_trip() -> TestRes {
        let batch = fixture_repo_topic_batch();
        let bytes = encode(&batch)?;
        let decoded: RepoTopicIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    fn file_contributor_ingest_batch_round_trip() -> TestRes {
        let batch = fixture_file_contributor_batch();
        let bytes = encode(&batch)?;
        let decoded: FileContributorIngestBatch = decode(&bytes)?;
        assert_eq!(decoded, batch);
        Ok(())
    }

    #[test]
    fn file_contributor_identity_manual_serde_enforces_wire_contract() -> TestRes {
        let missing_optional: FileContributorIdentityEntry =
            serde_json::from_str(r#"{"canonical":"alice"}"#)?;
        assert_eq!(
            missing_optional,
            FileContributorIdentityEntry {
                canonical: "alice".to_string(),
                name: None,
                email: None,
            }
        );

        let duplicate = serde_json::from_str::<FileContributorIdentityEntry>(
            r#"{"canonical":"alice","canonical":"bob"}"#,
        );
        assert!(matches!(
            duplicate,
            Err(error) if error.to_string().contains("duplicate field `canonical`")
        ));

        let missing = serde_json::from_str::<FileContributorIdentityEntry>(
            r#"{"name":"Alice","email":"alice@example.com"}"#,
        );
        assert!(matches!(
            missing,
            Err(error) if error.to_string().contains("missing field `canonical`")
        ));

        let unknown = serde_json::from_str::<FileContributorIdentityEntry>(
            r#"{"canonical":"alice","unexpected":true}"#,
        );
        assert!(matches!(
            unknown,
            Err(error) if error.to_string().contains("unknown field `unexpected`")
        ));
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
            manifest_digest: Some("sha256:feed".to_string()),
            batch_digest: "batch:fixture".to_string(),
            applied: true,
            durable_sequence: 7,
            semantic_content: None,
            accepted_replace_scopes: 2,
            accepted_tombstone_scopes: 1,
            accepted_semantic_replace_scopes: 3,
            accepted_semantic_tombstone_scopes: 1,
            accepted_clear_surfaces: 0,
            sealed: true,
        };
        let bytes = encode(&receipt)?;
        let decoded: BatchPublishReceipt = decode(&bytes)?;
        assert_eq!(decoded, receipt);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_request_envelope_round_trip_search_corpus() -> TestRes {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 1,
            payload: SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(
                fixture_search_corpus_batch(),
            ),
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
    fn search_plane_ingest_request_envelope_round_trip_repo_commit_recency() -> TestRes {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 4,
            payload: SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(
                fixture_repo_commit_recency_batch(),
            ),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_request_envelope_round_trip_repo_meta() -> TestRes {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 5,
            payload: SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(fixture_repo_meta_batch()),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_request_envelope_round_trip_repo_topic() -> TestRes {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 6,
            payload: SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(fixture_repo_topic_batch()),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_request_envelope_round_trip_repo_description() -> TestRes {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 12,
            payload: SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(
                fixture_repo_description_batch(),
            ),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_request_envelope_round_trip_file_contributor() -> TestRes {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 11,
            payload: SearchPlaneIngestIpcRequest::PublishFileContributorBatch(
                fixture_file_contributor_batch(),
            ),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcRequestEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_request_envelope_round_trip_dirty() -> TestRes {
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id: 6,
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
            request_id: 7,
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
            payload: SearchPlaneIngestIpcResponse::SearchCorpusReceipt(BatchPublishReceipt {
                generation: ManifestGeneration::new(1),
                manifest_digest: Some("digest-lex".to_string()),
                batch_digest: "batch:fixture".to_string(),
                applied: true,
                durable_sequence: 7,
                semantic_content: None,
                accepted_replace_scopes: 1,
                accepted_tombstone_scopes: 0,
                accepted_semantic_replace_scopes: 0,
                accepted_semantic_tombstone_scopes: 0,
                accepted_clear_surfaces: 0,
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
                code: crate::SearchPlaneErrorCodeV2::Internal,
                message: "channel write rejected".to_string(),
                repair: None,
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
                manifest_digest: Some("digest-hist".to_string()),
                batch_digest: "batch:fixture".to_string(),
                applied: true,
                durable_sequence: 7,
                semantic_content: None,
                accepted_replace_scopes: 4,
                accepted_tombstone_scopes: 0,
                accepted_semantic_replace_scopes: 0,
                accepted_semantic_tombstone_scopes: 0,
                accepted_clear_surfaces: 0,
                sealed: false,
            }),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_response_envelope_round_trip_repo_commit_recency_receipt() -> TestRes {
        let envelope = SearchPlaneIngestIpcResponseEnvelope {
            request_id: 6,
            payload: SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(BatchPublishReceipt {
                generation: ManifestGeneration::new(4),
                manifest_digest: Some("digest-repo-commit-recency".to_string()),
                batch_digest: "batch:fixture".to_string(),
                applied: true,
                durable_sequence: 7,
                semantic_content: None,
                accepted_replace_scopes: 2,
                accepted_tombstone_scopes: 0,
                accepted_semantic_replace_scopes: 0,
                accepted_semantic_tombstone_scopes: 0,
                accepted_clear_surfaces: 0,
                sealed: false,
            }),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_response_envelope_round_trip_repo_meta_receipt() -> TestRes {
        let envelope = SearchPlaneIngestIpcResponseEnvelope {
            request_id: 7,
            payload: SearchPlaneIngestIpcResponse::RepoMetaReceipt(BatchPublishReceipt {
                generation: ManifestGeneration::new(5),
                manifest_digest: Some("digest-repo-meta".to_string()),
                batch_digest: "batch:fixture".to_string(),
                applied: true,
                durable_sequence: 7,
                semantic_content: None,
                accepted_replace_scopes: 2,
                accepted_tombstone_scopes: 0,
                accepted_semantic_replace_scopes: 0,
                accepted_semantic_tombstone_scopes: 0,
                accepted_clear_surfaces: 0,
                sealed: false,
            }),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_response_envelope_round_trip_repo_description_receipt() -> TestRes {
        let envelope = SearchPlaneIngestIpcResponseEnvelope {
            request_id: 13,
            payload: SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(BatchPublishReceipt {
                generation: ManifestGeneration::new(7),
                manifest_digest: Some("digest-repo-description".to_string()),
                batch_digest: "batch:fixture".to_string(),
                applied: true,
                durable_sequence: 7,
                semantic_content: None,
                accepted_replace_scopes: 2,
                accepted_tombstone_scopes: 0,
                accepted_semantic_replace_scopes: 0,
                accepted_semantic_tombstone_scopes: 0,
                accepted_clear_surfaces: 0,
                sealed: false,
            }),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_response_envelope_round_trip_repo_topic_receipt() -> TestRes {
        let envelope = SearchPlaneIngestIpcResponseEnvelope {
            request_id: 8,
            payload: SearchPlaneIngestIpcResponse::RepoTopicReceipt(BatchPublishReceipt {
                generation: ManifestGeneration::new(6),
                manifest_digest: Some("digest-repo-topic".to_string()),
                batch_digest: "batch:fixture".to_string(),
                applied: true,
                durable_sequence: 7,
                semantic_content: None,
                accepted_replace_scopes: 2,
                accepted_tombstone_scopes: 0,
                accepted_semantic_replace_scopes: 0,
                accepted_semantic_tombstone_scopes: 0,
                accepted_clear_surfaces: 0,
                sealed: false,
            }),
        };
        let bytes = encode(&envelope)?;
        let decoded: SearchPlaneIngestIpcResponseEnvelope = decode(&bytes)?;
        assert_eq!(decoded, envelope);
        Ok(())
    }

    #[test]
    fn search_plane_ingest_response_envelope_round_trip_file_contributor_receipt() -> TestRes {
        let envelope = SearchPlaneIngestIpcResponseEnvelope {
            request_id: 12,
            payload: SearchPlaneIngestIpcResponse::FileContributorReceipt(BatchPublishReceipt {
                generation: ManifestGeneration::new(7),
                manifest_digest: Some("digest-file-contributor".to_string()),
                batch_digest: "batch:fixture".to_string(),
                applied: true,
                durable_sequence: 7,
                semantic_content: None,
                accepted_replace_scopes: 2,
                accepted_tombstone_scopes: 0,
                accepted_semantic_replace_scopes: 0,
                accepted_semantic_tombstone_scopes: 0,
                accepted_clear_surfaces: 0,
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
            request_id: 8,
            payload: SearchPlaneIngestIpcResponse::DirtyReceipt(BatchPublishReceipt {
                generation: ManifestGeneration::new(4),
                manifest_digest: Some("digest-dirty".to_string()),
                batch_digest: "batch:fixture".to_string(),
                applied: true,
                durable_sequence: 7,
                semantic_content: None,
                accepted_replace_scopes: 1,
                accepted_tombstone_scopes: 1,
                accepted_semantic_replace_scopes: 0,
                accepted_semantic_tombstone_scopes: 0,
                accepted_clear_surfaces: 0,
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
            request_id: 9,
            payload: SearchPlaneIngestIpcResponse::StructuralReceipt(BatchPublishReceipt {
                generation: ManifestGeneration::new(5),
                manifest_digest: Some("digest-struct".to_string()),
                batch_digest: "batch:fixture".to_string(),
                applied: true,
                durable_sequence: 7,
                semantic_content: None,
                accepted_replace_scopes: 1,
                accepted_tombstone_scopes: 0,
                accepted_semantic_replace_scopes: 0,
                accepted_semantic_tombstone_scopes: 0,
                accepted_clear_surfaces: 0,
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
        let result = BatchIngestMode::deserialize(bad);
        assert!(
            result.is_err(),
            "unknown batch ingest mode tag should fail closed: {result:?}"
        );
        let Err(err) = result else {
            return;
        };
        assert!(err.to_string().contains("unknown variant"));
    }

    // QI-BB-029: the batch shape is refused by the contract before any
    // adapter observes it, one defect per refusal, and a well-formed batch
    // of either mode passes.
    #[test]
    fn search_corpus_batch_shape_is_validated_before_any_adapter() -> TestRes {
        let mut batch = fixture_search_corpus_batch();
        batch.validate_v1()?;

        batch.base_generation = Some(ManifestGeneration::new(1));
        assert!(matches!(
            batch.validate_v1(),
            Err(SearchCorpusBatchShapeErrorV1::ModeBaseMismatch {
                mode: BatchIngestMode::ReplaceGeneration,
                ..
            })
        ));

        batch.mode = BatchIngestMode::Delta;
        batch.base_generation = None;
        assert!(matches!(
            batch.validate_v1(),
            Err(SearchCorpusBatchShapeErrorV1::ModeBaseMismatch {
                mode: BatchIngestMode::Delta,
                base_generation: None,
            })
        ));

        batch.base_generation = Some(batch.generation);
        assert!(matches!(
            batch.validate_v1(),
            Err(SearchCorpusBatchShapeErrorV1::BaseNotOlderThanTarget { .. })
        ));

        batch.base_generation = Some(ManifestGeneration::new(
            batch.generation.get().saturating_sub(1),
        ));
        batch.validate_v1()?;

        for value in ["", "has space"] {
            let mut malformed = fixture_search_corpus_batch();
            malformed.manifest_digest = value.to_string();
            assert_eq!(
                malformed.validate_v1(),
                Err(SearchCorpusBatchShapeErrorV1::DigestNotCanonical {
                    field: "manifest_digest"
                }),
                "value {value:?}"
            );
        }
        // The batch digest must have the canonical shape: 64 lowercase hex.
        let uppercase = FIXTURE_BATCH_DIGEST.to_ascii_uppercase();
        for value in ["", "batch:feed", "tab\there", "\u{e9}", &uppercase] {
            let mut malformed = fixture_search_corpus_batch();
            malformed.batch_digest = value.to_string();
            assert_eq!(
                malformed.validate_v1(),
                Err(SearchCorpusBatchShapeErrorV1::BatchDigestNotCanonical),
                "value {value:?}"
            );
        }
        Ok(())
    }
}
