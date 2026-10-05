//! Lexical and source-corpus ingest wire DTOs.

use super::super::semantic_source::{SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1};
use crate::lex::SymbolRecord;
use crate::semantic_kinds::SemanticCorpusKindV1;
use crate::{
    ChunkRecord, ManifestGeneration, OwnerDocKind, RepoId, RepoRelativePath, RevisionId,
    SourceFileCoverage, SourceFileKey, SourcePublicationEvent,
};
use core::fmt;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, SeqAccess, Visitor},
    ser::SerializeStruct,
};

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
    SymbolNameSourceMismatch,
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
            Self::SymbolNameSourceMismatch => formatter
                .write_str("attested ASCII symbol local name is absent from its source span"),
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
pub(super) struct SourceBytesWire<'a>(pub(super) &'a [u8]);

impl Serialize for SourceBytesWire<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0)
    }
}

pub(super) struct SourceBytesBuf(pub(super) Vec<u8>);

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
