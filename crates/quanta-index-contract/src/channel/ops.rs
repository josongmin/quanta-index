//! Lexical build op values.
//!
//! These ops never cross a wire: the enum has no serde and no persisted op
//! stream exists. They are the in-process vocabulary the lexical adapter's
//! build port consumes, lowered from typed search-corpus ingest batches
//! (`FullBundle`, `ClearLexicalSurface`, `ReplaceLexicalScope`,
//! `TombstoneLexicalScope`, `Seal`). The remaining variants (`UpsertChunk`,
//! `UpsertSymbol`, `UpsertCommit`, `UpsertRef`, `UpsertParseTree`,
//! `ReplaceStructuralScope`) have no production constructor; only adapter
//! and ledger test fixtures build them, and they are slated for deletion
//! once those fixtures ingest through the batch surface instead.
//!
//! Each enum variant carries a struct (rather than tuple). Op payloads are
//! opaque `Vec<u8>` blobs at this layer; the encoding is owned by the lexical
//! module. See the per-variant doc-comments for what each payload actually
//! carries — formats vary across variants (raw UTF-8, CBOR, opaque
//! bookkeeping) and are NOT all CBOR.

use crate::{ManifestGeneration, RepoId, RevisionId, SearchScopeSurface};

use super::ids::{ChunkId, SymbolId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalFullBundle {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    /// Opaque manifest blob reserved for producer-side bookkeeping; not
    /// consumed by the reference adapters.
    pub payload: Vec<u8>,
}

/// Test-fixture op: index one chunk's text directly.
///
/// Production ingest carries chunks inside `ReplaceLexicalScope`; this op
/// has no production constructor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertChunk {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub chunk_id: ChunkId,
    /// Raw UTF-8 chunk text bytes. The reference lexical adapter decodes via
    /// `String::from_utf8_lossy`; producers must emit valid UTF-8 to get
    /// search-correct results.
    pub payload: Vec<u8>,
}

/// Test-fixture op: index one symbol directly. No production constructor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertSymbol {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub symbol_id: SymbolId,
    /// Opaque payload reserved for future symbol indexing (not consumed by
    /// the reference adapter today).
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalSeal {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
}

/// Test-fixture op: upsert a commit record into the auxiliary ledger.
///
/// `payload` is an opaque commit blob the ledger decodes. No production
/// constructor; history ingest goes through `HistoryIngestBatch`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertCommit {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub payload: Vec<u8>,
}

/// Test-fixture op: upsert a ref pointer.
///
/// `name` is the fully qualified ref name (e.g. `refs/heads/main`); `sha`
/// is the 20-byte SHA-1 commit identifier the ref points at. No production
/// constructor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertRef {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub name: Box<str>,
    pub sha: [u8; 20],
}

/// Test-fixture op: upsert a parse-tree blob for a chunk.
///
/// `payload` is the CBOR-encoded parse-tree record the structural authority
/// decodes. No production constructor; structural ingest goes through
/// `StructuralIngestBatch`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertParseTree {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub chunk_id: ChunkId,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplaceLexicalScope {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TombstoneLexicalScope {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub payload: Vec<u8>,
}

/// Whole-surface deletion for a target lexical generation. `base_generation`
/// is carried explicitly so a clear-only Delta can materialize its target by
/// cloning the immutable base before deleting rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClearLexicalSurface {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub base_generation: Option<ManifestGeneration>,
    pub surface: SearchScopeSurface,
}

/// Test-fixture op: replace one structural scope's parse trees.
///
/// No production constructor; structural ingest goes through
/// `StructuralIngestBatch`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplaceStructuralScope {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LexicalChannelOp {
    FullBundle(LexicalFullBundle),
    UpsertChunk(UpsertChunk),
    UpsertSymbol(UpsertSymbol),
    ReplaceLexicalScope(ReplaceLexicalScope),
    TombstoneLexicalScope(TombstoneLexicalScope),
    ClearLexicalSurface(ClearLexicalSurface),
    Seal(LexicalSeal),
    UpsertCommit(UpsertCommit),
    UpsertRef(UpsertRef),
    UpsertParseTree(UpsertParseTree),
    ReplaceStructuralScope(ReplaceStructuralScope),
}

impl LexicalChannelOp {
    #[must_use]
    pub fn repo_id(&self) -> &RepoId {
        match self {
            Self::FullBundle(op) => &op.repo_id,
            Self::UpsertChunk(op) => &op.repo_id,
            Self::UpsertSymbol(op) => &op.repo_id,
            Self::ReplaceLexicalScope(op) => &op.repo_id,
            Self::TombstoneLexicalScope(op) => &op.repo_id,
            Self::ClearLexicalSurface(op) => &op.repo_id,
            Self::Seal(op) => &op.repo_id,
            Self::UpsertCommit(op) => &op.repo_id,
            Self::UpsertRef(op) => &op.repo_id,
            Self::UpsertParseTree(op) => &op.repo_id,
            Self::ReplaceStructuralScope(op) => &op.repo_id,
        }
    }

    #[must_use]
    pub fn revision_id(&self) -> &RevisionId {
        match self {
            Self::FullBundle(op) => &op.revision_id,
            Self::UpsertChunk(op) => &op.revision_id,
            Self::UpsertSymbol(op) => &op.revision_id,
            Self::ReplaceLexicalScope(op) => &op.revision_id,
            Self::TombstoneLexicalScope(op) => &op.revision_id,
            Self::ClearLexicalSurface(op) => &op.revision_id,
            Self::Seal(op) => &op.revision_id,
            Self::UpsertCommit(op) => &op.revision_id,
            Self::UpsertRef(op) => &op.revision_id,
            Self::UpsertParseTree(op) => &op.revision_id,
            Self::ReplaceStructuralScope(op) => &op.revision_id,
        }
    }

    #[must_use]
    pub fn generation(&self) -> ManifestGeneration {
        match self {
            Self::FullBundle(op) => op.generation,
            Self::UpsertChunk(op) => op.generation,
            Self::UpsertSymbol(op) => op.generation,
            Self::ReplaceLexicalScope(op) => op.generation,
            Self::TombstoneLexicalScope(op) => op.generation,
            Self::ClearLexicalSurface(op) => op.generation,
            Self::Seal(op) => op.generation,
            Self::UpsertCommit(op) => op.generation,
            Self::UpsertRef(op) => op.generation,
            Self::UpsertParseTree(op) => op.generation,
            Self::ReplaceStructuralScope(op) => op.generation,
        }
    }

    #[must_use]
    pub fn is_seal(&self) -> bool {
        matches!(self, Self::Seal(_))
    }
}
