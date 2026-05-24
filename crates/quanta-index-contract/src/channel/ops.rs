//! Channel op values.
//!
//! Each enum variant carries a struct (rather than tuple) so additional optional
//! fields can be added later without source breaking changes for existing producer
//! call sites. Op payloads carry opaque CBOR-encoded bodies — the schema is owned
//! by the lexical / semantic modules, not by transport.

use crate::{ManifestGeneration, RepoId, RevisionId};

use super::ids::{ChunkId, EmbeddingId, SymbolId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalFullBundle {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    /// CBOR-encoded `LexicalBundlePayload` (schema owned by the lexical module).
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertChunk {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub chunk_id: ChunkId,
    /// CBOR-encoded chunk record (text + metadata) owned by the lexical module.
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteChunk {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub chunk_id: ChunkId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertSymbol {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub symbol_id: SymbolId,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteSymbol {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub symbol_id: SymbolId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalSeal {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LexicalChannelOp {
    FullBundle(LexicalFullBundle),
    UpsertChunk(UpsertChunk),
    DeleteChunk(DeleteChunk),
    UpsertSymbol(UpsertSymbol),
    DeleteSymbol(DeleteSymbol),
    Seal(LexicalSeal),
}

impl LexicalChannelOp {
    #[must_use]
    pub fn repo_id(&self) -> &RepoId {
        match self {
            Self::FullBundle(op) => &op.repo_id,
            Self::UpsertChunk(op) => &op.repo_id,
            Self::DeleteChunk(op) => &op.repo_id,
            Self::UpsertSymbol(op) => &op.repo_id,
            Self::DeleteSymbol(op) => &op.repo_id,
            Self::Seal(op) => &op.repo_id,
        }
    }

    #[must_use]
    pub fn revision_id(&self) -> &RevisionId {
        match self {
            Self::FullBundle(op) => &op.revision_id,
            Self::UpsertChunk(op) => &op.revision_id,
            Self::DeleteChunk(op) => &op.revision_id,
            Self::UpsertSymbol(op) => &op.revision_id,
            Self::DeleteSymbol(op) => &op.revision_id,
            Self::Seal(op) => &op.revision_id,
        }
    }

    #[must_use]
    pub fn generation(&self) -> ManifestGeneration {
        match self {
            Self::FullBundle(op) => op.generation,
            Self::UpsertChunk(op) => op.generation,
            Self::DeleteChunk(op) => op.generation,
            Self::UpsertSymbol(op) => op.generation,
            Self::DeleteSymbol(op) => op.generation,
            Self::Seal(op) => op.generation,
        }
    }

    #[must_use]
    pub fn is_seal(&self) -> bool {
        matches!(self, Self::Seal(_))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticFullBundle {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertEmbedding {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub embedding_id: EmbeddingId,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteEmbedding {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub embedding_id: EmbeddingId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticSeal {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SemanticChannelOp {
    FullBundle(SemanticFullBundle),
    UpsertEmbedding(UpsertEmbedding),
    DeleteEmbedding(DeleteEmbedding),
    Seal(SemanticSeal),
}

impl SemanticChannelOp {
    #[must_use]
    pub fn repo_id(&self) -> &RepoId {
        match self {
            Self::FullBundle(op) => &op.repo_id,
            Self::UpsertEmbedding(op) => &op.repo_id,
            Self::DeleteEmbedding(op) => &op.repo_id,
            Self::Seal(op) => &op.repo_id,
        }
    }

    #[must_use]
    pub fn revision_id(&self) -> &RevisionId {
        match self {
            Self::FullBundle(op) => &op.revision_id,
            Self::UpsertEmbedding(op) => &op.revision_id,
            Self::DeleteEmbedding(op) => &op.revision_id,
            Self::Seal(op) => &op.revision_id,
        }
    }

    #[must_use]
    pub fn generation(&self) -> ManifestGeneration {
        match self {
            Self::FullBundle(op) => op.generation,
            Self::UpsertEmbedding(op) => op.generation,
            Self::DeleteEmbedding(op) => op.generation,
            Self::Seal(op) => op.generation,
        }
    }

    #[must_use]
    pub fn is_seal(&self) -> bool {
        matches!(self, Self::Seal(_))
    }
}
