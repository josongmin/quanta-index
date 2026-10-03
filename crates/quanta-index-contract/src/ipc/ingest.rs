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
//! `check-rust-derive-allowlist.py`); the wire format difference between the two is
//! intentional for the new ingest surface.

use core::fmt;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, SeqAccess, VariantAccess, Visitor},
    ser::SerializeStruct,
};

use crate::lex::{
    CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, ParseTreeRecord, SymbolRecord,
};
use crate::{
    ChunkId, ChunkRecord, EmbeddingRecord, ManifestGeneration, OwnerDocKind, RepoId,
    RepoRelativePath, RevisionId, SourceFileCoverage, SourceFileKey, SourcePublicationEvent,
};

use super::semantic_source::{
    ClusterMembershipReplaceV1, SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1,
};
use crate::semantic_kinds::SemanticCorpusKindV1;

mod payload_digest;
pub use payload_digest::source_event_payload_sha256;

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
    RecordPathMismatch(SearchScopeSurface),
    DuplicateCandidateId,
    InvalidCoverage,
    CoverageUnitMismatch,
    RecordSourceMismatch,
    RecordLanguageMismatch,
    InvalidRecordRange,
    SourceBytesDigestMismatch,
    ChunkSourceMismatch,
}

impl fmt::Display for SearchCorpusSurfaceMutationConflictV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCoverage => {
                formatter.write_str("invalid source-file coverage or tombstone identity")
            }
            Self::CoverageUnitMismatch => {
                formatter.write_str("coverage state or unit digest disagrees with supplied units")
            }
            Self::RecordSourceMismatch => {
                formatter.write_str("record source repo differs from replacement file owner")
            }
            Self::RecordLanguageMismatch => {
                formatter.write_str("record language differs from replacement file language")
            }
            Self::InvalidRecordRange => {
                formatter.write_str("record byte or line span is inconsistent with supplied text")
            }
            Self::SourceBytesDigestMismatch => {
                formatter.write_str("source bytes SHA-256 disagrees with source-file coverage")
            }
            Self::ChunkSourceMismatch => {
                formatter.write_str("chunk text disagrees with its source-file byte span")
            }
            Self::DuplicateClear(surface) => {
                write!(formatter, "duplicate clear for search surface {surface:?}")
            }
            Self::NonCanonicalClearOrder => {
                formatter.write_str("search surface clears must use canonical ascending order")
            }
            Self::DuplicateReplaceScope(surface) => {
                write!(
                    formatter,
                    "duplicate replace scope on search surface {surface:?}"
                )
            }
            Self::DuplicateTombstoneScope(surface) => {
                write!(
                    formatter,
                    "duplicate tombstone scope on search surface {surface:?}"
                )
            }
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
            Self::RecordPathMismatch(surface) => write!(
                formatter,
                "search {surface:?} record path must equal its replacement file path"
            ),
            Self::DuplicateCandidateId => {
                formatter.write_str("search candidate IDs must be unique across chunks and symbols")
            }
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
    pub coverage: SourceFileCoverage,
    /// Complete source bytes bound by `coverage.source.source_sha256`.
    /// File-oriented search must verify against this authority, not the
    /// potentially partial and overlapping chunk set.
    pub source_bytes: Vec<u8>,
    pub chunks: Vec<ChunkRecord>,
    pub symbols: Vec<SymbolRecord>,
}

const SEARCH_CORPUS_REPLACE_SCOPE_FIELDS: &[&str] =
    &["coverage", "source_bytes", "chunks", "symbols"];

// CBOR must carry a source file as one byte string. The default Vec<u8>
// serializer emits an array of integers, inflating ingest frames and forcing
// the decoder to allocate an element sequence for every file byte.
struct SourceBytesWire<'a>(&'a [u8]);

impl Serialize for SourceBytesWire<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0)
    }
}

struct SourceBytesBuf(Vec<u8>);

struct SourceBytesVisitor;

impl<'de> Visitor<'de> for SourceBytesVisitor {
    type Value = SourceBytesBuf;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("source file bytes")
    }

    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        Ok(SourceBytesBuf(value.to_vec()))
    }

    fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
        Ok(SourceBytesBuf(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut bytes = Vec::new();
        while let Some(byte) = seq.next_element::<u8>()? {
            bytes.push(byte);
        }
        Ok(SourceBytesBuf(bytes))
    }
}

impl<'de> Deserialize<'de> for SourceBytesBuf {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_byte_buf(SourceBytesVisitor)
    }
}

impl Serialize for SearchCorpusReplaceScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchCorpusReplaceScope", 4)?;
        state.serialize_field("coverage", &self.coverage)?;
        state.serialize_field("source_bytes", &SourceBytesWire(&self.source_bytes))?;
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
        let mut coverage: Option<SourceFileCoverage> = None;
        let mut source_bytes: Option<Vec<u8>> = None;
        let mut chunks: Option<Vec<ChunkRecord>> = None;
        let mut symbols: Option<Vec<SymbolRecord>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "coverage" => {
                    if coverage.is_some() {
                        return Err(de::Error::duplicate_field("coverage"));
                    }
                    coverage = Some(map.next_value()?);
                }
                "source_bytes" => {
                    if source_bytes.is_some() {
                        return Err(de::Error::duplicate_field("source_bytes"));
                    }
                    source_bytes = Some(map.next_value::<SourceBytesBuf>()?.0);
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
            coverage: coverage.ok_or_else(|| de::Error::missing_field("coverage"))?,
            source_bytes: source_bytes.ok_or_else(|| de::Error::missing_field("source_bytes"))?,
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
    pub file: SourceFileKey,
}

const SEARCH_CORPUS_TOMBSTONE_SCOPE_FIELDS: &[&str] = &["file"];

impl Serialize for SearchCorpusTombstoneScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchCorpusTombstoneScope", 1)?;
        state.serialize_field("file", &self.file)?;
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
        let mut file: Option<SourceFileKey> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "file" => {
                    if file.is_some() {
                        return Err(de::Error::duplicate_field("file"));
                    }
                    file = Some(map.next_value()?);
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
            file: file.ok_or_else(|| de::Error::missing_field("file"))?,
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
    /// Producer source identity, independent of this materialization target.
    pub source_event: SourcePublicationEvent,
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
    "source_event",
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
        let mut state = serializer.serialize_struct("SearchCorpusIngestBatch", 15)?;
        state.serialize_field("source_event", &self.source_event)?;
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
        let mut source_event: Option<SourcePublicationEvent> = None;
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
                "source_event" => {
                    if source_event.is_some() {
                        return Err(de::Error::duplicate_field("source_event"));
                    }
                    source_event = Some(map.next_value()?);
                }
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
            source_event: source_event.ok_or_else(|| de::Error::missing_field("source_event"))?,
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            base_generation: base_generation
                .ok_or_else(|| de::Error::missing_field("base_generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            mode: mode.ok_or_else(|| de::Error::missing_field("mode"))?,
            bundle_payload: bundle_payload
                .ok_or_else(|| de::Error::missing_field("bundle_payload"))?,
            clear_surfaces: clear_surfaces
                .ok_or_else(|| de::Error::missing_field("clear_surfaces"))?,
            replace_scopes: replace_scopes
                .ok_or_else(|| de::Error::missing_field("replace_scopes"))?,
            tombstone_scopes: tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("tombstone_scopes"))?,
            semantic_replace_scopes: semantic_replace_scopes
                .ok_or_else(|| de::Error::missing_field("semantic_replace_scopes"))?,
            semantic_tombstone_scopes: semantic_tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("semantic_tombstone_scopes"))?,
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

mod validation;
pub use validation::{SearchCorpusBatchShapeErrorV1, validate_lexical_file_mutations_v1};

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
            cluster_memberships: cluster_memberships
                .ok_or_else(|| de::Error::missing_field("cluster_memberships"))?,
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
    pub semantic_scope: SemanticSourceScopeKeyV1,
}

const SEMANTIC_TOMBSTONE_SCOPE_FIELDS: &[&str] = &["semantic_scope"];

impl Serialize for SemanticTombstoneScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticTombstoneScope", 1)?;
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
        let mut semantic_scope: Option<SemanticSourceScopeKeyV1> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
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
        let semantic_scope =
            semantic_scope.ok_or_else(|| de::Error::missing_field("semantic_scope"))?;
        if semantic_scope.owner_id.is_empty() {
            return Err(de::Error::custom(
                "semantic tombstone owner_id must not be empty",
            ));
        }
        Ok(SemanticTombstoneScope { semantic_scope })
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
            required_corpora: required_corpora
                .ok_or_else(|| de::Error::missing_field("required_corpora"))?,
            corpus_policy_digest: corpus_policy_digest
                .ok_or_else(|| de::Error::missing_field("corpus_policy_digest"))?,
            clear_surfaces: clear_surfaces
                .ok_or_else(|| de::Error::missing_field("clear_surfaces"))?,
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
            name: name.ok_or_else(|| de::Error::missing_field("name"))?,
            email: email.ok_or_else(|| de::Error::missing_field("email"))?,
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

mod envelope;
pub use envelope::*;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "serde roundtrip tests use assert_eq! for compact proof"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "fixed nonempty fixture vectors intentionally fail the test if their shape changes"
)]
mod tests;
