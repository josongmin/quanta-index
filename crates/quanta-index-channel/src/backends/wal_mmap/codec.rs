//! Op-specific frame body encoding.
//!
//! The transport (segment + frame header) is op-agnostic. This module supplies
//! the per-track body codec — one for [`LexicalCodec`] and one for
//! [`SemanticCodec`]. Each codec is a zero-sized type so the publisher /
//! subscriber generic does not require a value-bearing parameter.

use quanta_index_contract::{
    ChannelSeq, ChunkId, DeleteChunk, DeleteEmbedding, DeleteSymbol, EmbeddingId, LexicalChannelOp,
    LexicalFullBundle, LexicalSeal, ManifestGeneration, RepoId, RevisionId, SemanticChannelOp,
    SemanticFullBundle, SemanticSeal, SymbolId, UpsertChunk, UpsertEmbedding, UpsertSymbol,
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
            LexicalChannelOp::Seal(LexicalSeal {
                repo_id,
                revision_id,
                generation,
            }) => {
                write_common(out, repo_id, revision_id, *generation)?;
                Ok(LexicalOpTag::Seal.to_byte())
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
            LexicalOpTag::Seal => {
                reader.finish()?;
                Ok(LexicalChannelOp::Seal(LexicalSeal {
                    repo_id,
                    revision_id,
                    generation,
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
    Seal = 16,
}

impl SemanticOpTag {
    const fn to_byte(self) -> u8 {
        match self {
            Self::FullBundle => 11,
            Self::UpsertEmbedding => 12,
            Self::DeleteEmbedding => 13,
            Self::Seal => 16,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            11 => Self::FullBundle,
            12 => Self::UpsertEmbedding,
            13 => Self::DeleteEmbedding,
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
