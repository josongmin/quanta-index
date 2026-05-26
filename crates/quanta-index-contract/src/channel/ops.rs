//! Channel op values.
//!
//! Each enum variant carries a struct (rather than tuple) so additional optional
//! fields can be added later without source breaking changes for existing producer
//! call sites. Op payloads are opaque `Vec<u8>` blobs at the transport layer;
//! the wire format is owned by the lexical / semantic modules. See the
//! per-variant doc-comments for what each payload actually carries — formats
//! vary across variants (raw UTF-8, CBOR, opaque bookkeeping) and are NOT all
//! CBOR despite the earlier blanket claim that lived on this module.

use crate::{ManifestGeneration, RepoId, RevisionId};

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
    /// Opaque payload reserved for future symbol indexing (not consumed by
    /// the reference adapter today).
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

/// PRE-CONTRACT-EXT additive op: upsert a commit metadata record into the
/// lexical channel. `payload` is an opaque producer-side commit blob; the
/// reference adapter does not introspect its bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertCommit {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub payload: Vec<u8>,
}

/// PRE-CONTRACT-EXT additive op: upsert a ref pointer. `name` is the fully
/// qualified ref name (e.g. `refs/heads/main`). `sha` is the 20-byte SHA-1
/// commit identifier the ref points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertRef {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub name: Box<str>,
    pub sha: [u8; 20],
}

/// PRE-CONTRACT-EXT additive op: upsert a tag pointer. Shape mirrors
/// [`UpsertRef`]; tags are tracked in a separate ops domain because the
/// downstream history index treats them as a distinct namespace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertTag {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub name: Box<str>,
    pub sha: [u8; 20],
}

/// PRE-CONTRACT-EXT additive op: delete a ref pointer by name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteRef {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub name: Box<str>,
}

/// PRE-CONTRACT-EXT additive op: delete a tag pointer by name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteTag {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub name: Box<str>,
}

/// PRE-CONTRACT-EXT additive op: upsert a working-tree dirty doc shadow.
///
/// `applied_at_ms` is unix epoch milliseconds at the producer side and is
/// used to break ties when concurrent dirty edits race. `payload_hash` is a
/// 32-byte content digest used for idempotency checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertDirty {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub doc_id: ChunkId,
    pub applied_at_ms: u64,
    pub payload_hash: [u8; 32],
}

/// PRE-CONTRACT-EXT additive op: evict a dirty doc shadow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvictDirty {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub doc_id: ChunkId,
}

/// PRE-CONTRACT-EXT additive op: upsert a parse-tree blob for a chunk.
/// `payload` is opaque to the channel layer — the parse-tree adapter owns the
/// wire format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertParseTree {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub chunk_id: ChunkId,
    pub payload: Vec<u8>,
}

/// PRE-CONTRACT-EXT additive op: delete a parse-tree blob for a chunk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteParseTree {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub chunk_id: ChunkId,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplaceStructuralScope {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TombstoneStructuralScope {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub payload: Vec<u8>,
}

/// PRE-CONTRACT-EXT additive op: upsert a diff hunk record for a commit.
///
/// `commit_sha` is the 20-byte SHA-1 the hunk belongs to. `file_path` is a
/// repo-relative path string. `payload` is the opaque diff hunk blob.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpsertDiffHunk {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub commit_sha: [u8; 20],
    pub file_path: Box<str>,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LexicalChannelOp {
    FullBundle(LexicalFullBundle),
    UpsertChunk(UpsertChunk),
    DeleteChunk(DeleteChunk),
    UpsertSymbol(UpsertSymbol),
    DeleteSymbol(DeleteSymbol),
    ReplaceLexicalScope(ReplaceLexicalScope),
    TombstoneLexicalScope(TombstoneLexicalScope),
    Seal(LexicalSeal),
    // PRE-CONTRACT-EXT additive variants — appended only; existing variants
    // above keep their order and shape for wire compatibility.
    UpsertCommit(UpsertCommit),
    UpsertRef(UpsertRef),
    UpsertTag(UpsertTag),
    DeleteRef(DeleteRef),
    DeleteTag(DeleteTag),
    UpsertDirty(UpsertDirty),
    EvictDirty(EvictDirty),
    UpsertParseTree(UpsertParseTree),
    DeleteParseTree(DeleteParseTree),
    ReplaceStructuralScope(ReplaceStructuralScope),
    TombstoneStructuralScope(TombstoneStructuralScope),
    UpsertDiffHunk(UpsertDiffHunk),
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
            Self::ReplaceLexicalScope(op) => &op.repo_id,
            Self::TombstoneLexicalScope(op) => &op.repo_id,
            Self::Seal(op) => &op.repo_id,
            Self::UpsertCommit(op) => &op.repo_id,
            Self::UpsertRef(op) => &op.repo_id,
            Self::UpsertTag(op) => &op.repo_id,
            Self::DeleteRef(op) => &op.repo_id,
            Self::DeleteTag(op) => &op.repo_id,
            Self::UpsertDirty(op) => &op.repo_id,
            Self::EvictDirty(op) => &op.repo_id,
            Self::UpsertParseTree(op) => &op.repo_id,
            Self::DeleteParseTree(op) => &op.repo_id,
            Self::ReplaceStructuralScope(op) => &op.repo_id,
            Self::TombstoneStructuralScope(op) => &op.repo_id,
            Self::UpsertDiffHunk(op) => &op.repo_id,
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
            Self::ReplaceLexicalScope(op) => &op.revision_id,
            Self::TombstoneLexicalScope(op) => &op.revision_id,
            Self::Seal(op) => &op.revision_id,
            Self::UpsertCommit(op) => &op.revision_id,
            Self::UpsertRef(op) => &op.revision_id,
            Self::UpsertTag(op) => &op.revision_id,
            Self::DeleteRef(op) => &op.revision_id,
            Self::DeleteTag(op) => &op.revision_id,
            Self::UpsertDirty(op) => &op.revision_id,
            Self::EvictDirty(op) => &op.revision_id,
            Self::UpsertParseTree(op) => &op.revision_id,
            Self::DeleteParseTree(op) => &op.revision_id,
            Self::ReplaceStructuralScope(op) => &op.revision_id,
            Self::TombstoneStructuralScope(op) => &op.revision_id,
            Self::UpsertDiffHunk(op) => &op.revision_id,
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
            Self::ReplaceLexicalScope(op) => op.generation,
            Self::TombstoneLexicalScope(op) => op.generation,
            Self::Seal(op) => op.generation,
            Self::UpsertCommit(op) => op.generation,
            Self::UpsertRef(op) => op.generation,
            Self::UpsertTag(op) => op.generation,
            Self::DeleteRef(op) => op.generation,
            Self::DeleteTag(op) => op.generation,
            Self::UpsertDirty(op) => op.generation,
            Self::EvictDirty(op) => op.generation,
            Self::UpsertParseTree(op) => op.generation,
            Self::DeleteParseTree(op) => op.generation,
            Self::ReplaceStructuralScope(op) => op.generation,
            Self::TombstoneStructuralScope(op) => op.generation,
            Self::UpsertDiffHunk(op) => op.generation,
        }
    }

    #[must_use]
    pub fn is_seal(&self) -> bool {
        matches!(self, Self::Seal(_))
    }
}

// -- PRE-CONTRACT-EXT additive manual serde impls -------------------------
//
// Hand-rolled because:
// * workspace bans serde proc-macro derives (semgrep rule
//   `rust-no-serde-derive`)
// * wire shape must be auditable in review
// * unknown-field rejection must be fail-closed (no silent drop)
//
// All ten new structs use the same shape: a CBOR map with stable field names,
// `unknown_field` rejected via `de::Error::unknown_field`, and missing-field
// rejection via `de::Error::missing_field`. The 20-byte `sha` / `commit_sha`
// and 32-byte `payload_hash` arrays serialize as CBOR major-type-2 byte
// strings.

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self as serde_de, MapAccess, Visitor},
    ser::SerializeStruct,
};

fn box_str_from_string(s: String) -> Box<str> {
    s.into_boxed_str()
}

fn sha20_from_bytes<E>(bytes: &[u8]) -> Result<[u8; 20], E>
where
    E: serde_de::Error,
{
    let len = bytes.len();
    <[u8; 20]>::try_from(bytes)
        .map_err(|_err| E::invalid_length(len, &"expected a 20-byte SHA-1 commit identifier"))
}

fn hash32_from_bytes<E>(bytes: &[u8]) -> Result<[u8; 32], E>
where
    E: serde_de::Error,
{
    let len = bytes.len();
    <[u8; 32]>::try_from(bytes)
        .map_err(|_err| E::invalid_length(len, &"expected a 32-byte payload hash"))
}

const UPSERT_COMMIT_FIELDS: &[&str] = &["repo_id", "revision_id", "generation", "payload"];

impl Serialize for UpsertCommit {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("UpsertCommit", 4)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("payload", &serde_bytes_helper::Bytes::new(&self.payload))?;
        state.end()
    }
}

struct UpsertCommitVisitor;

impl<'de> Visitor<'de> for UpsertCommitVisitor {
    type Value = UpsertCommit;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("an UpsertCommit map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut payload: Option<Vec<u8>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(serde_de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "payload" => {
                    if payload.is_some() {
                        return Err(serde_de::Error::duplicate_field("payload"));
                    }
                    let bytes: serde_bytes_helper::ByteBuf = map.next_value()?;
                    payload = Some(bytes.into_vec());
                }
                other => {
                    return Err(serde_de::Error::unknown_field(other, UPSERT_COMMIT_FIELDS));
                }
            }
        }
        Ok(UpsertCommit {
            repo_id: repo_id.ok_or_else(|| serde_de::Error::missing_field("repo_id"))?,
            revision_id: revision_id
                .ok_or_else(|| serde_de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| serde_de::Error::missing_field("generation"))?,
            payload: payload.ok_or_else(|| serde_de::Error::missing_field("payload"))?,
        })
    }
}

impl<'de> Deserialize<'de> for UpsertCommit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("UpsertCommit", UPSERT_COMMIT_FIELDS, UpsertCommitVisitor)
    }
}

const UPSERT_REF_FIELDS: &[&str] = &["repo_id", "revision_id", "generation", "name", "sha"];

impl Serialize for UpsertRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("UpsertRef", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("name", self.name.as_ref())?;
        state.serialize_field("sha", &serde_bytes_helper::Bytes::new(&self.sha))?;
        state.end()
    }
}

struct UpsertRefVisitor;

impl<'de> Visitor<'de> for UpsertRefVisitor {
    type Value = UpsertRef;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("an UpsertRef map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut name: Option<Box<str>> = None;
        let mut sha: Option<[u8; 20]> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(serde_de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "name" => {
                    if name.is_some() {
                        return Err(serde_de::Error::duplicate_field("name"));
                    }
                    let raw: String = map.next_value()?;
                    name = Some(box_str_from_string(raw));
                }
                "sha" => {
                    if sha.is_some() {
                        return Err(serde_de::Error::duplicate_field("sha"));
                    }
                    let bytes: serde_bytes_helper::ByteBuf = map.next_value()?;
                    sha = Some(sha20_from_bytes::<A::Error>(bytes.as_slice())?);
                }
                other => {
                    return Err(serde_de::Error::unknown_field(other, UPSERT_REF_FIELDS));
                }
            }
        }
        Ok(UpsertRef {
            repo_id: repo_id.ok_or_else(|| serde_de::Error::missing_field("repo_id"))?,
            revision_id: revision_id
                .ok_or_else(|| serde_de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| serde_de::Error::missing_field("generation"))?,
            name: name.ok_or_else(|| serde_de::Error::missing_field("name"))?,
            sha: sha.ok_or_else(|| serde_de::Error::missing_field("sha"))?,
        })
    }
}

impl<'de> Deserialize<'de> for UpsertRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("UpsertRef", UPSERT_REF_FIELDS, UpsertRefVisitor)
    }
}

const UPSERT_TAG_FIELDS: &[&str] = &["repo_id", "revision_id", "generation", "name", "sha"];

impl Serialize for UpsertTag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("UpsertTag", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("name", self.name.as_ref())?;
        state.serialize_field("sha", &serde_bytes_helper::Bytes::new(&self.sha))?;
        state.end()
    }
}

struct UpsertTagVisitor;

impl<'de> Visitor<'de> for UpsertTagVisitor {
    type Value = UpsertTag;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("an UpsertTag map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut name: Option<Box<str>> = None;
        let mut sha: Option<[u8; 20]> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(serde_de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "name" => {
                    if name.is_some() {
                        return Err(serde_de::Error::duplicate_field("name"));
                    }
                    let raw: String = map.next_value()?;
                    name = Some(box_str_from_string(raw));
                }
                "sha" => {
                    if sha.is_some() {
                        return Err(serde_de::Error::duplicate_field("sha"));
                    }
                    let bytes: serde_bytes_helper::ByteBuf = map.next_value()?;
                    sha = Some(sha20_from_bytes::<A::Error>(bytes.as_slice())?);
                }
                other => {
                    return Err(serde_de::Error::unknown_field(other, UPSERT_TAG_FIELDS));
                }
            }
        }
        Ok(UpsertTag {
            repo_id: repo_id.ok_or_else(|| serde_de::Error::missing_field("repo_id"))?,
            revision_id: revision_id
                .ok_or_else(|| serde_de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| serde_de::Error::missing_field("generation"))?,
            name: name.ok_or_else(|| serde_de::Error::missing_field("name"))?,
            sha: sha.ok_or_else(|| serde_de::Error::missing_field("sha"))?,
        })
    }
}

impl<'de> Deserialize<'de> for UpsertTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("UpsertTag", UPSERT_TAG_FIELDS, UpsertTagVisitor)
    }
}

const DELETE_REF_FIELDS: &[&str] = &["repo_id", "revision_id", "generation", "name"];

impl Serialize for DeleteRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DeleteRef", 4)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("name", self.name.as_ref())?;
        state.end()
    }
}

struct DeleteRefVisitor;

impl<'de> Visitor<'de> for DeleteRefVisitor {
    type Value = DeleteRef;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("a DeleteRef map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut name: Option<Box<str>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(serde_de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "name" => {
                    if name.is_some() {
                        return Err(serde_de::Error::duplicate_field("name"));
                    }
                    let raw: String = map.next_value()?;
                    name = Some(box_str_from_string(raw));
                }
                other => {
                    return Err(serde_de::Error::unknown_field(other, DELETE_REF_FIELDS));
                }
            }
        }
        Ok(DeleteRef {
            repo_id: repo_id.ok_or_else(|| serde_de::Error::missing_field("repo_id"))?,
            revision_id: revision_id
                .ok_or_else(|| serde_de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| serde_de::Error::missing_field("generation"))?,
            name: name.ok_or_else(|| serde_de::Error::missing_field("name"))?,
        })
    }
}

impl<'de> Deserialize<'de> for DeleteRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("DeleteRef", DELETE_REF_FIELDS, DeleteRefVisitor)
    }
}

const DELETE_TAG_FIELDS: &[&str] = &["repo_id", "revision_id", "generation", "name"];

impl Serialize for DeleteTag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DeleteTag", 4)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("name", self.name.as_ref())?;
        state.end()
    }
}

struct DeleteTagVisitor;

impl<'de> Visitor<'de> for DeleteTagVisitor {
    type Value = DeleteTag;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("a DeleteTag map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut name: Option<Box<str>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(serde_de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "name" => {
                    if name.is_some() {
                        return Err(serde_de::Error::duplicate_field("name"));
                    }
                    let raw: String = map.next_value()?;
                    name = Some(box_str_from_string(raw));
                }
                other => {
                    return Err(serde_de::Error::unknown_field(other, DELETE_TAG_FIELDS));
                }
            }
        }
        Ok(DeleteTag {
            repo_id: repo_id.ok_or_else(|| serde_de::Error::missing_field("repo_id"))?,
            revision_id: revision_id
                .ok_or_else(|| serde_de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| serde_de::Error::missing_field("generation"))?,
            name: name.ok_or_else(|| serde_de::Error::missing_field("name"))?,
        })
    }
}

impl<'de> Deserialize<'de> for DeleteTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("DeleteTag", DELETE_TAG_FIELDS, DeleteTagVisitor)
    }
}

const UPSERT_DIRTY_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "doc_id",
    "applied_at_ms",
    "payload_hash",
];

impl Serialize for UpsertDirty {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("UpsertDirty", 6)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("doc_id", &self.doc_id)?;
        state.serialize_field("applied_at_ms", &self.applied_at_ms)?;
        state.serialize_field(
            "payload_hash",
            &serde_bytes_helper::Bytes::new(&self.payload_hash),
        )?;
        state.end()
    }
}

struct UpsertDirtyVisitor;

impl<'de> Visitor<'de> for UpsertDirtyVisitor {
    type Value = UpsertDirty;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("an UpsertDirty map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut doc_id: Option<ChunkId> = None;
        let mut applied_at_ms: Option<u64> = None;
        let mut payload_hash: Option<[u8; 32]> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(serde_de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "doc_id" => {
                    if doc_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("doc_id"));
                    }
                    doc_id = Some(map.next_value()?);
                }
                "applied_at_ms" => {
                    if applied_at_ms.is_some() {
                        return Err(serde_de::Error::duplicate_field("applied_at_ms"));
                    }
                    applied_at_ms = Some(map.next_value()?);
                }
                "payload_hash" => {
                    if payload_hash.is_some() {
                        return Err(serde_de::Error::duplicate_field("payload_hash"));
                    }
                    let bytes: serde_bytes_helper::ByteBuf = map.next_value()?;
                    payload_hash = Some(hash32_from_bytes::<A::Error>(bytes.as_slice())?);
                }
                other => {
                    return Err(serde_de::Error::unknown_field(other, UPSERT_DIRTY_FIELDS));
                }
            }
        }
        Ok(UpsertDirty {
            repo_id: repo_id.ok_or_else(|| serde_de::Error::missing_field("repo_id"))?,
            revision_id: revision_id
                .ok_or_else(|| serde_de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| serde_de::Error::missing_field("generation"))?,
            doc_id: doc_id.ok_or_else(|| serde_de::Error::missing_field("doc_id"))?,
            applied_at_ms: applied_at_ms
                .ok_or_else(|| serde_de::Error::missing_field("applied_at_ms"))?,
            payload_hash: payload_hash
                .ok_or_else(|| serde_de::Error::missing_field("payload_hash"))?,
        })
    }
}

impl<'de> Deserialize<'de> for UpsertDirty {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("UpsertDirty", UPSERT_DIRTY_FIELDS, UpsertDirtyVisitor)
    }
}

const EVICT_DIRTY_FIELDS: &[&str] = &["repo_id", "revision_id", "generation", "doc_id"];

impl Serialize for EvictDirty {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("EvictDirty", 4)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("doc_id", &self.doc_id)?;
        state.end()
    }
}

struct EvictDirtyVisitor;

impl<'de> Visitor<'de> for EvictDirtyVisitor {
    type Value = EvictDirty;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("an EvictDirty map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut doc_id: Option<ChunkId> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(serde_de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "doc_id" => {
                    if doc_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("doc_id"));
                    }
                    doc_id = Some(map.next_value()?);
                }
                other => {
                    return Err(serde_de::Error::unknown_field(other, EVICT_DIRTY_FIELDS));
                }
            }
        }
        Ok(EvictDirty {
            repo_id: repo_id.ok_or_else(|| serde_de::Error::missing_field("repo_id"))?,
            revision_id: revision_id
                .ok_or_else(|| serde_de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| serde_de::Error::missing_field("generation"))?,
            doc_id: doc_id.ok_or_else(|| serde_de::Error::missing_field("doc_id"))?,
        })
    }
}

impl<'de> Deserialize<'de> for EvictDirty {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("EvictDirty", EVICT_DIRTY_FIELDS, EvictDirtyVisitor)
    }
}

const UPSERT_PARSE_TREE_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "chunk_id",
    "payload",
];

impl Serialize for UpsertParseTree {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("UpsertParseTree", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("chunk_id", &self.chunk_id)?;
        state.serialize_field("payload", &serde_bytes_helper::Bytes::new(&self.payload))?;
        state.end()
    }
}

struct UpsertParseTreeVisitor;

impl<'de> Visitor<'de> for UpsertParseTreeVisitor {
    type Value = UpsertParseTree;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("an UpsertParseTree map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut chunk_id: Option<ChunkId> = None;
        let mut payload: Option<Vec<u8>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(serde_de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "chunk_id" => {
                    if chunk_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("chunk_id"));
                    }
                    chunk_id = Some(map.next_value()?);
                }
                "payload" => {
                    if payload.is_some() {
                        return Err(serde_de::Error::duplicate_field("payload"));
                    }
                    let bytes: serde_bytes_helper::ByteBuf = map.next_value()?;
                    payload = Some(bytes.into_vec());
                }
                other => {
                    return Err(serde_de::Error::unknown_field(
                        other,
                        UPSERT_PARSE_TREE_FIELDS,
                    ));
                }
            }
        }
        Ok(UpsertParseTree {
            repo_id: repo_id.ok_or_else(|| serde_de::Error::missing_field("repo_id"))?,
            revision_id: revision_id
                .ok_or_else(|| serde_de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| serde_de::Error::missing_field("generation"))?,
            chunk_id: chunk_id.ok_or_else(|| serde_de::Error::missing_field("chunk_id"))?,
            payload: payload.ok_or_else(|| serde_de::Error::missing_field("payload"))?,
        })
    }
}

impl<'de> Deserialize<'de> for UpsertParseTree {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "UpsertParseTree",
            UPSERT_PARSE_TREE_FIELDS,
            UpsertParseTreeVisitor,
        )
    }
}

const DELETE_PARSE_TREE_FIELDS: &[&str] = &["repo_id", "revision_id", "generation", "chunk_id"];

impl Serialize for DeleteParseTree {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DeleteParseTree", 4)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("chunk_id", &self.chunk_id)?;
        state.end()
    }
}

struct DeleteParseTreeVisitor;

impl<'de> Visitor<'de> for DeleteParseTreeVisitor {
    type Value = DeleteParseTree;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("a DeleteParseTree map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut chunk_id: Option<ChunkId> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(serde_de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "chunk_id" => {
                    if chunk_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("chunk_id"));
                    }
                    chunk_id = Some(map.next_value()?);
                }
                other => {
                    return Err(serde_de::Error::unknown_field(
                        other,
                        DELETE_PARSE_TREE_FIELDS,
                    ));
                }
            }
        }
        Ok(DeleteParseTree {
            repo_id: repo_id.ok_or_else(|| serde_de::Error::missing_field("repo_id"))?,
            revision_id: revision_id
                .ok_or_else(|| serde_de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| serde_de::Error::missing_field("generation"))?,
            chunk_id: chunk_id.ok_or_else(|| serde_de::Error::missing_field("chunk_id"))?,
        })
    }
}

impl<'de> Deserialize<'de> for DeleteParseTree {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "DeleteParseTree",
            DELETE_PARSE_TREE_FIELDS,
            DeleteParseTreeVisitor,
        )
    }
}

const UPSERT_DIFF_HUNK_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "commit_sha",
    "file_path",
    "payload",
];

impl Serialize for UpsertDiffHunk {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("UpsertDiffHunk", 6)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field(
            "commit_sha",
            &serde_bytes_helper::Bytes::new(&self.commit_sha),
        )?;
        state.serialize_field("file_path", self.file_path.as_ref())?;
        state.serialize_field("payload", &serde_bytes_helper::Bytes::new(&self.payload))?;
        state.end()
    }
}

struct UpsertDiffHunkVisitor;

impl<'de> Visitor<'de> for UpsertDiffHunkVisitor {
    type Value = UpsertDiffHunk;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("an UpsertDiffHunk map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut commit_sha: Option<[u8; 20]> = None;
        let mut file_path: Option<Box<str>> = None;
        let mut payload: Option<Vec<u8>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(serde_de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "generation" => {
                    if generation.is_some() {
                        return Err(serde_de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "commit_sha" => {
                    if commit_sha.is_some() {
                        return Err(serde_de::Error::duplicate_field("commit_sha"));
                    }
                    let bytes: serde_bytes_helper::ByteBuf = map.next_value()?;
                    commit_sha = Some(sha20_from_bytes::<A::Error>(bytes.as_slice())?);
                }
                "file_path" => {
                    if file_path.is_some() {
                        return Err(serde_de::Error::duplicate_field("file_path"));
                    }
                    let raw: String = map.next_value()?;
                    file_path = Some(box_str_from_string(raw));
                }
                "payload" => {
                    if payload.is_some() {
                        return Err(serde_de::Error::duplicate_field("payload"));
                    }
                    let bytes: serde_bytes_helper::ByteBuf = map.next_value()?;
                    payload = Some(bytes.into_vec());
                }
                other => {
                    return Err(serde_de::Error::unknown_field(
                        other,
                        UPSERT_DIFF_HUNK_FIELDS,
                    ));
                }
            }
        }
        Ok(UpsertDiffHunk {
            repo_id: repo_id.ok_or_else(|| serde_de::Error::missing_field("repo_id"))?,
            revision_id: revision_id
                .ok_or_else(|| serde_de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| serde_de::Error::missing_field("generation"))?,
            commit_sha: commit_sha.ok_or_else(|| serde_de::Error::missing_field("commit_sha"))?,
            file_path: file_path.ok_or_else(|| serde_de::Error::missing_field("file_path"))?,
            payload: payload.ok_or_else(|| serde_de::Error::missing_field("payload"))?,
        })
    }
}

impl<'de> Deserialize<'de> for UpsertDiffHunk {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "UpsertDiffHunk",
            UPSERT_DIFF_HUNK_FIELDS,
            UpsertDiffHunkVisitor,
        )
    }
}

// Local helper module: emits/consumes a CBOR `bytes` payload for fixed-size
// byte fields and opaque blobs. Avoids pulling in the `serde_bytes` crate
// (workspace dep policy is to minimise transitive deps) while keeping the
// wire shape as CBOR major type 2 (byte string) rather than CBOR major
// type 4 (array of small integers).
mod serde_bytes_helper {
    use core::fmt;
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Visitor};

    pub(super) struct Bytes<'a>(&'a [u8]);

    impl<'a> Bytes<'a> {
        pub(super) const fn new(value: &'a [u8]) -> Self {
            Self(value)
        }
    }

    impl Serialize for Bytes<'_> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_bytes(self.0)
        }
    }

    pub(super) struct ByteBuf(Vec<u8>);

    impl ByteBuf {
        pub(super) fn into_vec(self) -> Vec<u8> {
            self.0
        }

        pub(super) fn as_slice(&self) -> &[u8] {
            self.0.as_slice()
        }
    }

    struct ByteBufVisitor;

    impl<'de> Visitor<'de> for ByteBufVisitor {
        type Value = ByteBuf;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a byte buffer")
        }

        fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(ByteBuf(v.to_vec()))
        }

        fn visit_borrowed_bytes<E>(self, v: &'de [u8]) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(ByteBuf(v.to_vec()))
        }

        fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(ByteBuf(v))
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            // Some decoders surface `bytes` as a numeric array. Accept that
            // shape too so we are robust across encoders.
            let mut out: Vec<u8> = seq.size_hint().map_or_else(Vec::new, Vec::with_capacity);
            while let Some(elem) = seq.next_element::<u16>()? {
                let byte = u8::try_from(elem).map_err(|_err| {
                    <A::Error as serde::de::Error>::invalid_value(
                        serde::de::Unexpected::Unsigned(u64::from(elem)),
                        &"a byte value in [0, 255]",
                    )
                })?;
                out.push(byte);
            }
            Ok(ByteBuf(out))
        }
    }

    impl<'de> Deserialize<'de> for ByteBuf {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_byte_buf(ByteBufVisitor)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeleteParseTree, DeleteRef, DeleteTag, EvictDirty, UpsertCommit, UpsertDiffHunk,
        UpsertDirty, UpsertParseTree, UpsertRef, UpsertTag,
    };
    use crate::{ChunkId, ManifestGeneration, RepoId, RevisionId};

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    fn encode<T: serde::Serialize>(v: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut buf: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(v, &mut buf)?;
        Ok(buf)
    }

    fn decode<T>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        Ok(ciborium::de::from_reader(bytes)?)
    }

    fn add_unknown_field(value: &mut ciborium::Value) -> Result<(), Box<dyn std::error::Error>> {
        let ciborium::Value::Map(fields) = value else {
            return Err("expected CBOR map".into());
        };
        fields.push((
            ciborium::Value::Text("__never_field".to_owned()),
            ciborium::Value::Bool(true),
        ));
        Ok(())
    }

    fn assert_unknown_field_rejected<T>(value: &T) -> TestRes
    where
        T: serde::Serialize + for<'de> serde::Deserialize<'de>,
    {
        let wire_bytes = encode(value)?;
        let mut wire: ciborium::Value = decode(&wire_bytes)?;
        add_unknown_field(&mut wire)?;
        let mutated = encode(&wire)?;
        let decoded: Result<T, _> = ciborium::de::from_reader::<T, _>(mutated.as_slice());
        match decoded {
            Ok(_v) => Err("unknown field accepted; expected rejection".into()),
            Err(_e) => Ok(()),
        }
    }

    fn sample_repo() -> RepoId {
        RepoId::new("repo-alpha")
    }
    fn sample_rev() -> RevisionId {
        RevisionId::new("rev-beta")
    }
    const SAMPLE_GEN: ManifestGeneration = ManifestGeneration::new(7);

    #[test]
    fn upsert_commit_cbor_roundtrip() -> TestRes {
        let v = UpsertCommit {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            payload: vec![1, 2, 3, 4],
        };
        let bytes = encode(&v)?;
        let back: UpsertCommit = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn upsert_commit_unknown_field_rejected() -> TestRes {
        assert_unknown_field_rejected(&UpsertCommit {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            payload: vec![0xDE, 0xAD],
        })
    }

    #[test]
    fn upsert_ref_cbor_roundtrip() -> TestRes {
        let v = UpsertRef {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            name: "refs/heads/main".into(),
            sha: [0xAB_u8; 20],
        };
        let bytes = encode(&v)?;
        let back: UpsertRef = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn upsert_ref_unknown_field_rejected() -> TestRes {
        assert_unknown_field_rejected(&UpsertRef {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            name: "refs/heads/main".into(),
            sha: [0xAB_u8; 20],
        })
    }

    #[test]
    fn upsert_tag_cbor_roundtrip() -> TestRes {
        let v = UpsertTag {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            name: "refs/tags/v1.0".into(),
            sha: [0x42_u8; 20],
        };
        let bytes = encode(&v)?;
        let back: UpsertTag = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn upsert_tag_unknown_field_rejected() -> TestRes {
        assert_unknown_field_rejected(&UpsertTag {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            name: "refs/tags/v1.0".into(),
            sha: [0x42_u8; 20],
        })
    }

    #[test]
    fn delete_ref_cbor_roundtrip() -> TestRes {
        let v = DeleteRef {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            name: "refs/heads/feature".into(),
        };
        let bytes = encode(&v)?;
        let back: DeleteRef = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn delete_ref_unknown_field_rejected() -> TestRes {
        assert_unknown_field_rejected(&DeleteRef {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            name: "refs/heads/feature".into(),
        })
    }

    #[test]
    fn delete_tag_cbor_roundtrip() -> TestRes {
        let v = DeleteTag {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            name: "refs/tags/old".into(),
        };
        let bytes = encode(&v)?;
        let back: DeleteTag = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn delete_tag_unknown_field_rejected() -> TestRes {
        assert_unknown_field_rejected(&DeleteTag {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            name: "refs/tags/old".into(),
        })
    }

    #[test]
    fn upsert_dirty_cbor_roundtrip() -> TestRes {
        let v = UpsertDirty {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            doc_id: ChunkId::new("chunk-1"),
            applied_at_ms: 1_700_000_000_000,
            payload_hash: [0xCD_u8; 32],
        };
        let bytes = encode(&v)?;
        let back: UpsertDirty = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn upsert_dirty_unknown_field_rejected() -> TestRes {
        assert_unknown_field_rejected(&UpsertDirty {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            doc_id: ChunkId::new("chunk-1"),
            applied_at_ms: 1_700_000_000_000,
            payload_hash: [0xCD_u8; 32],
        })
    }

    #[test]
    fn evict_dirty_cbor_roundtrip() -> TestRes {
        let v = EvictDirty {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            doc_id: ChunkId::new("chunk-2"),
        };
        let bytes = encode(&v)?;
        let back: EvictDirty = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn evict_dirty_unknown_field_rejected() -> TestRes {
        assert_unknown_field_rejected(&EvictDirty {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            doc_id: ChunkId::new("chunk-2"),
        })
    }

    #[test]
    fn upsert_parse_tree_cbor_roundtrip() -> TestRes {
        let v = UpsertParseTree {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            chunk_id: ChunkId::new("chunk-pt"),
            payload: vec![10, 20, 30],
        };
        let bytes = encode(&v)?;
        let back: UpsertParseTree = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn upsert_parse_tree_unknown_field_rejected() -> TestRes {
        assert_unknown_field_rejected(&UpsertParseTree {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            chunk_id: ChunkId::new("chunk-pt"),
            payload: vec![10, 20, 30],
        })
    }

    #[test]
    fn delete_parse_tree_cbor_roundtrip() -> TestRes {
        let v = DeleteParseTree {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            chunk_id: ChunkId::new("chunk-pt"),
        };
        let bytes = encode(&v)?;
        let back: DeleteParseTree = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn delete_parse_tree_unknown_field_rejected() -> TestRes {
        assert_unknown_field_rejected(&DeleteParseTree {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            chunk_id: ChunkId::new("chunk-pt"),
        })
    }

    #[test]
    fn upsert_diff_hunk_cbor_roundtrip() -> TestRes {
        let v = UpsertDiffHunk {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            commit_sha: [0x11_u8; 20],
            file_path: "src/foo.rs".into(),
            payload: vec![1, 1, 2, 3, 5, 8],
        };
        let bytes = encode(&v)?;
        let back: UpsertDiffHunk = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn upsert_diff_hunk_unknown_field_rejected() -> TestRes {
        assert_unknown_field_rejected(&UpsertDiffHunk {
            repo_id: sample_repo(),
            revision_id: sample_rev(),
            generation: SAMPLE_GEN,
            commit_sha: [0x11_u8; 20],
            file_path: "src/foo.rs".into(),
            payload: vec![1, 1, 2, 3, 5, 8],
        })
    }
}
