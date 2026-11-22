//! Private, provisional export of one physically sealed Legacy Source census.
//! Frames do not authorize a Native publication. The receiver must compare the
//! complete inventory, each original file and the applied Catalog ACK against
//! the held Legacy source before it stages a replacement.

use super::{CatalogView, Digest, ImmutableOperation, OperationKey, UpdateRequest, ViewRef};
use crate::{
    BatchPublishReceipt, ManifestGeneration, RepoId, RevisionId, SearchCorpusActiveHeadV1,
    SourceFileRevision, SourcePublicationEvent,
    native_cutover_evidence_v1::source_binding::{
        NativeCutoverEvidenceSourcePublicationBindingV1, NativeCutoverEvidenceTargetV1,
    },
    repomap::RepoMapExpectedActiveV2,
};
use quanta_index_contract_base::NativeCutoverDigest64V1;
use sha2::{Digest as _, Sha256};
use std::io::{self, Write};

/// Legacy restore records accept unknown named fields for forward compatibility.
/// Known names may arrive as CBOR text or byte strings.
struct RestoreFieldKeyV1(&'static [&'static str]);

impl<'de> serde::de::DeserializeSeed<'de> for RestoreFieldKeyV1 {
    type Value = Option<&'static str>;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        struct KeyVisitor(&'static [&'static str]);

        impl serde::de::Visitor<'_> for KeyVisitor {
            type Value = Option<&'static str>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a restore record field name")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(self.0.iter().copied().find(|field| *field == value))
            }

            fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                Ok(self
                    .0
                    .iter()
                    .copied()
                    .find(|field| field.as_bytes() == value))
            }
        }

        deserializer.deserialize_identifier(KeyVisitor(self.0))
    }
}

macro_rules! restore_open_record {
    ($( [$mode:ident] )? $name:ident { $($field:ident : $ty:ty $(= $map_default:ident)?),+ $(,)? }) => {
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                let mut state = serializer.serialize_struct(stringify!($name), FIELDS.len())?;
                $(serde::ser::SerializeStruct::serialize_field(&mut state, stringify!($field), &self.$field)?;)+
                serde::ser::SerializeStruct::end(state)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                struct RecordVisitor;
                impl<'de> serde::de::Visitor<'de> for RecordVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(concat!("a ", stringify!($name), " record"))
                    }

                    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                        $(let mut $field: Option<$ty> = None;)+
                        while let Some(key) = map.next_key_seed(RestoreFieldKeyV1(FIELDS))? {
                            match key {
                                $(Some(stringify!($field)) => {
                                    if $field.is_some() {
                                        return Err(serde::de::Error::duplicate_field(stringify!($field)));
                                    }
                                    $field = Some(map.next_value::<$ty>()?);
                                },)+
                                None => { let _: serde::de::IgnoredAny = map.next_value()?; }
                                _ => unreachable!("restore field seed only returns schema names"),
                            }
                        }
                        Ok($name { $($field: restore_open_record!(@map_value $field $(= $map_default)?),)+ })
                    }

                    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                        $(let $field = seq.next_element::<$ty>()?
                            .ok_or_else(|| serde::de::Error::missing_field(stringify!($field)))?;)+
                        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                            return Err(serde::de::Error::invalid_length(FIELDS.len().saturating_add(1), &self));
                        }
                        Ok($name { $($field,)+ })
                    }
                }
                restore_open_record!(@read deserializer, $name, FIELDS, RecordVisitor $(, $mode)?)
            }
        }
    };
    (@map_value $field:ident) => {
        $field.ok_or_else(|| serde::de::Error::missing_field(stringify!($field)))?
    };
    (@map_value $field:ident = default) => {
        $field.unwrap_or_default()
    };
    (@read $decoder:ident, $name:ident, $fields:ident, $visitor:ident) => {
        $decoder.deserialize_struct(stringify!($name), $fields, $visitor)
    };
    (@read $decoder:ident, $name:ident, $fields:ident, $visitor:ident, map_only) => {
        $decoder.deserialize_map($visitor)
    };
}

const SOURCE_MANIFEST_DOMAIN_V1: &[u8] = b"qidx.imported-legacy-source-files.v1\0";
const IMPORT_MANIFEST_DOMAIN_V1: &[u8] = b"qidx.imported-legacy-source-manifest.v1\0";
const COMPLETED_DOMAIN_V1: &[u8] = b"historical-import/completed\0";

struct DigestWriterV1<'a>(&'a mut Sha256);

impl Write for DigestWriterV1<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn typed_digest_v1(value: &impl serde::Serialize) -> Option<Digest> {
    let mut digest = Sha256::new();
    ciborium::ser::into_writer(value, DigestWriterV1(&mut digest)).ok()?;
    Some(Digest(digest.finalize().into()))
}

fn frame_bytes_v1(digest: &mut Sha256, bytes: &[u8]) -> Option<()> {
    digest.update(u64::try_from(bytes.len()).ok()?.to_le_bytes());
    digest.update(bytes);
    Some(())
}

/// Hash complete historical source bodies without retaining a second corpus.
/// The caller must independently prove the exporter inventory and old root.
pub struct LegacySourceManifestDigestV1 {
    digest: Sha256,
    files: u64,
}

impl LegacySourceManifestDigestV1 {
    #[must_use]
    pub fn new_v1() -> Self {
        let mut digest = Sha256::new();
        digest.update(SOURCE_MANIFEST_DOMAIN_V1);
        Self { digest, files: 0 }
    }

    pub fn append_file_v1(
        &mut self,
        source: &SourceFileRevision,
        text_admitted: bool,
        bytes: &[u8],
    ) -> Option<()> {
        source.validate().ok()?;
        if text_admitted && std::str::from_utf8(bytes).is_err() {
            return None;
        }
        if <[u8; 32]>::from(Sha256::digest(bytes)) != source.source_sha256 {
            return None;
        }
        let next = self.files.checked_add(1)?;
        frame_bytes_v1(
            &mut self.digest,
            source.file.source_repo_id.as_str().as_bytes(),
        )?;
        frame_bytes_v1(
            &mut self.digest,
            source.file.repo_relative_path.as_str().as_bytes(),
        )?;
        frame_bytes_v1(&mut self.digest, source.revision_id.as_str().as_bytes())?;
        self.digest.update(source.source_sha256);
        self.digest.update([u8::from(text_admitted)]);
        frame_bytes_v1(&mut self.digest, bytes)?;
        self.files = next;
        Some(())
    }

    #[must_use]
    pub fn finish_v1(mut self) -> (Digest, u64) {
        self.digest.update(self.files.to_le_bytes());
        (Digest(self.digest.finalize().into()), self.files)
    }
}

impl Default for LegacySourceManifestDigestV1 {
    fn default() -> Self {
        Self::new_v1()
    }
}

/// Bind the completed exporter, exact original file manifest and immutable Q
/// request. The digest is DATA; only the retained producer journal issues it.
pub fn imported_legacy_source_manifest_digest_v1(
    origin: &LegacySourceOriginV1,
    accepted_transcript_sha256: Digest,
    source_manifest_sha256: Digest,
    original_snapshot_id: u64,
    candidate_root_nonce: Digest,
    target: &NativeCutoverEvidenceTargetV1,
    request: &UpdateRequest,
) -> Option<Digest> {
    if !origin.is_canonical_v1()
        || original_snapshot_id == 0
        || accepted_transcript_sha256.0 == [0; 32]
        || source_manifest_sha256.0 == [0; 32]
        || candidate_root_nonce.0 == [0; 32]
        || target.operation != request.operation
    {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(IMPORT_MANIFEST_DOMAIN_V1);
    digest.update(typed_digest_v1(origin)?.0);
    digest.update(accepted_transcript_sha256.0);
    digest.update(source_manifest_sha256.0);
    digest.update(original_snapshot_id.to_le_bytes());
    digest.update(candidate_root_nonce.0);
    digest.update(typed_digest_v1(target)?.0);
    digest.update(typed_digest_v1(request)?.0);
    Some(Digest(digest.finalize().into()))
}

/// A distinct completed historical-import purpose; it cannot be mistaken for
/// the ordinary current RepoState V5 completion pointer.
pub fn imported_legacy_source_completed_digest_v1(
    manifest_digest: Digest,
    operation: &ImmutableOperation,
    publication: &CatalogView,
) -> Option<Digest> {
    if manifest_digest.0 == [0; 32]
        || !imported_source_publication_scope_v1(operation.key, publication.identity)
        || publication.identity_digest().ok() != Some(publication.identity.digest)
    {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(COMPLETED_DOMAIN_V1);
    digest.update(manifest_digest.0);
    digest.update(typed_digest_v1(operation)?.0);
    digest.update(typed_digest_v1(publication)?.0);
    Some(Digest(digest.finalize().into()))
}

fn digest64_lower_v1(digest: Digest) -> NativeCutoverDigest64V1 {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 64];
    for (index, byte) in digest.0.iter().enumerate() {
        bytes[2 * index] = HEX[usize::from(byte >> 4)];
        bytes[2 * index + 1] = HEX[usize::from(byte & 15)];
    }
    NativeCutoverDigest64V1(bytes)
}

/// Exact source-head map root decoded from the committed lexical file authority.
/// The committed file-authority artifact and generation seal authenticate this
/// descriptor; its hash is never derived from a streamed file list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LexicalSourceHeadsCommitmentV1 {
    pub root_sha256: Option<[u8; 32]>,
    pub entries: u64,
}

restore_open_record!(LexicalSourceHeadsCommitmentV1 {
    root_sha256: Option<[u8; 32]> = default,
    entries: u64,
});

impl LexicalSourceHeadsCommitmentV1 {
    #[must_use]
    pub fn is_canonical_v1(&self) -> bool {
        self.root_sha256.is_some() == (self.entries != 0)
    }
}

/// Bounded private child stdout stream. A HeadBegin and its file frames are
/// provisional until HeadComplete; all heads are provisional until InventoryComplete.
/// A file body is emitted in <=64 KiB chunks without retaining the whole corpus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceSnapshotExportFrameV1 {
    HeadBegin {
        repo_id: RepoId,
        revision_id: RevisionId,
        active: SearchCorpusActiveHeadV1,
        applied_ack: BatchPublishReceipt,
        source_publication_event: SourcePublicationEvent,
        source_heads: LexicalSourceHeadsCommitmentV1,
        repomap_active: Option<RepoMapExpectedActiveV2>,
    },
    FileBegin {
        source: SourceFileRevision,
        text_admitted: bool,
        source_bytes: u64,
    },
    FileChunk {
        offset: u64,
        bytes: Vec<u8>,
    },
    FileEnd {
        source: SourceFileRevision,
    },
    HeadComplete {
        repo_id: RepoId,
        revision_id: RevisionId,
        file_count: u64,
        source_heads: LexicalSourceHeadsCommitmentV1,
    },
    RepoMapBegin {
        repo_id: RepoId,
        revision_id: RevisionId,
        generation: ManifestGeneration,
        old_active: RepoMapExpectedActiveV2,
        payload_bytes: u64,
        payload_sha256: Digest,
    },
    RepoMapChunk {
        offset: u64,
        bytes: Vec<u8>,
    },
    RepoMapComplete {
        repo_id: RepoId,
        revision_id: RevisionId,
    },
    InventoryComplete {
        head_count: u64,
        repomap_head_count: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceFrameKindV1 {
    HeadBegin,
    FileBegin,
    FileChunk,
    FileEnd,
    HeadComplete,
    RepoMapBegin,
    RepoMapChunk,
    RepoMapComplete,
    InventoryComplete,
}

impl SourceFrameKindV1 {
    const NAMES: &'static [&'static str] = &[
        "head_begin",
        "file_begin",
        "file_chunk",
        "file_end",
        "head_complete",
        "repo_map_begin",
        "repo_map_chunk",
        "repo_map_complete",
        "inventory_complete",
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::HeadBegin => "head_begin",
            Self::FileBegin => "file_begin",
            Self::FileChunk => "file_chunk",
            Self::FileEnd => "file_end",
            Self::HeadComplete => "head_complete",
            Self::RepoMapBegin => "repo_map_begin",
            Self::RepoMapChunk => "repo_map_chunk",
            Self::RepoMapComplete => "repo_map_complete",
            Self::InventoryComplete => "inventory_complete",
        }
    }

    fn parse<E: serde::de::Error>(name: &str) -> Result<Self, E> {
        match name {
            "head_begin" => Ok(Self::HeadBegin),
            "file_begin" => Ok(Self::FileBegin),
            "file_chunk" => Ok(Self::FileChunk),
            "file_end" => Ok(Self::FileEnd),
            "head_complete" => Ok(Self::HeadComplete),
            "repo_map_begin" => Ok(Self::RepoMapBegin),
            "repo_map_chunk" => Ok(Self::RepoMapChunk),
            "repo_map_complete" => Ok(Self::RepoMapComplete),
            "inventory_complete" => Ok(Self::InventoryComplete),
            _ => Err(E::unknown_variant(name, Self::NAMES)),
        }
    }
}

impl<'de> serde::Deserialize<'de> for SourceFrameKindV1 {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct KindVisitor;
        impl serde::de::Visitor<'_> for KindVisitor {
            type Value = SourceFrameKindV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a Source export frame kind")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                SourceFrameKindV1::parse(value)
            }

            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                SourceFrameKindV1::parse(&value)
            }
        }
        decoder.deserialize_identifier(KindVisitor)
    }
}

macro_rules! source_frame_body_encode {
    ($serializer:ident, $name:literal, { $($field:ident),+ $(,)? }) => {{
        let mut body = $serializer.serialize_struct($name, [$(stringify!($field)),+].len())?;
        $(serde::ser::SerializeStruct::serialize_field(&mut body, stringify!($field), $field)?;)+
        serde::ser::SerializeStruct::end(body)
    }};
}

struct SourceFrameBodyRefV1<'a>(&'a SourceSnapshotExportFrameV1);

impl serde::Serialize for SourceFrameBodyRefV1<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            SourceSnapshotExportFrameV1::HeadBegin {
                repo_id,
                revision_id,
                active,
                applied_ack,
                source_publication_event,
                source_heads,
                repomap_active,
            } => {
                source_frame_body_encode!(serializer, "HeadBegin", { repo_id, revision_id, active, applied_ack, source_publication_event, source_heads, repomap_active })
            }
            SourceSnapshotExportFrameV1::FileBegin {
                source,
                text_admitted,
                source_bytes,
            } => {
                source_frame_body_encode!(serializer, "FileBegin", { source, text_admitted, source_bytes })
            }
            SourceSnapshotExportFrameV1::FileChunk { offset, bytes } => {
                source_frame_body_encode!(serializer, "FileChunk", { offset, bytes })
            }
            SourceSnapshotExportFrameV1::FileEnd { source } => {
                source_frame_body_encode!(serializer, "FileEnd", { source })
            }
            SourceSnapshotExportFrameV1::HeadComplete {
                repo_id,
                revision_id,
                file_count,
                source_heads,
            } => {
                source_frame_body_encode!(serializer, "HeadComplete", { repo_id, revision_id, file_count, source_heads })
            }
            SourceSnapshotExportFrameV1::RepoMapBegin {
                repo_id,
                revision_id,
                generation,
                old_active,
                payload_bytes,
                payload_sha256,
            } => {
                source_frame_body_encode!(serializer, "RepoMapBegin", { repo_id, revision_id, generation, old_active, payload_bytes, payload_sha256 })
            }
            SourceSnapshotExportFrameV1::RepoMapChunk { offset, bytes } => {
                source_frame_body_encode!(serializer, "RepoMapChunk", { offset, bytes })
            }
            SourceSnapshotExportFrameV1::RepoMapComplete {
                repo_id,
                revision_id,
            } => source_frame_body_encode!(serializer, "RepoMapComplete", { repo_id, revision_id }),
            SourceSnapshotExportFrameV1::InventoryComplete {
                head_count,
                repomap_head_count,
            } => {
                source_frame_body_encode!(serializer, "InventoryComplete", { head_count, repomap_head_count })
            }
        }
    }
}

impl serde::Serialize for SourceSnapshotExportFrameV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let kind = match self {
            Self::HeadBegin { .. } => SourceFrameKindV1::HeadBegin,
            Self::FileBegin { .. } => SourceFrameKindV1::FileBegin,
            Self::FileChunk { .. } => SourceFrameKindV1::FileChunk,
            Self::FileEnd { .. } => SourceFrameKindV1::FileEnd,
            Self::HeadComplete { .. } => SourceFrameKindV1::HeadComplete,
            Self::RepoMapBegin { .. } => SourceFrameKindV1::RepoMapBegin,
            Self::RepoMapChunk { .. } => SourceFrameKindV1::RepoMapChunk,
            Self::RepoMapComplete { .. } => SourceFrameKindV1::RepoMapComplete,
            Self::InventoryComplete { .. } => SourceFrameKindV1::InventoryComplete,
        };
        let mut frame = serializer.serialize_struct("SourceSnapshotExportFrameV1", 2)?;
        serde::ser::SerializeStruct::serialize_field(&mut frame, "kind", kind.name())?;
        serde::ser::SerializeStruct::serialize_field(
            &mut frame,
            "body",
            &SourceFrameBodyRefV1(self),
        )?;
        serde::ser::SerializeStruct::end(frame)
    }
}

// The outer frame can be a map or a positional pair, but its struct-variant
// body is a map. Replay the already bounded input slice so we can decode a
// body before its kind without retaining an untyped content tree.
struct SourceHeadBeginBodyV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
    active: SearchCorpusActiveHeadV1,
    applied_ack: BatchPublishReceipt,
    source_publication_event: SourcePublicationEvent,
    source_heads: LexicalSourceHeadsCommitmentV1,
    repomap_active: Option<RepoMapExpectedActiveV2>,
}
restore_open_record!([map_only] SourceHeadBeginBodyV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
    active: SearchCorpusActiveHeadV1,
    applied_ack: BatchPublishReceipt,
    source_publication_event: SourcePublicationEvent,
    source_heads: LexicalSourceHeadsCommitmentV1,
    repomap_active: Option<RepoMapExpectedActiveV2> = default,
});

struct SourceFileBeginBodyV1 {
    source: SourceFileRevision,
    text_admitted: bool,
    source_bytes: u64,
}
restore_open_record!([map_only] SourceFileBeginBodyV1 {
    source: SourceFileRevision,
    text_admitted: bool,
    source_bytes: u64,
});

struct SourceChunkBodyV1 {
    offset: u64,
    bytes: Vec<u8>,
}
restore_open_record!([map_only] SourceChunkBodyV1 { offset: u64, bytes: Vec<u8> });

struct SourceFileEndBodyV1 {
    source: SourceFileRevision,
}
restore_open_record!([map_only] SourceFileEndBodyV1 {
    source: SourceFileRevision
});

struct SourceHeadCompleteBodyV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
    file_count: u64,
    source_heads: LexicalSourceHeadsCommitmentV1,
}
restore_open_record!([map_only] SourceHeadCompleteBodyV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
    file_count: u64,
    source_heads: LexicalSourceHeadsCommitmentV1,
});

struct SourceRepoMapBeginBodyV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    old_active: RepoMapExpectedActiveV2,
    payload_bytes: u64,
    payload_sha256: Digest,
}
restore_open_record!([map_only] SourceRepoMapBeginBodyV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    old_active: RepoMapExpectedActiveV2,
    payload_bytes: u64,
    payload_sha256: Digest,
});

struct SourceRepoMapCompleteBodyV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
}
restore_open_record!([map_only] SourceRepoMapCompleteBodyV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
});

struct SourceInventoryCompleteBodyV1 {
    head_count: u64,
    repomap_head_count: u64,
}
restore_open_record!([map_only] SourceInventoryCompleteBodyV1 {
    head_count: u64,
    repomap_head_count: u64,
});

macro_rules! source_frame_decode_body {
    ($decoder:ident, $record:ident, $variant:ident { $($field:ident),+ $(,)? }) => {{
        let body: $record = serde::Deserialize::deserialize($decoder)?;
        Ok(SourceSnapshotExportFrameV1::$variant { $($field: body.$field,)+ })
    }};
}

struct SourceFrameBodySeedV1(SourceFrameKindV1);

impl<'de> serde::de::DeserializeSeed<'de> for SourceFrameBodySeedV1 {
    type Value = SourceSnapshotExportFrameV1;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        match self.0 {
            SourceFrameKindV1::HeadBegin => source_frame_decode_body!(
                decoder,
                SourceHeadBeginBodyV1,
                HeadBegin {
                    repo_id,
                    revision_id,
                    active,
                    applied_ack,
                    source_publication_event,
                    source_heads,
                    repomap_active
                }
            ),
            SourceFrameKindV1::FileBegin => source_frame_decode_body!(
                decoder,
                SourceFileBeginBodyV1,
                FileBegin {
                    source,
                    text_admitted,
                    source_bytes
                }
            ),
            SourceFrameKindV1::FileChunk => {
                source_frame_decode_body!(decoder, SourceChunkBodyV1, FileChunk { offset, bytes })
            }
            SourceFrameKindV1::FileEnd => {
                source_frame_decode_body!(decoder, SourceFileEndBodyV1, FileEnd { source })
            }
            SourceFrameKindV1::HeadComplete => source_frame_decode_body!(
                decoder,
                SourceHeadCompleteBodyV1,
                HeadComplete {
                    repo_id,
                    revision_id,
                    file_count,
                    source_heads
                }
            ),
            SourceFrameKindV1::RepoMapBegin => source_frame_decode_body!(
                decoder,
                SourceRepoMapBeginBodyV1,
                RepoMapBegin {
                    repo_id,
                    revision_id,
                    generation,
                    old_active,
                    payload_bytes,
                    payload_sha256
                }
            ),
            SourceFrameKindV1::RepoMapChunk => source_frame_decode_body!(
                decoder,
                SourceChunkBodyV1,
                RepoMapChunk { offset, bytes }
            ),
            SourceFrameKindV1::RepoMapComplete => source_frame_decode_body!(
                decoder,
                SourceRepoMapCompleteBodyV1,
                RepoMapComplete {
                    repo_id,
                    revision_id
                }
            ),
            SourceFrameKindV1::InventoryComplete => source_frame_decode_body!(
                decoder,
                SourceInventoryCompleteBodyV1,
                InventoryComplete {
                    head_count,
                    repomap_head_count
                }
            ),
        }
    }
}

// With the body first, the previous adjacent-tag decoder validated JSON
// scalars before retaining its buffered body. Validate the same input while
// discarding each value, so field order never requires a temporary DOM.
struct SourceFrameValueCheckV1;

impl<'de> serde::Deserialize<'de> for SourceFrameValueCheckV1 {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct ValueVisitor;
        impl<'de> serde::de::Visitor<'de> for ValueVisitor {
            type Value = SourceFrameValueCheckV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a valid JSON value")
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(SourceFrameValueCheckV1)
            }

            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                self.visit_unit()
            }

            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                self.visit_unit()
            }

            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                self.visit_unit()
            }

            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                self.visit_unit()
            }

            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
                self.visit_unit()
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                while sequence
                    .next_element::<SourceFrameValueCheckV1>()?
                    .is_some()
                {}
                self.visit_unit()
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                while map
                    .next_entry::<SourceFrameValueCheckV1, SourceFrameValueCheckV1>()?
                    .is_some()
                {}
                self.visit_unit()
            }
        }
        decoder.deserialize_any(ValueVisitor)
    }
}

struct SourceFrameKindReadV1;

impl SourceFrameKindReadV1 {
    fn decode(bytes: &[u8]) -> Result<SourceFrameKindV1, serde_json::Error> {
        struct KindReader;
        impl<'de> serde::de::DeserializeSeed<'de> for KindReader {
            type Value = SourceFrameKindV1;
            fn deserialize<D: serde::Deserializer<'de>>(
                self,
                decoder: D,
            ) -> Result<Self::Value, D::Error> {
                struct Visitor;
                impl<'de> serde::de::Visitor<'de> for Visitor {
                    type Value = SourceFrameKindV1;
                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str("an adjacent-tag Source export frame")
                    }
                    fn visit_map<A: serde::de::MapAccess<'de>>(
                        self,
                        mut map: A,
                    ) -> Result<Self::Value, A::Error> {
                        let mut kind = None;
                        let mut body_seen = false;
                        while let Some(key) =
                            map.next_key_seed(RestoreFieldKeyV1(&["kind", "body"]))?
                        {
                            match key {
                                Some("kind") => {
                                    if kind.is_some() {
                                        return Err(serde::de::Error::duplicate_field("kind"));
                                    }
                                    kind = Some(map.next_value::<SourceFrameKindV1>()?);
                                }
                                Some("body") => {
                                    if body_seen {
                                        return Err(serde::de::Error::duplicate_field("body"));
                                    }
                                    if kind.is_none() {
                                        let _: SourceFrameValueCheckV1 = map.next_value()?;
                                    } else {
                                        let _: serde::de::IgnoredAny = map.next_value()?;
                                    }
                                    body_seen = true;
                                }
                                None => {
                                    let _: serde::de::IgnoredAny = map.next_value()?;
                                }
                                _ => unreachable!("restore field seed only returns schema names"),
                            }
                        }
                        if !body_seen {
                            return Err(serde::de::Error::missing_field("body"));
                        }
                        kind.ok_or_else(|| serde::de::Error::missing_field("kind"))
                    }
                    fn visit_seq<A: serde::de::SeqAccess<'de>>(
                        self,
                        mut seq: A,
                    ) -> Result<Self::Value, A::Error> {
                        let kind = seq
                            .next_element::<SourceFrameKindV1>()?
                            .ok_or_else(|| serde::de::Error::missing_field("kind"))?;
                        let _: serde::de::IgnoredAny = seq
                            .next_element()?
                            .ok_or_else(|| serde::de::Error::missing_field("body"))?;
                        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                            return Err(serde::de::Error::invalid_length(3, &self));
                        }
                        Ok(kind)
                    }
                }
                decoder.deserialize_struct(
                    "SourceSnapshotExportFrameV1",
                    &["kind", "body"],
                    Visitor,
                )
            }
        }
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let kind = serde::de::DeserializeSeed::deserialize(KindReader, &mut decoder)?;
        decoder.end()?;
        Ok(kind)
    }
}

struct SourceFrameDecodeSeedV1(SourceFrameKindV1);

impl<'de> serde::de::DeserializeSeed<'de> for SourceFrameDecodeSeedV1 {
    type Value = SourceSnapshotExportFrameV1;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        struct FrameVisitor(SourceFrameKindV1);
        impl<'de> serde::de::Visitor<'de> for FrameVisitor {
            type Value = SourceSnapshotExportFrameV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an adjacent-tag Source export frame")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut kind_seen = false;
                let mut body = None;
                while let Some(key) = map.next_key_seed(RestoreFieldKeyV1(&["kind", "body"]))? {
                    match key {
                        Some("kind") => {
                            if kind_seen {
                                return Err(serde::de::Error::duplicate_field("kind"));
                            }
                            let actual = map.next_value::<SourceFrameKindV1>()?;
                            if actual != self.0 {
                                return Err(serde::de::Error::custom(
                                    "Source frame kind changed between passes",
                                ));
                            }
                            kind_seen = true;
                        }
                        Some("body") => {
                            if body.is_some() {
                                return Err(serde::de::Error::duplicate_field("body"));
                            }
                            body = Some(map.next_value_seed(SourceFrameBodySeedV1(self.0))?);
                        }
                        None => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                        _ => unreachable!("restore field seed only returns schema names"),
                    }
                }
                if !kind_seen {
                    return Err(serde::de::Error::missing_field("kind"));
                }
                body.ok_or_else(|| serde::de::Error::missing_field("body"))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let actual = seq
                    .next_element::<SourceFrameKindV1>()?
                    .ok_or_else(|| serde::de::Error::missing_field("kind"))?;
                if actual != self.0 {
                    return Err(serde::de::Error::custom(
                        "Source frame kind changed between passes",
                    ));
                }
                let body = seq
                    .next_element_seed(SourceFrameBodySeedV1(self.0))?
                    .ok_or_else(|| serde::de::Error::missing_field("body"))?;
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(3, &self));
                }
                Ok(body)
            }
        }
        decoder.deserialize_struct(
            "SourceSnapshotExportFrameV1",
            &["kind", "body"],
            FrameVisitor(self.0),
        )
    }
}

impl SourceSnapshotExportFrameV1 {
    /// Decode one already framed JSON payload without buffering an untyped body.
    /// The caller owns the frame-length and memory admission before this call.
    pub fn decode_json_v1(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let kind = SourceFrameKindReadV1::decode(bytes)?;
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let frame =
            serde::de::DeserializeSeed::deserialize(SourceFrameDecodeSeedV1(kind), &mut decoder)?;
        decoder.end()?;
        Ok(frame)
    }
}

pub const MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1: usize = 64 * 1024;

/// Source identity issued only after a producer Original has completed the
/// entire physically exported Legacy inventory. This DTO is not an issuer or
/// a substitute for the FD6 retained Source fact that carries it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacySourceOriginV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub active: SearchCorpusActiveHeadV1,
    pub applied_ack: BatchPublishReceipt,
    pub source_publication_event: SourcePublicationEvent,
    pub source_heads: LexicalSourceHeadsCommitmentV1,
}

restore_open_record!(LegacySourceOriginV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
    active: SearchCorpusActiveHeadV1,
    applied_ack: BatchPublishReceipt,
    source_publication_event: SourcePublicationEvent,
    source_heads: LexicalSourceHeadsCommitmentV1,
});

impl LegacySourceOriginV1 {
    #[must_use]
    pub fn is_canonical_v1(&self) -> bool {
        self.active.validate_v1().is_ok()
            && self.repo_id == self.active.generation.lexical.repo_id
            && self.revision_id == self.active.generation.lexical.revision_id
            && self.applied_ack.generation == self.active.generation.lexical.manifest_generation
            && self.applied_ack.manifest_digest.as_deref()
                == Some(self.active.generation.lexical.manifest_digest.as_str())
            && self.applied_ack.sealed
            && self.applied_ack.semantic_content.as_ref()
                == Some(&self.active.generation.semantic_content)
            && self.source_publication_event.validate().is_ok()
            && self.source_heads.is_canonical_v1()
    }
}

/// Exact old physical Source origin joined to one published Native Source
/// operation in a separate sibling workspace. The production issuer must
/// retain the completed Original snapshot and the same-operation receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeImportedSourceHeadV1 {
    pub origin: LegacySourceOriginV1,
    pub candidate_root_nonce: Digest,
    pub source_operation: ImmutableOperation,
    pub source_publication: CatalogView,
    /// The exact producer Source completion retained with its settled V9.
    pub source_binding: NativeCutoverEvidenceSourcePublicationBindingV1,
    pub original_snapshot_id: u64,
    pub accepted_transcript_sha256: Digest,
    pub source_manifest_sha256: Digest,
    pub completed_journal_digest: Digest,
}

restore_open_record!(NativeImportedSourceHeadV1 {
    origin: LegacySourceOriginV1,
    candidate_root_nonce: Digest,
    source_operation: ImmutableOperation,
    source_publication: CatalogView,
    source_binding: NativeCutoverEvidenceSourcePublicationBindingV1,
    original_snapshot_id: u64,
    accepted_transcript_sha256: Digest,
    source_manifest_sha256: Digest,
    completed_journal_digest: Digest,
});

impl NativeImportedSourceHeadV1 {
    #[must_use]
    pub fn is_canonical_v1(&self) -> bool {
        self.origin.is_canonical_v1()
            && self.candidate_root_nonce.0 != [0; 32]
            && self.original_snapshot_id != 0
            && self.source_binding.target_snapshot_id == self.original_snapshot_id
            && self.accepted_transcript_sha256.0 != [0; 32]
            && self.source_manifest_sha256.0 != [0; 32]
            && self.completed_journal_digest.0 != [0; 32]
            && imported_source_publication_scope_v1(
                self.source_operation.key,
                self.source_publication.identity,
            )
            && self.source_publication.identity_digest().ok()
                == Some(self.source_publication.identity.digest)
    }

    #[must_use]
    pub fn matches_completed_binding_v1(
        &self,
        target: &NativeCutoverEvidenceTargetV1,
        request: &UpdateRequest,
    ) -> bool {
        if !self.is_canonical_v1() || self.source_operation.key != target.operation {
            return false;
        }
        let Some(manifest) = imported_legacy_source_manifest_digest_v1(
            &self.origin,
            self.accepted_transcript_sha256,
            self.source_manifest_sha256,
            self.original_snapshot_id,
            self.candidate_root_nonce,
            target,
            request,
        ) else {
            return false;
        };
        let Some(completed) = imported_legacy_source_completed_digest_v1(
            manifest,
            &self.source_operation,
            &self.source_publication,
        ) else {
            return false;
        };
        self.completed_journal_digest == completed
            && self.source_binding.transition_digest == digest64_lower_v1(manifest)
            && self.source_binding.completion_pointer_digest == digest64_lower_v1(completed)
            && self.source_binding.physical_target_source_digest
                == digest64_lower_v1(self.source_manifest_sha256)
    }
}

fn imported_source_publication_scope_v1(operation: OperationKey, view: ViewRef) -> bool {
    operation.workspace == view.workspace && operation.epoch == view.epoch && view.sequence != 0
}

#[cfg(test)]
mod tests {
    use super::{
        LegacySourceManifestDigestV1, LexicalSourceHeadsCommitmentV1, SourceSnapshotExportFrameV1,
        imported_source_publication_scope_v1,
    };
    use crate::index::{
        CollectionId, Digest, OperationKey, ProducerId, SlotId, StreamId, TenantId, ViewRef,
        WorkspaceKey, WorktreeId,
    };
    use crate::{
        BatchPublishReceipt, CandidateCommitmentV1, GenerationSnapshot, ManifestGeneration, RepoId,
        RepoRelativePath, RevisionId, SearchCorpusActivationTokenV1, SearchCorpusActiveHeadV1,
        SearchCorpusGenerationIdentityV1, SearchPlaneTrackKind, SemanticContentRootsV1,
        SourceFileKey, SourceFileRevision, SourcePublicationEvent,
        repomap::RepoMapExpectedActiveV2,
    };
    use sha2::{Digest as _, Sha256};
    use std::num::NonZeroU64;

    fn sample_source_export_frames_v1() -> Vec<SourceSnapshotExportFrameV1> {
        let repo_id = RepoId::new("repo").expect("repo");
        let revision_id = RevisionId::new("rev").expect("revision");
        let generation = ManifestGeneration::new(7);
        let snapshot = |track| GenerationSnapshot {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
            manifest_generation: generation,
            manifest_digest: "digest".into(),
        };
        let active = SearchCorpusActiveHeadV1 {
            generation: SearchCorpusGenerationIdentityV1 {
                lexical: snapshot(SearchPlaneTrackKind::Lexical),
                semantic: snapshot(SearchPlaneTrackKind::Semantic),
                semantic_content: SemanticContentRootsV1 {
                    row_root_digest: format!("sha256:{}", "a".repeat(64)),
                    membership_root_digest: format!("sha256:{}", "b".repeat(64)),
                },
            },
            activation_token: SearchCorpusActivationTokenV1::new(
                [1; crate::ACTIVATION_ROOT_INCARNATION_BYTES_V1],
                NonZeroU64::new(1).expect("sequence"),
            )
            .expect("activation token"),
        };
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: repo_id.clone(),
                repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            },
            revision_id: revision_id.clone(),
            source_sha256: [2; 32],
        };
        let source_heads = LexicalSourceHeadsCommitmentV1 {
            root_sha256: Some([3; 32]),
            entries: 1,
        };
        let old_active = RepoMapExpectedActiveV2::new(
            NonZeroU64::new(1).expect("epoch"),
            CandidateCommitmentV1::from_bytes([4; 32]),
        );
        vec![
            SourceSnapshotExportFrameV1::HeadBegin {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                active,
                applied_ack: BatchPublishReceipt::empty_for(
                    generation,
                    Some("digest".into()),
                    "batch",
                ),
                source_publication_event: SourcePublicationEvent {
                    stream_id: "stream".into(),
                    event_id: "event".into(),
                    expected_base_event_id: None,
                    payload_sha256: [5; 32],
                },
                source_heads,
                repomap_active: None,
            },
            SourceSnapshotExportFrameV1::FileBegin {
                source: source.clone(),
                text_admitted: true,
                source_bytes: 2,
            },
            SourceSnapshotExportFrameV1::FileChunk {
                offset: 0,
                bytes: vec![0, 255],
            },
            SourceSnapshotExportFrameV1::FileEnd { source },
            SourceSnapshotExportFrameV1::HeadComplete {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                file_count: 1,
                source_heads,
            },
            SourceSnapshotExportFrameV1::RepoMapBegin {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                generation,
                old_active,
                payload_bytes: 2,
                payload_sha256: Digest([6; 32]),
            },
            SourceSnapshotExportFrameV1::RepoMapChunk {
                offset: 0,
                bytes: vec![1, 2],
            },
            SourceSnapshotExportFrameV1::RepoMapComplete {
                repo_id,
                revision_id,
            },
            SourceSnapshotExportFrameV1::InventoryComplete {
                head_count: 1,
                repomap_head_count: 1,
            },
        ]
    }

    #[test]
    fn source_frame_json_codec_preserves_all_variants_and_body_first() {
        for frame in sample_source_export_frames_v1() {
            let wire = serde_json::to_string(&frame).expect("encode frame");
            assert!(wire.starts_with("{\"kind\":"), "{wire}");
            assert_eq!(
                SourceSnapshotExportFrameV1::decode_json_v1(wire.as_bytes())
                    .expect("two-pass decoder"),
                frame
            );
            let (kind, body_with_closing_brace) = wire
                .strip_prefix("{\"kind\":")
                .expect("adjacent tag")
                .split_once(",\"body\":")
                .expect("body field");
            let body = body_with_closing_brace
                .strip_suffix('}')
                .expect("outer close");
            let body_first = format!("{{\"body\":{body},\"kind\":{kind}}}");
            assert_eq!(
                SourceSnapshotExportFrameV1::decode_json_v1(body_first.as_bytes())
                    .expect("two-pass body-first decoder"),
                frame
            );
        }
    }

    #[test]
    fn source_frame_json_codec_keeps_fixed_wire_and_decode_edge_cases() {
        let frame = SourceSnapshotExportFrameV1::FileChunk {
            offset: 7,
            bytes: vec![0, 1, 255],
        };
        assert_eq!(
            serde_json::to_string(&frame).expect("frame JSON"),
            r#"{"kind":"file_chunk","body":{"offset":7,"bytes":[0,1,255]}}"#
        );
        let full_chunk = SourceSnapshotExportFrameV1::FileChunk {
            offset: 0,
            bytes: vec![255; super::MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1],
        };
        let full_wire = serde_json::to_vec(&full_chunk).expect("full chunk JSON");
        assert_eq!(
            SourceSnapshotExportFrameV1::decode_json_v1(&full_wire).expect("full chunk decode"),
            full_chunk
        );
        for wire in [
            r#"{"body":{"offset":7,"bytes":[0,1,255],"future":true},"kind":"file_chunk","future":{}}"#,
            r#"["file_chunk",{"offset":7,"bytes":[0,1,255]}]"#,
            r#"{"kind":"file_chunk","body":{"offset":7,"bytes":[0,1,255],"future":1e9999}}"#,
        ] {
            assert_eq!(
                SourceSnapshotExportFrameV1::decode_json_v1(wire.as_bytes()).expect("file chunk"),
                frame
            );
        }
        for wire in [
            r#"{"kind":"file_chunk","body":{"offset":7,"bytes":[0]},"kind":"file_chunk"}"#,
            r#"{"body":{"offset":7,"bytes":[0]},"kind":"file_chunk","body":{}}"#,
            r#"{"kind":"file_chunk","body":{"offset":7,"offset":8,"bytes":[0]}}"#,
            r#"["file_chunk",[7,[0,1,255]]]"#,
            r#"{"kind":"file_chunk","body":[7,[0,1,255]]}"#,
            r#"{"body":[7,[0,1,255]],"kind":"file_chunk"}"#,
            r#"{"body":{"offset":7,"bytes":[0],"future":1e9999},"kind":"file_chunk"}"#,
            r#"{"body":{"offset":7,"bytes":[0],"future":"\ud800"},"kind":"file_chunk"}"#,
            r#"{"kind":"file_chunk"}"#,
            r#"{"body":{"offset":7,"bytes":[0]}}"#,
        ] {
            assert!(
                SourceSnapshotExportFrameV1::decode_json_v1(wire.as_bytes()).is_err(),
                "{wire}"
            );
        }
    }

    #[test]
    fn source_heads_wire_preserves_open_map_and_nullable_root() {
        let expected = LexicalSourceHeadsCommitmentV1 {
            root_sha256: None,
            entries: 0,
        };
        assert_eq!(
            serde_json::to_string(&expected).expect("source heads JSON"),
            r#"{"root_sha256":null,"entries":0}"#
        );
        for wire in [
            r#"{"entries":0}"#,
            r#"{"root_sha256":null,"entries":0,"future":{"value":1}}"#,
        ] {
            assert_eq!(
                serde_json::from_str::<LexicalSourceHeadsCommitmentV1>(wire)
                    .expect("legacy open map"),
                expected
            );
        }
        assert!(
            serde_json::from_str::<LexicalSourceHeadsCommitmentV1>(
                r#"{"root_sha256":null,"root_sha256":null,"entries":0}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<LexicalSourceHeadsCommitmentV1>(r#"{"root_sha256":null}"#)
                .is_err()
        );

        let mut cbor = Vec::new();
        let value = ciborium::value::Value::Map(vec![
            (
                ciborium::value::Value::Bytes(b"root_sha256".to_vec()),
                ciborium::value::Value::Null,
            ),
            (
                ciborium::value::Value::Text("entries".into()),
                ciborium::value::Value::Integer(0.into()),
            ),
            (
                ciborium::value::Value::Bytes(b"future".to_vec()),
                ciborium::value::Value::Null,
            ),
            (
                ciborium::value::Value::Bytes(vec![0xff]),
                ciborium::value::Value::Null,
            ),
        ]);
        ciborium::ser::into_writer(&value, &mut cbor).expect("CBOR open map");
        assert_eq!(
            ciborium::de::from_reader::<LexicalSourceHeadsCommitmentV1, _>(cbor.as_slice())
                .expect("legacy byte keys"),
            expected
        );
    }

    #[test]
    fn imported_source_operation_sequence_is_independent_of_view_sequence() {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let operation = OperationKey {
            workspace,
            epoch: 2,
            producer: ProducerId([2; 16]),
            stream: StreamId([3; 16]),
            stream_epoch: 1,
            sequence: 41,
        };
        let view = ViewRef {
            workspace,
            epoch: 2,
            sequence: 3,
            digest: Digest([4; 32]),
        };
        assert!(imported_source_publication_scope_v1(operation, view));
        assert!(!imported_source_publication_scope_v1(
            operation,
            ViewRef {
                workspace: WorkspaceKey {
                    slot: SlotId([5; 16]),
                    ..workspace
                },
                ..view
            }
        ));
    }

    #[test]
    fn source_heads_commitment_requires_root_exactly_for_nonempty_census() {
        assert!(
            LexicalSourceHeadsCommitmentV1 {
                root_sha256: None,
                entries: 0
            }
            .is_canonical_v1()
        );
        assert!(
            !LexicalSourceHeadsCommitmentV1 {
                root_sha256: None,
                entries: 1
            }
            .is_canonical_v1()
        );
        assert!(
            !LexicalSourceHeadsCommitmentV1 {
                root_sha256: Some([0; 32]),
                entries: 0
            }
            .is_canonical_v1()
        );
        assert!(
            LexicalSourceHeadsCommitmentV1 {
                root_sha256: Some([0; 32]),
                entries: 1
            }
            .is_canonical_v1()
        );
    }

    #[test]
    fn imported_source_manifest_binds_exact_binary_body_and_source_repo() {
        let bytes = [0_u8, 0xff, 4_u8];
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("foreign").expect("repo"),
                repo_relative_path: RepoRelativePath::new("src/raw.bin"),
            },
            revision_id: RevisionId::new("a".repeat(64)).expect("revision"),
            source_sha256: Sha256::digest(bytes).into(),
        };
        let mut original = LegacySourceManifestDigestV1::new_v1();
        original
            .append_file_v1(&source, false, &bytes)
            .expect("binary original");
        let (digest, count) = original.finish_v1();
        assert_eq!(count, 1);
        let mut same = LegacySourceManifestDigestV1::new_v1();
        same.append_file_v1(&source, false, &bytes)
            .expect("same original");
        assert_eq!(same.finish_v1().0, digest);
        let mut foreign = source.clone();
        foreign.file.source_repo_id = RepoId::new("other").expect("repo");
        let mut changed = LegacySourceManifestDigestV1::new_v1();
        changed
            .append_file_v1(&foreign, false, &bytes)
            .expect("other owner");
        assert_ne!(changed.finish_v1().0, digest);
        let mut refused = LegacySourceManifestDigestV1::new_v1();
        assert!(refused.append_file_v1(&source, true, &bytes).is_none());
        assert!(refused.append_file_v1(&source, false, &[1, 2, 3]).is_none());
    }
}
