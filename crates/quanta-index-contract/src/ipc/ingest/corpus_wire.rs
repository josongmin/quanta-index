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
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
    ser::SerializeStruct,
};

mod admitted;
pub use admitted::CorpusDecodeLimitsV1;
use admitted::{CorpusDecodePolicy, Ordinary};

/// Classify a wire map key against its existing schema without creating an
/// owned String for each known field. The returned key is the schema literal.
struct CorpusFieldKeySeed {
    fields: &'static [&'static str],
}

impl<'de> DeserializeSeed<'de> for CorpusFieldKeySeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_identifier(self)
    }
}

impl CorpusFieldKeySeed {
    fn classify<E: de::Error>(self, key: &str) -> Result<&'static str, E> {
        self.fields
            .iter()
            .copied()
            .find(|field| *field == key)
            .ok_or_else(|| de::Error::unknown_field(key, self.fields))
    }
}

impl Visitor<'_> for CorpusFieldKeySeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a corpus field name")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        self.classify(key)
    }

    fn visit_string<E: de::Error>(self, key: String) -> Result<Self::Value, E> {
        self.classify(&key)
    }
}

#[cfg(test)]
mod corpus_field_key_tests {
    use super::{CorpusFieldKeySeed, SEARCH_SCOPE_KEY_FIELDS, SearchScopeKey, SearchScopeSurface};
    use crate::RepoRelativePath;
    use serde::de::{DeserializeSeed, value};

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn known_keys_resolve_to_static_schema_literals() -> TestRes {
        const JSON: &str = "\"doc_surface\"";
        let expected = SEARCH_SCOPE_KEY_FIELDS
            .first()
            .ok_or("missing doc_surface schema field")?;

        let mut json = serde_json::Deserializer::from_str(JSON);
        let json_key = CorpusFieldKeySeed {
            fields: SEARCH_SCOPE_KEY_FIELDS,
        }
        .deserialize(&mut json)?;
        if !core::ptr::eq(json_key.as_ptr(), expected.as_ptr()) {
            return Err("JSON key did not resolve to schema literal".into());
        }

        let borrowed = value::BorrowedStrDeserializer::<value::Error>::new("doc_surface");
        let borrowed_key = CorpusFieldKeySeed {
            fields: SEARCH_SCOPE_KEY_FIELDS,
        }
        .deserialize(borrowed)?;
        if !core::ptr::eq(borrowed_key.as_ptr(), expected.as_ptr()) {
            return Err("borrowed key did not resolve to schema literal".into());
        }

        // A deserializer that already owns the key still returns the schema
        // literal, so the caller never retains a parser-owned String.
        let owned = value::StringDeserializer::<value::Error>::new("doc_surface".to_owned());
        let owned_key = CorpusFieldKeySeed {
            fields: SEARCH_SCOPE_KEY_FIELDS,
        }
        .deserialize(owned)?;
        if !core::ptr::eq(owned_key.as_ptr(), expected.as_ptr()) {
            return Err("owned key did not resolve to schema literal".into());
        }
        Ok(())
    }

    #[test]
    fn fixed_json_and_cbor_map_wires_preserve_key_behavior() -> TestRes {
        const JSON_SCOPE: &str = r#"{"doc_surface":"File","repo_relative_path":"src/main.rs"}"#;
        const CBOR_SCOPE: &[u8] =
            b"\xa2\x6bdoc_surface\x64File\x72repo_relative_path\x6bsrc/main.rs";
        let expected = SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
        };
        let json: SearchScopeKey = serde_json::from_str(JSON_SCOPE)?;
        let cbor: SearchScopeKey = ciborium::from_reader(CBOR_SCOPE)?;
        if json != expected || cbor != expected {
            return Err("fixed JSON or CBOR map changed SearchScopeKey".into());
        }

        let mut json = serde_json::Deserializer::from_str("\"alien\"");
        if (CorpusFieldKeySeed {
            fields: SEARCH_SCOPE_KEY_FIELDS,
        })
        .deserialize(&mut json)
        .is_ok()
        {
            return Err("unknown JSON key was accepted".into());
        }

        for wire in [&b"\xa1\x65alien\xf5"[..], &b"\xa1\x01\xf5"[..]] {
            if ciborium::from_reader::<SearchScopeKey, _>(wire).is_ok() {
                return Err("unknown or nontext CBOR key was accepted".into());
            }
        }
        Ok(())
    }

    #[test]
    fn known_noncanonical_cbor_key_forms_are_refused() {
        // An indefinite text key concatenates to the known "doc_surface"
        // spelling. Identifier decoding intentionally accepts only finite
        // text keys, matching the canonical writer's wire form.
        const INDEFINITE_TEXT_KEY: &[u8] =
            b"\xa2\x7f\x64doc_\x67surface\xff\x64File\x72repo_relative_path\x6bsrc/main.rs";
        // CBOR byte strings are not text keys, even if their bytes spell a
        // known field name.
        const BYTE_STRING_KEY: &[u8] =
            b"\xa2\x4bdoc_surface\x64File\x72repo_relative_path\x6bsrc/main.rs";
        for wire in [INDEFINITE_TEXT_KEY, BYTE_STRING_KEY] {
            assert!(
                ciborium::from_reader::<SearchScopeKey, _>(wire).is_err(),
                "noncanonical CBOR field key was accepted"
            );
        }
    }
}

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

struct SearchScopeKeyVisitor<P>(P);

impl<'de, P: CorpusDecodePolicy<'de>> Visitor<'de> for SearchScopeKeyVisitor<P> {
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
        while let Some(key) = self.0.next_field(&mut map, SEARCH_SCOPE_KEY_FIELDS)? {
            match key {
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
                    repo_relative_path = Some(self.0.next_path(&mut map)?);
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
            SearchScopeKeyVisitor(Ordinary),
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

struct SearchCorpusReplaceScopeVisitor<P>(P);

impl<'de, P: CorpusDecodePolicy<'de>> Visitor<'de> for SearchCorpusReplaceScopeVisitor<P> {
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
        while let Some(key) = self
            .0
            .next_field(&mut map, SEARCH_CORPUS_REPLACE_SCOPE_FIELDS)?
        {
            match key {
                "coverage" => {
                    if coverage.is_some() {
                        return Err(de::Error::duplicate_field("coverage"));
                    }
                    coverage = Some(self.0.next_coverage(&mut map)?);
                }
                "source_bytes" => {
                    if source_bytes.is_some() {
                        return Err(de::Error::duplicate_field("source_bytes"));
                    }
                    source_bytes = Some(self.0.next_source_bytes(&mut map)?);
                }
                "chunks" => {
                    if chunks.is_some() {
                        return Err(de::Error::duplicate_field("chunks"));
                    }
                    chunks = Some(self.0.next_chunks(&mut map)?);
                }
                "symbols" => {
                    if symbols.is_some() {
                        return Err(de::Error::duplicate_field("symbols"));
                    }
                    symbols = Some(self.0.next_symbols(&mut map)?);
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
            SearchCorpusReplaceScopeVisitor(Ordinary),
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

struct SearchCorpusTombstoneScopeVisitor<P>(P);

impl<'de, P: CorpusDecodePolicy<'de>> Visitor<'de> for SearchCorpusTombstoneScopeVisitor<P> {
    type Value = SearchCorpusTombstoneScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchCorpusTombstoneScope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut file: Option<SourceFileKey> = None;
        while let Some(key) = self
            .0
            .next_field(&mut map, SEARCH_CORPUS_TOMBSTONE_SCOPE_FIELDS)?
        {
            match key {
                "file" => {
                    if file.is_some() {
                        return Err(de::Error::duplicate_field("file"));
                    }
                    file = Some(self.0.next_file(&mut map)?);
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
            SearchCorpusTombstoneScopeVisitor(Ordinary),
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

struct SearchCorpusIngestBatchVisitor<P>(P);

impl<'de, P: CorpusDecodePolicy<'de>> Visitor<'de> for SearchCorpusIngestBatchVisitor<P> {
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
        while let Some(key) = self
            .0
            .next_field(&mut map, SEARCH_CORPUS_INGEST_BATCH_FIELDS)?
        {
            match key {
                "source_event" => {
                    if source_event.is_some() {
                        return Err(de::Error::duplicate_field("source_event"));
                    }
                    source_event = Some(self.0.next_event(&mut map)?);
                }
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(self.0.next_repo(&mut map)?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(self.0.next_revision(&mut map)?);
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
                    manifest_digest = Some(self.0.next_digest(&mut map)?);
                }
                "batch_digest" => {
                    if batch_digest.is_some() {
                        return Err(de::Error::duplicate_field("batch_digest"));
                    }
                    batch_digest = Some(self.0.next_digest(&mut map)?);
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
                    bundle_payload = Some(self.0.next_bundle(&mut map)?);
                }
                "clear_surfaces" => {
                    if clear_surfaces.is_some() {
                        return Err(de::Error::duplicate_field("clear_surfaces"));
                    }
                    clear_surfaces = Some(self.0.next_clear(&mut map)?);
                }
                "replace_scopes" => {
                    if replace_scopes.is_some() {
                        return Err(de::Error::duplicate_field("replace_scopes"));
                    }
                    replace_scopes = Some(self.0.next_replace(&mut map)?);
                }
                "tombstone_scopes" => {
                    if tombstone_scopes.is_some() {
                        return Err(de::Error::duplicate_field("tombstone_scopes"));
                    }
                    tombstone_scopes = Some(self.0.next_tombstone(&mut map)?);
                }
                "semantic_replace_scopes" => {
                    if semantic_replace_scopes.is_some() {
                        return Err(de::Error::duplicate_field("semantic_replace_scopes"));
                    }
                    semantic_replace_scopes = Some(self.0.next_semantic_replace(&mut map)?);
                }
                "semantic_tombstone_scopes" => {
                    if semantic_tombstone_scopes.is_some() {
                        return Err(de::Error::duplicate_field("semantic_tombstone_scopes"));
                    }
                    semantic_tombstone_scopes = Some(self.0.next_semantic_tombstone(&mut map)?);
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
            SearchCorpusIngestBatchVisitor(Ordinary),
        )
    }
}
