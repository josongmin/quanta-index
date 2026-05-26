//! Op-specific frame body encoding.
//!
//! The transport (segment + frame header) is op-agnostic. This module supplies
//! the per-track body codec — one for [`LexicalCodec`] and one for
//! [`SemanticCodec`]. Each codec is a zero-sized type so the publisher /
//! subscriber generic does not require a value-bearing parameter.

use quanta_index_contract::channel::{ChannelSeq, LexicalChannelOp, SemanticChannelOp};
use quanta_index_contract::{
    ChunkId, DeleteChunk, DeleteEmbedding, DeleteParseTree, DeleteRef, DeleteSymbol, DeleteTag,
    EmbeddingId, EvictDirty, LexicalFullBundle, LexicalSeal, ManifestGeneration,
    ReplaceLexicalScope, ReplaceSemanticScope, ReplaceStructuralScope, RepoId, RevisionId,
    SemanticFullBundle, SemanticSeal, SymbolId, TombstoneLexicalScope, TombstoneSemanticScope,
    TombstoneStructuralScope, UpsertChunk, UpsertCommit, UpsertDiffHunk, UpsertDirty,
    UpsertEmbedding, UpsertParseTree, UpsertRef, UpsertSymbol, UpsertTag,
};

use crate::api::error::ChannelError;

use super::{LexicalChannelEvent, SemanticChannelEvent};

/// Max length of an embedded length-prefixed string or byte vector inside a
/// single frame body. Mirrored against the segment frame cap.
pub const MAX_FIELD_LEN: u32 = 16 * 1024 * 1024;

/// Codec interface for a single channel track.
pub trait OpCodec: Clone + Send + Sync + 'static {
    type Op: Clone + Send + Sync;
    type Event: Clone + Send + Sync;

    /// Encode the op body — does NOT include the outer length prefix or crc.
    /// Returns body bytes and the `op_tag` byte.
    fn encode_op(op: &Self::Op, out: &mut Vec<u8>) -> Result<u8, ChannelError>;

    /// Decode a frame body back into an op.
    fn decode_op(body: &[u8]) -> Result<Self::Op, ChannelError>;

    /// Wrap (seq, op) into the codec's event shape.
    fn event(seq: ChannelSeq, op: Self::Op) -> Self::Event;

    /// Best-effort detection: is this op a Seal marker (fsync trigger)?
    fn is_seal(op: &Self::Op) -> bool;

    /// Construct a Seal op for the codec's track. Used by `seal()` on the
    /// publisher trait to avoid leaking codec-specific constructors into
    /// generic publisher code.
    fn make_seal(repo: RepoId, revision: RevisionId, generation: ManifestGeneration) -> Self::Op;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LexicalCodec;

impl OpCodec for LexicalCodec {
    type Op = LexicalChannelOp;
    type Event = LexicalChannelEvent;

    fn encode_op(op: &Self::Op, out: &mut Vec<u8>) -> Result<u8, ChannelError> {
        match op {
            LexicalChannelOp::FullBundle(LexicalFullBundle {
                repo_id,
                revision_id,
                generation,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_bytes(out, payload)?;
                Ok(LexicalOpTag::FullBundle.to_byte())
            }
            LexicalChannelOp::UpsertChunk(UpsertChunk {
                repo_id,
                revision_id,
                generation,
                chunk_id,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, chunk_id.as_str())?;
                write_bytes(out, payload)?;
                Ok(LexicalOpTag::UpsertChunk.to_byte())
            }
            LexicalChannelOp::DeleteChunk(DeleteChunk {
                repo_id,
                revision_id,
                generation,
                chunk_id,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, chunk_id.as_str())?;
                Ok(LexicalOpTag::DeleteChunk.to_byte())
            }
            LexicalChannelOp::UpsertSymbol(UpsertSymbol {
                repo_id,
                revision_id,
                generation,
                symbol_id,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, symbol_id.as_str())?;
                write_bytes(out, payload)?;
                Ok(LexicalOpTag::UpsertSymbol.to_byte())
            }
            LexicalChannelOp::DeleteSymbol(DeleteSymbol {
                repo_id,
                revision_id,
                generation,
                symbol_id,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, symbol_id.as_str())?;
                Ok(LexicalOpTag::DeleteSymbol.to_byte())
            }
            LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
                repo_id,
                revision_id,
                generation,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_bytes(out, payload)?;
                Ok(LexicalOpTag::ReplaceLexicalScope.to_byte())
            }
            LexicalChannelOp::TombstoneLexicalScope(TombstoneLexicalScope {
                repo_id,
                revision_id,
                generation,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_bytes(out, payload)?;
                Ok(LexicalOpTag::TombstoneLexicalScope.to_byte())
            }
            LexicalChannelOp::Seal(LexicalSeal {
                repo_id,
                revision_id,
                generation,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                Ok(LexicalOpTag::Seal.to_byte())
            }
            LexicalChannelOp::UpsertCommit(UpsertCommit {
                repo_id,
                revision_id,
                generation,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_bytes(out, payload)?;
                Ok(LexicalOpTag::UpsertCommit.to_byte())
            }
            LexicalChannelOp::UpsertRef(UpsertRef {
                repo_id,
                revision_id,
                generation,
                name,
                sha,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, name)?;
                out.extend_from_slice(sha);
                Ok(LexicalOpTag::UpsertRef.to_byte())
            }
            LexicalChannelOp::UpsertTag(UpsertTag {
                repo_id,
                revision_id,
                generation,
                name,
                sha,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, name)?;
                out.extend_from_slice(sha);
                Ok(LexicalOpTag::UpsertTag.to_byte())
            }
            LexicalChannelOp::DeleteRef(DeleteRef {
                repo_id,
                revision_id,
                generation,
                name,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, name)?;
                Ok(LexicalOpTag::DeleteRef.to_byte())
            }
            LexicalChannelOp::DeleteTag(DeleteTag {
                repo_id,
                revision_id,
                generation,
                name,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, name)?;
                Ok(LexicalOpTag::DeleteTag.to_byte())
            }
            LexicalChannelOp::UpsertDirty(UpsertDirty {
                repo_id,
                revision_id,
                generation,
                doc_id,
                applied_at_ms,
                payload_hash,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, doc_id.as_str())?;
                out.extend_from_slice(&applied_at_ms.to_le_bytes());
                out.extend_from_slice(payload_hash);
                Ok(LexicalOpTag::UpsertDirty.to_byte())
            }
            LexicalChannelOp::EvictDirty(EvictDirty {
                repo_id,
                revision_id,
                generation,
                doc_id,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, doc_id.as_str())?;
                Ok(LexicalOpTag::EvictDirty.to_byte())
            }
            LexicalChannelOp::UpsertParseTree(UpsertParseTree {
                repo_id,
                revision_id,
                generation,
                chunk_id,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, chunk_id.as_str())?;
                write_bytes(out, payload)?;
                Ok(LexicalOpTag::UpsertParseTree.to_byte())
            }
            LexicalChannelOp::DeleteParseTree(DeleteParseTree {
                repo_id,
                revision_id,
                generation,
                chunk_id,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, chunk_id.as_str())?;
                Ok(LexicalOpTag::DeleteParseTree.to_byte())
            }
            LexicalChannelOp::ReplaceStructuralScope(ReplaceStructuralScope {
                repo_id,
                revision_id,
                generation,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_bytes(out, payload)?;
                Ok(LexicalOpTag::ReplaceStructuralScope.to_byte())
            }
            LexicalChannelOp::TombstoneStructuralScope(TombstoneStructuralScope {
                repo_id,
                revision_id,
                generation,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_bytes(out, payload)?;
                Ok(LexicalOpTag::TombstoneStructuralScope.to_byte())
            }
            LexicalChannelOp::UpsertDiffHunk(UpsertDiffHunk {
                repo_id,
                revision_id,
                generation,
                commit_sha,
                file_path,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                out.extend_from_slice(commit_sha);
                write_str(out, file_path)?;
                write_bytes(out, payload)?;
                Ok(LexicalOpTag::UpsertDiffHunk.to_byte())
            }
        }
    }

    fn decode_op(body: &[u8]) -> Result<Self::Op, ChannelError> {
        let mut reader = BodyReader::new(body);
        let tag_byte = reader.read_u8()?;
        let tag = LexicalOpTag::from_u8(tag_byte)
            .ok_or_else(|| ChannelError::Encoding(format!("unknown lexical tag {tag_byte}")))?;
        let (repo_id, revision_id, generation) = read_common(&mut reader)?;
        match tag {
            LexicalOpTag::FullBundle => {
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(LexicalChannelOp::FullBundle(LexicalFullBundle {
                    repo_id,
                    revision_id,
                    generation,
                    payload,
                }))
            }
            LexicalOpTag::UpsertChunk => {
                let chunk_id = ChunkId::new(reader.read_string()?);
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(LexicalChannelOp::UpsertChunk(UpsertChunk {
                    repo_id,
                    revision_id,
                    generation,
                    chunk_id,
                    payload,
                }))
            }
            LexicalOpTag::DeleteChunk => {
                let chunk_id = ChunkId::new(reader.read_string()?);
                reader.finish()?;
                Ok(LexicalChannelOp::DeleteChunk(DeleteChunk {
                    repo_id,
                    revision_id,
                    generation,
                    chunk_id,
                }))
            }
            LexicalOpTag::UpsertSymbol => {
                let symbol_id = SymbolId::new(reader.read_string()?);
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(LexicalChannelOp::UpsertSymbol(UpsertSymbol {
                    repo_id,
                    revision_id,
                    generation,
                    symbol_id,
                    payload,
                }))
            }
            LexicalOpTag::DeleteSymbol => {
                let symbol_id = SymbolId::new(reader.read_string()?);
                reader.finish()?;
                Ok(LexicalChannelOp::DeleteSymbol(DeleteSymbol {
                    repo_id,
                    revision_id,
                    generation,
                    symbol_id,
                }))
            }
            LexicalOpTag::ReplaceLexicalScope => {
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
                    repo_id,
                    revision_id,
                    generation,
                    payload,
                }))
            }
            LexicalOpTag::TombstoneLexicalScope => {
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(LexicalChannelOp::TombstoneLexicalScope(
                    TombstoneLexicalScope {
                        repo_id,
                        revision_id,
                        generation,
                        payload,
                    },
                ))
            }
            LexicalOpTag::Seal => {
                reader.finish()?;
                Ok(LexicalChannelOp::Seal(LexicalSeal {
                    repo_id,
                    revision_id,
                    generation,
                }))
            }
            LexicalOpTag::UpsertCommit => {
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(LexicalChannelOp::UpsertCommit(UpsertCommit {
                    repo_id,
                    revision_id,
                    generation,
                    payload,
                }))
            }
            LexicalOpTag::UpsertRef => {
                let name = reader.read_string()?.into_boxed_str();
                let sha: [u8; 20] = reader
                    .advance(20)?
                    .try_into()
                    .map_err(|_err| ChannelError::Encoding("sha truncated".to_string()))?;
                reader.finish()?;
                Ok(LexicalChannelOp::UpsertRef(UpsertRef {
                    repo_id,
                    revision_id,
                    generation,
                    name,
                    sha,
                }))
            }
            LexicalOpTag::UpsertTag => {
                let name = reader.read_string()?.into_boxed_str();
                let sha: [u8; 20] = reader
                    .advance(20)?
                    .try_into()
                    .map_err(|_err| ChannelError::Encoding("sha truncated".to_string()))?;
                reader.finish()?;
                Ok(LexicalChannelOp::UpsertTag(UpsertTag {
                    repo_id,
                    revision_id,
                    generation,
                    name,
                    sha,
                }))
            }
            LexicalOpTag::DeleteRef => {
                let name = reader.read_string()?.into_boxed_str();
                reader.finish()?;
                Ok(LexicalChannelOp::DeleteRef(DeleteRef {
                    repo_id,
                    revision_id,
                    generation,
                    name,
                }))
            }
            LexicalOpTag::DeleteTag => {
                let name = reader.read_string()?.into_boxed_str();
                reader.finish()?;
                Ok(LexicalChannelOp::DeleteTag(DeleteTag {
                    repo_id,
                    revision_id,
                    generation,
                    name,
                }))
            }
            LexicalOpTag::UpsertDirty => {
                let doc_id = ChunkId::new(reader.read_string()?);
                let applied_at_ms = reader.read_u64()?;
                let payload_hash: [u8; 32] = reader
                    .advance(32)?
                    .try_into()
                    .map_err(|_err| ChannelError::Encoding("payload_hash truncated".to_string()))?;
                reader.finish()?;
                Ok(LexicalChannelOp::UpsertDirty(UpsertDirty {
                    repo_id,
                    revision_id,
                    generation,
                    doc_id,
                    applied_at_ms,
                    payload_hash,
                }))
            }
            LexicalOpTag::EvictDirty => {
                let doc_id = ChunkId::new(reader.read_string()?);
                reader.finish()?;
                Ok(LexicalChannelOp::EvictDirty(EvictDirty {
                    repo_id,
                    revision_id,
                    generation,
                    doc_id,
                }))
            }
            LexicalOpTag::UpsertParseTree => {
                let chunk_id = ChunkId::new(reader.read_string()?);
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(LexicalChannelOp::UpsertParseTree(UpsertParseTree {
                    repo_id,
                    revision_id,
                    generation,
                    chunk_id,
                    payload,
                }))
            }
            LexicalOpTag::DeleteParseTree => {
                let chunk_id = ChunkId::new(reader.read_string()?);
                reader.finish()?;
                Ok(LexicalChannelOp::DeleteParseTree(DeleteParseTree {
                    repo_id,
                    revision_id,
                    generation,
                    chunk_id,
                }))
            }
            LexicalOpTag::ReplaceStructuralScope => {
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(LexicalChannelOp::ReplaceStructuralScope(
                    ReplaceStructuralScope {
                        repo_id,
                        revision_id,
                        generation,
                        payload,
                    },
                ))
            }
            LexicalOpTag::TombstoneStructuralScope => {
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(LexicalChannelOp::TombstoneStructuralScope(
                    TombstoneStructuralScope {
                        repo_id,
                        revision_id,
                        generation,
                        payload,
                    },
                ))
            }
            LexicalOpTag::UpsertDiffHunk => {
                let commit_sha: [u8; 20] = reader
                    .advance(20)?
                    .try_into()
                    .map_err(|_err| ChannelError::Encoding("commit_sha truncated".to_string()))?;
                let file_path = reader.read_string()?.into_boxed_str();
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(LexicalChannelOp::UpsertDiffHunk(UpsertDiffHunk {
                    repo_id,
                    revision_id,
                    generation,
                    commit_sha,
                    file_path,
                    payload,
                }))
            }
        }
    }

    fn event(seq: ChannelSeq, op: Self::Op) -> Self::Event {
        LexicalChannelEvent { seq, op }
    }

    fn is_seal(op: &Self::Op) -> bool {
        op.is_seal()
    }

    fn make_seal(repo: RepoId, revision: RevisionId, generation: ManifestGeneration) -> Self::Op {
        LexicalChannelOp::Seal(LexicalSeal {
            repo_id: repo,
            revision_id: revision,
            generation,
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SemanticCodec;

impl OpCodec for SemanticCodec {
    type Op = SemanticChannelOp;
    type Event = SemanticChannelEvent;

    fn encode_op(op: &Self::Op, out: &mut Vec<u8>) -> Result<u8, ChannelError> {
        match op {
            SemanticChannelOp::FullBundle(SemanticFullBundle {
                repo_id,
                revision_id,
                generation,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_bytes(out, payload)?;
                Ok(SemanticOpTag::FullBundle.to_byte())
            }
            SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
                repo_id,
                revision_id,
                generation,
                embedding_id,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, embedding_id.as_str())?;
                write_bytes(out, payload)?;
                Ok(SemanticOpTag::UpsertEmbedding.to_byte())
            }
            SemanticChannelOp::DeleteEmbedding(DeleteEmbedding {
                repo_id,
                revision_id,
                generation,
                embedding_id,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_str(out, embedding_id.as_str())?;
                Ok(SemanticOpTag::DeleteEmbedding.to_byte())
            }
            SemanticChannelOp::ReplaceSemanticScope(ReplaceSemanticScope {
                repo_id,
                revision_id,
                generation,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_bytes(out, payload)?;
                Ok(SemanticOpTag::ReplaceSemanticScope.to_byte())
            }
            SemanticChannelOp::TombstoneSemanticScope(TombstoneSemanticScope {
                repo_id,
                revision_id,
                generation,
                payload,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                write_bytes(out, payload)?;
                Ok(SemanticOpTag::TombstoneSemanticScope.to_byte())
            }
            SemanticChannelOp::Seal(SemanticSeal {
                repo_id,
                revision_id,
                generation,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                Ok(SemanticOpTag::Seal.to_byte())
            }
        }
    }

    fn decode_op(body: &[u8]) -> Result<Self::Op, ChannelError> {
        let mut reader = BodyReader::new(body);
        let tag_byte = reader.read_u8()?;
        let tag = SemanticOpTag::from_u8(tag_byte)
            .ok_or_else(|| ChannelError::Encoding(format!("unknown semantic tag {tag_byte}")))?;
        let (repo_id, revision_id, generation) = read_common(&mut reader)?;
        match tag {
            SemanticOpTag::FullBundle => {
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(SemanticChannelOp::FullBundle(SemanticFullBundle {
                    repo_id,
                    revision_id,
                    generation,
                    payload,
                }))
            }
            SemanticOpTag::UpsertEmbedding => {
                let embedding_id = EmbeddingId::new(reader.read_string()?);
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
                    repo_id,
                    revision_id,
                    generation,
                    embedding_id,
                    payload,
                }))
            }
            SemanticOpTag::DeleteEmbedding => {
                let embedding_id = EmbeddingId::new(reader.read_string()?);
                reader.finish()?;
                Ok(SemanticChannelOp::DeleteEmbedding(DeleteEmbedding {
                    repo_id,
                    revision_id,
                    generation,
                    embedding_id,
                }))
            }
            SemanticOpTag::ReplaceSemanticScope => {
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(SemanticChannelOp::ReplaceSemanticScope(
                    ReplaceSemanticScope {
                        repo_id,
                        revision_id,
                        generation,
                        payload,
                    },
                ))
            }
            SemanticOpTag::TombstoneSemanticScope => {
                let payload = reader.read_bytes()?;
                reader.finish()?;
                Ok(SemanticChannelOp::TombstoneSemanticScope(
                    TombstoneSemanticScope {
                        repo_id,
                        revision_id,
                        generation,
                        payload,
                    },
                ))
            }
            SemanticOpTag::Seal => {
                reader.finish()?;
                Ok(SemanticChannelOp::Seal(SemanticSeal {
                    repo_id,
                    revision_id,
                    generation,
                }))
            }
        }
    }

    fn event(seq: ChannelSeq, op: Self::Op) -> Self::Event {
        SemanticChannelEvent { seq, op }
    }

    fn is_seal(op: &Self::Op) -> bool {
        op.is_seal()
    }

    fn make_seal(repo: RepoId, revision: RevisionId, generation: ManifestGeneration) -> Self::Op {
        SemanticChannelOp::Seal(SemanticSeal {
            repo_id: repo,
            revision_id: revision,
            generation,
        })
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LexicalOpTag {
    FullBundle = 1,
    UpsertChunk = 2,
    DeleteChunk = 3,
    UpsertSymbol = 4,
    DeleteSymbol = 5,
    Seal = 6,
    UpsertCommit = 7,
    UpsertRef = 8,
    UpsertTag = 9,
    DeleteRef = 10,
    DeleteTag = 11,
    UpsertDirty = 12,
    EvictDirty = 13,
    UpsertParseTree = 14,
    DeleteParseTree = 15,
    UpsertDiffHunk = 16,
    ReplaceLexicalScope = 17,
    TombstoneLexicalScope = 18,
    ReplaceStructuralScope = 19,
    TombstoneStructuralScope = 20,
}

impl LexicalOpTag {
    const fn to_byte(self) -> u8 {
        match self {
            Self::FullBundle => 1,
            Self::UpsertChunk => 2,
            Self::DeleteChunk => 3,
            Self::UpsertSymbol => 4,
            Self::DeleteSymbol => 5,
            Self::Seal => 6,
            Self::UpsertCommit => 7,
            Self::UpsertRef => 8,
            Self::UpsertTag => 9,
            Self::DeleteRef => 10,
            Self::DeleteTag => 11,
            Self::UpsertDirty => 12,
            Self::EvictDirty => 13,
            Self::UpsertParseTree => 14,
            Self::DeleteParseTree => 15,
            Self::UpsertDiffHunk => 16,
            Self::ReplaceLexicalScope => 17,
            Self::TombstoneLexicalScope => 18,
            Self::ReplaceStructuralScope => 19,
            Self::TombstoneStructuralScope => 20,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::FullBundle,
            2 => Self::UpsertChunk,
            3 => Self::DeleteChunk,
            4 => Self::UpsertSymbol,
            5 => Self::DeleteSymbol,
            6 => Self::Seal,
            7 => Self::UpsertCommit,
            8 => Self::UpsertRef,
            9 => Self::UpsertTag,
            10 => Self::DeleteRef,
            11 => Self::DeleteTag,
            12 => Self::UpsertDirty,
            13 => Self::EvictDirty,
            14 => Self::UpsertParseTree,
            15 => Self::DeleteParseTree,
            16 => Self::UpsertDiffHunk,
            17 => Self::ReplaceLexicalScope,
            18 => Self::TombstoneLexicalScope,
            19 => Self::ReplaceStructuralScope,
            20 => Self::TombstoneStructuralScope,
            _ => return None,
        })
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SemanticOpTag {
    FullBundle = 11,
    UpsertEmbedding = 12,
    DeleteEmbedding = 13,
    ReplaceSemanticScope = 14,
    TombstoneSemanticScope = 15,
    Seal = 16,
}

impl SemanticOpTag {
    const fn to_byte(self) -> u8 {
        match self {
            Self::FullBundle => 11,
            Self::UpsertEmbedding => 12,
            Self::DeleteEmbedding => 13,
            Self::ReplaceSemanticScope => 14,
            Self::TombstoneSemanticScope => 15,
            Self::Seal => 16,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            11 => Self::FullBundle,
            12 => Self::UpsertEmbedding,
            13 => Self::DeleteEmbedding,
            14 => Self::ReplaceSemanticScope,
            15 => Self::TombstoneSemanticScope,
            16 => Self::Seal,
            _ => return None,
        })
    }
}

fn write_common(
    out: &mut Vec<u8>,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
) -> Result<(), ChannelError> {
    out.extend_from_slice(&generation.get().to_le_bytes());
    write_str(out, repo.as_str())?;
    write_str(out, revision.as_str())?;
    Ok(())
}

fn write_str(out: &mut Vec<u8>, value: &str) -> Result<(), ChannelError> {
    write_bytes(out, value.as_bytes())
}

fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), ChannelError> {
    let len = u32::try_from(bytes.len()).map_err(|_err| {
        ChannelError::Encoding(format!(
            "field length {} exceeds {MAX_FIELD_LEN}",
            bytes.len()
        ))
    })?;
    if len > MAX_FIELD_LEN {
        return Err(ChannelError::Encoding(format!(
            "field length {len} exceeds {MAX_FIELD_LEN}"
        )));
    }
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn read_common(
    reader: &mut BodyReader<'_>,
) -> Result<(RepoId, RevisionId, ManifestGeneration), ChannelError> {
    let generation = ManifestGeneration::new(reader.read_u64()?);
    let repo = RepoId::new(reader.read_string()?);
    let revision = RevisionId::new(reader.read_string()?);
    Ok((repo, revision, generation))
}

pub struct BodyReader<'a> {
    body: &'a [u8],
    pos: usize,
}

impl<'a> BodyReader<'a> {
    fn new(body: &'a [u8]) -> Self {
        Self { body, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.body.len().saturating_sub(self.pos)
    }

    fn advance(&mut self, n: usize) -> Result<&'a [u8], ChannelError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| ChannelError::Encoding("body offset overflow".to_string()))?;
        let slice = self
            .body
            .get(self.pos..end)
            .ok_or_else(|| ChannelError::Encoding("body truncated".to_string()))?;
        self.pos = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, ChannelError> {
        let slice = self.advance(1)?;
        slice
            .first()
            .copied()
            .ok_or_else(|| ChannelError::Encoding("u8 truncated".to_string()))
    }

    fn read_u32(&mut self) -> Result<u32, ChannelError> {
        let slice = self.advance(4)?;
        let arr: [u8; 4] = slice
            .try_into()
            .map_err(|_err| ChannelError::Encoding("u32 truncated".to_string()))?;
        Ok(u32::from_le_bytes(arr))
    }

    fn read_u64(&mut self) -> Result<u64, ChannelError> {
        let slice = self.advance(8)?;
        let arr: [u8; 8] = slice
            .try_into()
            .map_err(|_err| ChannelError::Encoding("u64 truncated".to_string()))?;
        Ok(u64::from_le_bytes(arr))
    }

    fn read_bytes(&mut self) -> Result<Vec<u8>, ChannelError> {
        let len = self.read_u32()?;
        if len > MAX_FIELD_LEN {
            return Err(ChannelError::Encoding(format!(
                "field length {len} exceeds {MAX_FIELD_LEN}"
            )));
        }
        let len_usize = usize::try_from(len)
            .map_err(|_err| ChannelError::Encoding("usize conversion".to_string()))?;
        let slice = self.advance(len_usize)?;
        Ok(slice.to_vec())
    }

    fn read_string(&mut self) -> Result<String, ChannelError> {
        let bytes = self.read_bytes()?;
        String::from_utf8(bytes)
            .map_err(|err| ChannelError::Encoding(format!("invalid utf-8: {err}")))
    }

    fn finish(&self) -> Result<(), ChannelError> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(ChannelError::Encoding(format!(
                "trailing {} bytes after frame body",
                self.remaining()
            )))
        }
    }
}
