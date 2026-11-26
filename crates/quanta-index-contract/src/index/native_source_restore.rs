//! Provisional Native Source-only export frames. The selected predecessor
//! lease and physical Catalog, not these DTOs, establish source authority.

use super::{
    CatalogView, Digest, ImmutableOperation, NativeProtectedSourceHeadV1, ObjectRef, UpdateRequest,
};
use super::{frame_body_replay::BodyReplay, source_restore::FrameJsonValueCheckV1};
use crate::native_cutover_evidence_v1::{
    NativeCutoverEvidenceSourcePublicationBindingV1, NativeCutoverEvidenceTargetV1,
};
use crate::{RepoId, RevisionId, SourceFileRevision};
use quanta_index_contract_base::NativeCutoverDigest64V1;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest as _, Sha256};
use std::io::{self, Write};

struct HashWriterV1<'a>(&'a mut Sha256);

impl Write for HashWriterV1<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn hash_json_v1(hash: &mut Sha256, value: &impl Serialize) -> Result<(), serde_json::Error> {
    serde_json::to_writer(HashWriterV1(hash), value)
}

fn json_sha256_v1(value: &impl Serialize) -> Option<Digest> {
    let mut hash = Sha256::new();
    hash_json_v1(&mut hash, value).ok()?;
    Some(Digest(hash.finalize().into()))
}

/// Old Native Source provenance for one protected repo/revision pair. The
/// exporter authenticates this against its selected root; the S issuer retains
/// its completed Original and exact transcript before issuing a new binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSourceOriginV1 {
    pub old_root_nonce: Digest,
    pub old_routes_ref: ObjectRef,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub old_source_head: NativeProtectedSourceHeadV1,
    pub file_count: u64,
    pub file_census_sha256: Digest,
    pub accepted_transcript_sha256: Digest,
}
crate::native_serde_struct!(NativeSourceOriginV1 {
    old_root_nonce: Digest,
    old_routes_ref: ObjectRef,
    repo_id: RepoId,
    revision_id: RevisionId,
    old_source_head: NativeProtectedSourceHeadV1,
    file_count: u64,
    file_census_sha256: Digest,
    accepted_transcript_sha256: Digest
});

impl NativeSourceOriginV1 {
    #[must_use]
    pub fn is_canonical_v1(&self) -> bool {
        self.old_root_nonce.0 != [0; 32]
            && self.old_routes_ref.0.0 != [0; 32]
            && self.file_census_sha256.0 != [0; 32]
            && self.accepted_transcript_sha256.0 != [0; 32]
            && self.repo_id == self.old_source_head.repo_id
            && self.revision_id == self.old_source_head.revision_id
            && self
                .old_source_head
                .source_publication
                .identity_digest()
                .ok()
                == Some(self.old_source_head.source_publication.identity.digest)
    }

    pub fn digest_v1(&self) -> Result<Digest, serde_json::Error> {
        let mut hash = Sha256::new();
        hash.update(b"quanta-index/native-source-origin/v1\0");
        hash_json_v1(&mut hash, self)?;
        Ok(Digest(hash.finalize().into()))
    }
}

/// Distinguish the intended new imported Source manifest from the old physical
/// file census while binding exactly the same file identities and metadata.
#[must_use]
pub fn native_source_target_manifest_digest_v1(file_census_sha256: Digest) -> Digest {
    let mut hash = Sha256::new();
    hash.update(b"quanta-index/native-source-target-manifest/v1\0");
    hash.update(file_census_sha256.0);
    Digest(hash.finalize().into())
}

/// Completed S Original association from an authenticated old Native Source
/// pair to one newly published candidate Source pair. A digest alone is data;
/// the installed verifier must join this to the durable S issuer and Q roots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeImportedNativeSourceHeadV1 {
    pub origin: NativeSourceOriginV1,
    pub candidate_root_nonce: Digest,
    pub source_operation: ImmutableOperation,
    pub source_publication: CatalogView,
    pub source_binding: NativeCutoverEvidenceSourcePublicationBindingV1,
    pub original_snapshot_id: u64,
    pub original_request_sha256: Digest,
    pub new_source_manifest_sha256: Digest,
    pub completed_journal_digest: Digest,
}
crate::native_serde_struct!(NativeImportedNativeSourceHeadV1 {
    origin: NativeSourceOriginV1,
    candidate_root_nonce: Digest,
    source_operation: ImmutableOperation,
    source_publication: CatalogView,
    source_binding: NativeCutoverEvidenceSourcePublicationBindingV1,
    original_snapshot_id: u64,
    original_request_sha256: Digest,
    new_source_manifest_sha256: Digest,
    completed_journal_digest: Digest
});

pub fn imported_native_source_manifest_digest_v1(
    origin: &NativeSourceOriginV1,
    candidate_root_nonce: Digest,
    original_snapshot_id: u64,
    new_source_manifest_sha256: Digest,
    target: &NativeCutoverEvidenceTargetV1,
    request: &UpdateRequest,
) -> Option<Digest> {
    if !origin.is_canonical_v1()
        || candidate_root_nonce.0 == [0; 32]
        || original_snapshot_id == 0
        || new_source_manifest_sha256.0 == [0; 32]
        || target.operation != request.operation
    {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(b"quanta-index/imported-native-source-manifest/v1\0");
    hash.update(origin.digest_v1().ok()?.0);
    hash.update(candidate_root_nonce.0);
    hash.update(original_snapshot_id.to_le_bytes());
    hash.update(new_source_manifest_sha256.0);
    hash_json_v1(&mut hash, target).ok()?;
    hash_json_v1(&mut hash, request).ok()?;
    Some(Digest(hash.finalize().into()))
}

pub fn imported_native_source_completed_digest_v1(
    manifest: Digest,
    operation: &ImmutableOperation,
    publication: &CatalogView,
) -> Option<Digest> {
    if manifest.0 == [0; 32]
        || operation.key.workspace != publication.identity.workspace
        || operation.key.epoch != publication.identity.epoch
        || publication.identity_digest().ok() != Some(publication.identity.digest)
    {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(b"quanta-index/imported-native-source-completed/v1\0");
    hash.update(manifest.0);
    hash_json_v1(&mut hash, operation).ok()?;
    hash_json_v1(&mut hash, publication).ok()?;
    Some(Digest(hash.finalize().into()))
}

impl NativeImportedNativeSourceHeadV1 {
    #[must_use]
    pub fn matches_completed_binding_v1(
        &self,
        target: &NativeCutoverEvidenceTargetV1,
        request: &UpdateRequest,
    ) -> bool {
        let Some(request_sha256) = json_sha256_v1(request) else {
            return false;
        };
        if self.source_operation.key != target.operation
            || self.original_request_sha256.0 == [0; 32]
            || self.original_request_sha256 != request_sha256
            || self.source_binding.target_snapshot_id != self.original_snapshot_id
            || self.new_source_manifest_sha256
                != native_source_target_manifest_digest_v1(self.origin.file_census_sha256)
        {
            return false;
        }
        let Some(manifest) = imported_native_source_manifest_digest_v1(
            &self.origin,
            self.candidate_root_nonce,
            self.original_snapshot_id,
            self.new_source_manifest_sha256,
            target,
            request,
        ) else {
            return false;
        };
        let Some(completed) = imported_native_source_completed_digest_v1(
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
                == digest64_lower_v1(self.new_source_manifest_sha256)
    }
}

/// FD6 carries only the issuer's commitment. The installed verifier restores
/// the omitted old head and new view from held physical Catalog roots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeImportedNativeSourceCompactV1 {
    pub origin: NativeSourceOriginCompactV1,
    pub candidate_root_nonce: Digest,
    pub source_operation: ImmutableOperation,
    pub source_publication_sha256: Digest,
    pub source_binding: NativeCutoverEvidenceSourcePublicationBindingV1,
    pub original_snapshot_id: u64,
    pub original_request_sha256: Digest,
    pub new_source_manifest_sha256: Digest,
    pub completed_journal_digest: Digest,
}
crate::native_serde_struct!(NativeImportedNativeSourceCompactV1 {
    origin: NativeSourceOriginCompactV1,
    candidate_root_nonce: Digest,
    source_operation: ImmutableOperation,
    source_publication_sha256: Digest,
    source_binding: NativeCutoverEvidenceSourcePublicationBindingV1,
    original_snapshot_id: u64,
    original_request_sha256: Digest,
    new_source_manifest_sha256: Digest,
    completed_journal_digest: Digest
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSourceOriginCompactV1 {
    pub old_root_nonce: Digest,
    pub old_routes_ref: ObjectRef,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub old_source_head_sha256: Digest,
    pub file_count: u64,
    pub file_census_sha256: Digest,
    pub accepted_transcript_sha256: Digest,
}
crate::native_serde_struct!(NativeSourceOriginCompactV1 {
    old_root_nonce: Digest,
    old_routes_ref: ObjectRef,
    repo_id: RepoId,
    revision_id: RevisionId,
    old_source_head_sha256: Digest,
    file_count: u64,
    file_census_sha256: Digest,
    accepted_transcript_sha256: Digest
});

impl NativeImportedNativeSourceCompactV1 {
    pub fn from_completed_v1(
        full: &NativeImportedNativeSourceHeadV1,
        target: &NativeCutoverEvidenceTargetV1,
        request: &UpdateRequest,
    ) -> Option<Self> {
        if !full.matches_completed_binding_v1(target, request) {
            return None;
        }
        Some(Self {
            origin: NativeSourceOriginCompactV1 {
                old_root_nonce: full.origin.old_root_nonce,
                old_routes_ref: full.origin.old_routes_ref,
                repo_id: full.origin.repo_id.clone(),
                revision_id: full.origin.revision_id.clone(),
                old_source_head_sha256: json_sha256_v1(&full.origin.old_source_head)?,
                file_count: full.origin.file_count,
                file_census_sha256: full.origin.file_census_sha256,
                accepted_transcript_sha256: full.origin.accepted_transcript_sha256,
            },
            candidate_root_nonce: full.candidate_root_nonce,
            source_operation: full.source_operation,
            source_publication_sha256: json_sha256_v1(&full.source_publication)?,
            source_binding: full.source_binding.clone(),
            original_snapshot_id: full.original_snapshot_id,
            original_request_sha256: full.original_request_sha256,
            new_source_manifest_sha256: full.new_source_manifest_sha256,
            completed_journal_digest: full.completed_journal_digest,
        })
    }

    #[must_use]
    pub fn matches_request_v1(
        &self,
        target: &NativeCutoverEvidenceTargetV1,
        request: &UpdateRequest,
    ) -> bool {
        self.origin.old_root_nonce.0 != [0; 32]
            && self.origin.old_routes_ref.0.0 != [0; 32]
            && self.origin.old_source_head_sha256.0 != [0; 32]
            && self.origin.file_census_sha256.0 != [0; 32]
            && self.origin.accepted_transcript_sha256.0 != [0; 32]
            && self.candidate_root_nonce.0 != [0; 32]
            && self.source_publication_sha256.0 != [0; 32]
            && self.completed_journal_digest.0 != [0; 32]
            && self.original_request_sha256.0 != [0; 32]
            && self.source_operation.key == target.operation
            && self.original_request_sha256 == json_sha256_v1(request).unwrap_or(Digest([0; 32]))
            && self.source_binding.target_snapshot_id == self.original_snapshot_id
            && self.new_source_manifest_sha256
                == native_source_target_manifest_digest_v1(self.origin.file_census_sha256)
            && self.source_binding.physical_target_source_digest
                == digest64_lower_v1(self.new_source_manifest_sha256)
    }

    #[must_use]
    pub fn matches_completed_v1(
        &self,
        full: &NativeImportedNativeSourceHeadV1,
        target: &NativeCutoverEvidenceTargetV1,
        request: &UpdateRequest,
    ) -> bool {
        Self::from_completed_v1(full, target, request).as_ref() == Some(self)
    }
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

/// Private pipe format for every protected Source head of one selected Native
/// root. File revisions are new content-derived import identities; the old
/// Native SourceRecord has no SourceFileRevision to preserve.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeSourceSnapshotExportFrameV1 {
    HeadBegin {
        source_root_nonce: Digest,
        source_routes: ObjectRef,
        repo_id: RepoId,
        revision_id: RevisionId,
        source_head: NativeProtectedSourceHeadV1,
        file_count: u64,
    },
    FileBegin {
        source: SourceFileRevision,
        text_admitted: bool,
        source_bytes: u64,
        language: Option<String>,
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
        file_census_sha256: Digest,
    },
    /// Authenticated old route identities, exported without RepoMap payload.
    RetiredProjectionIdentity {
        projection: super::ProjectionId,
        instance: super::IndexInstanceId,
    },
    InventoryComplete {
        source_root_nonce: Digest,
        source_routes: ObjectRef,
        head_count: u64,
        route_head_count: u64,
        retired_identity_count: u64,
        file_census_sha256: Digest,
        transcript_sha256: Digest,
    },
}

struct NativeSourceFieldKeyV1(&'static [&'static str]);

impl<'de> serde::de::DeserializeSeed<'de> for NativeSourceFieldKeyV1 {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl serde::de::Visitor<'_> for NativeSourceFieldKeyV1 {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a Native Source export frame field")
    }

    fn visit_str<E: serde::de::Error>(self, field: &str) -> Result<Self::Value, E> {
        self.0
            .iter()
            .copied()
            .find(|known| *known == field)
            .ok_or_else(|| E::unknown_field(field, self.0))
    }

    fn visit_bytes<E: serde::de::Error>(self, field: &[u8]) -> Result<Self::Value, E> {
        let name = std::str::from_utf8(field)
            .map_err(|_| E::invalid_value(serde::de::Unexpected::Bytes(field), &self))?;
        self.visit_str(name)
    }

    fn visit_u64<E: serde::de::Error>(self, field: u64) -> Result<Self::Value, E> {
        usize::try_from(field)
            .ok()
            .and_then(|ordinal| self.0.get(ordinal))
            .copied()
            .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Unsigned(field), &self))
    }
}

macro_rules! native_source_frame_decode {
    ($decoder:expr, $variant:ident { $($field:ident: $ty:ty $(= $default:ident)?),+ $(,)? }) => {{
        const FIELDS: &[&str] = &[$(stringify!($field)),+];
        struct BodyVisitor;

        impl<'de> serde::de::Visitor<'de> for BodyVisitor {
            type Value = NativeSourceSnapshotExportFrameV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(concat!("struct variant ", stringify!($variant)))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                $(let mut $field: Option<$ty> = None;)+
                while let Some(key) = map.next_key_seed(NativeSourceFieldKeyV1(FIELDS))? {
                    match key {
                        $(stringify!($field) => {
                            if $field.is_some() {
                                return Err(serde::de::Error::duplicate_field(stringify!($field)));
                            }
                            $field = Some(map.next_value::<$ty>()?);
                        },)+
                        _ => unreachable!("closed field seed only returns schema keys"),
                    }
                }
                Ok(NativeSourceSnapshotExportFrameV1::$variant {
                    $($field: native_source_frame_decode!(@field $field $(= $default)?),)+
                })
            }
        }

        // Derived struct variants inside the adjacent body have a map visitor.
        $decoder.deserialize_any(BodyVisitor)
    }};
    (@field $field:ident) => {
        $field.ok_or_else(|| serde::de::Error::missing_field(stringify!($field)))?
    };
    (@field $field:ident = default) => {
        $field.unwrap_or_default()
    };
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum NativeSourceFrameKindV1 {
    HeadBegin,
    FileBegin,
    FileChunk,
    FileEnd,
    HeadComplete,
    RetiredProjectionIdentity,
    InventoryComplete,
}

impl NativeSourceFrameKindV1 {
    const NAMES: &'static [&'static str] = &[
        "head_begin",
        "file_begin",
        "file_chunk",
        "file_end",
        "head_complete",
        "retired_projection_identity",
        "inventory_complete",
    ];

    fn index(self) -> u32 {
        match self {
            Self::HeadBegin => 0,
            Self::FileBegin => 1,
            Self::FileChunk => 2,
            Self::FileEnd => 3,
            Self::HeadComplete => 4,
            Self::RetiredProjectionIdentity => 5,
            Self::InventoryComplete => 6,
        }
    }

    fn name(self) -> &'static str {
        Self::NAMES[self.index() as usize]
    }

    fn parse<E: serde::de::Error>(name: &str) -> Result<Self, E> {
        match name {
            "head_begin" => Ok(Self::HeadBegin),
            "file_begin" => Ok(Self::FileBegin),
            "file_chunk" => Ok(Self::FileChunk),
            "file_end" => Ok(Self::FileEnd),
            "head_complete" => Ok(Self::HeadComplete),
            "retired_projection_identity" => Ok(Self::RetiredProjectionIdentity),
            "inventory_complete" => Ok(Self::InventoryComplete),
            _ => Err(E::unknown_variant(name, Self::NAMES)),
        }
    }
}

impl Serialize for NativeSourceFrameKindV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_unit_variant(
            "NativeSourceSnapshotExportFrameV1",
            self.index(),
            self.name(),
        )
    }
}

struct NativeSourceFrameKindNameV1(NativeSourceFrameKindV1);

impl<'de> Deserialize<'de> for NativeSourceFrameKindNameV1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NameVisitor;
        impl serde::de::Visitor<'_> for NameVisitor {
            type Value = NativeSourceFrameKindNameV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a Native Source export frame kind")
            }

            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<Self::Value, E> {
                NativeSourceFrameKindV1::parse(name).map(NativeSourceFrameKindNameV1)
            }

            fn visit_bytes<E: serde::de::Error>(self, name: &[u8]) -> Result<Self::Value, E> {
                let name = std::str::from_utf8(name)
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Bytes(name), &self))?;
                NativeSourceFrameKindV1::parse(name).map(NativeSourceFrameKindNameV1)
            }

            fn visit_u64<E: serde::de::Error>(self, index: u64) -> Result<Self::Value, E> {
                match index {
                    0 => Ok(NativeSourceFrameKindNameV1(
                        NativeSourceFrameKindV1::HeadBegin,
                    )),
                    1 => Ok(NativeSourceFrameKindNameV1(
                        NativeSourceFrameKindV1::FileBegin,
                    )),
                    2 => Ok(NativeSourceFrameKindNameV1(
                        NativeSourceFrameKindV1::FileChunk,
                    )),
                    3 => Ok(NativeSourceFrameKindNameV1(
                        NativeSourceFrameKindV1::FileEnd,
                    )),
                    4 => Ok(NativeSourceFrameKindNameV1(
                        NativeSourceFrameKindV1::HeadComplete,
                    )),
                    5 => Ok(NativeSourceFrameKindNameV1(
                        NativeSourceFrameKindV1::RetiredProjectionIdentity,
                    )),
                    6 => Ok(NativeSourceFrameKindNameV1(
                        NativeSourceFrameKindV1::InventoryComplete,
                    )),
                    _ => Err(E::invalid_value(
                        serde::de::Unexpected::Unsigned(index),
                        &self,
                    )),
                }
            }
        }
        deserializer.deserialize_identifier(NameVisitor)
    }
}

impl<'de> Deserialize<'de> for NativeSourceFrameKindV1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KindVisitor;
        impl<'de> serde::de::Visitor<'de> for KindVisitor {
            type Value = NativeSourceFrameKindV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a Native Source export frame kind")
            }

            fn visit_enum<A: serde::de::EnumAccess<'de>>(
                self,
                data: A,
            ) -> Result<Self::Value, A::Error> {
                let (name, variant) =
                    serde::de::EnumAccess::variant::<NativeSourceFrameKindNameV1>(data)?;
                serde::de::VariantAccess::unit_variant(variant)?;
                Ok(name.0)
            }
        }
        deserializer.deserialize_enum(
            "NativeSourceSnapshotExportFrameV1",
            NativeSourceFrameKindV1::NAMES,
            KindVisitor,
        )
    }
}

macro_rules! native_source_frame_body_encode {
    ($serializer:ident, $name:literal, { $($field:ident),+ $(,)? }) => {{
        let mut body = $serializer.serialize_struct($name, [$(stringify!($field)),+].len())?;
        $(serde::ser::SerializeStruct::serialize_field(&mut body, stringify!($field), $field)?;)+
        serde::ser::SerializeStruct::end(body)
    }};
}

struct NativeSourceFrameBodyRefV1<'a>(&'a NativeSourceSnapshotExportFrameV1);

impl Serialize for NativeSourceFrameBodyRefV1<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            NativeSourceSnapshotExportFrameV1::HeadBegin {
                source_root_nonce,
                source_routes,
                repo_id,
                revision_id,
                source_head,
                file_count,
            } => native_source_frame_body_encode!(serializer, "head_begin", {
                source_root_nonce, source_routes, repo_id, revision_id, source_head, file_count
            }),
            NativeSourceSnapshotExportFrameV1::FileBegin {
                source,
                text_admitted,
                source_bytes,
                language,
            } => native_source_frame_body_encode!(serializer, "file_begin", {
                source, text_admitted, source_bytes, language
            }),
            NativeSourceSnapshotExportFrameV1::FileChunk { offset, bytes } => {
                native_source_frame_body_encode!(serializer, "file_chunk", { offset, bytes })
            }
            NativeSourceSnapshotExportFrameV1::FileEnd { source } => {
                native_source_frame_body_encode!(serializer, "file_end", { source })
            }
            NativeSourceSnapshotExportFrameV1::HeadComplete {
                repo_id,
                revision_id,
                file_count,
                file_census_sha256,
            } => native_source_frame_body_encode!(serializer, "head_complete", {
                repo_id, revision_id, file_count, file_census_sha256
            }),
            NativeSourceSnapshotExportFrameV1::RetiredProjectionIdentity {
                projection,
                instance,
            } => native_source_frame_body_encode!(serializer, "retired_projection_identity", {
                projection, instance
            }),
            NativeSourceSnapshotExportFrameV1::InventoryComplete {
                source_root_nonce,
                source_routes,
                head_count,
                route_head_count,
                retired_identity_count,
                file_census_sha256,
                transcript_sha256,
            } => native_source_frame_body_encode!(serializer, "inventory_complete", {
                source_root_nonce, source_routes, head_count, route_head_count,
                retired_identity_count, file_census_sha256, transcript_sha256
            }),
        }
    }
}

impl Serialize for NativeSourceSnapshotExportFrameV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let kind = match self {
            Self::HeadBegin { .. } => NativeSourceFrameKindV1::HeadBegin,
            Self::FileBegin { .. } => NativeSourceFrameKindV1::FileBegin,
            Self::FileChunk { .. } => NativeSourceFrameKindV1::FileChunk,
            Self::FileEnd { .. } => NativeSourceFrameKindV1::FileEnd,
            Self::HeadComplete { .. } => NativeSourceFrameKindV1::HeadComplete,
            Self::RetiredProjectionIdentity { .. } => {
                NativeSourceFrameKindV1::RetiredProjectionIdentity
            }
            Self::InventoryComplete { .. } => NativeSourceFrameKindV1::InventoryComplete,
        };
        let mut frame = serializer.serialize_struct("NativeSourceSnapshotExportFrameV1", 2)?;
        serde::ser::SerializeStruct::serialize_field(&mut frame, "kind", &kind)?;
        serde::ser::SerializeStruct::serialize_field(
            &mut frame,
            "body",
            &NativeSourceFrameBodyRefV1(self),
        )?;
        serde::ser::SerializeStruct::end(frame)
    }
}

struct NativeSourceFrameBodySeedV1(NativeSourceFrameKindV1);

impl<'de> serde::de::DeserializeSeed<'de> for NativeSourceFrameBodySeedV1 {
    type Value = NativeSourceSnapshotExportFrameV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        match self.0 {
            NativeSourceFrameKindV1::HeadBegin => native_source_frame_decode!(
                decoder,
                HeadBegin {
                    source_root_nonce: Digest,
                    source_routes: ObjectRef,
                    repo_id: RepoId,
                    revision_id: RevisionId,
                    source_head: NativeProtectedSourceHeadV1,
                    file_count: u64,
                }
            ),
            NativeSourceFrameKindV1::FileBegin => native_source_frame_decode!(decoder, FileBegin {
                source: SourceFileRevision,
                text_admitted: bool,
                source_bytes: u64,
                language: Option<String> = default,
            }),
            NativeSourceFrameKindV1::FileChunk => native_source_frame_decode!(decoder, FileChunk {
                offset: u64,
                bytes: Vec<u8>,
            }),
            NativeSourceFrameKindV1::FileEnd => native_source_frame_decode!(
                decoder,
                FileEnd {
                    source: SourceFileRevision,
                }
            ),
            NativeSourceFrameKindV1::HeadComplete => native_source_frame_decode!(
                decoder,
                HeadComplete {
                    repo_id: RepoId,
                    revision_id: RevisionId,
                    file_count: u64,
                    file_census_sha256: Digest,
                }
            ),
            NativeSourceFrameKindV1::RetiredProjectionIdentity => native_source_frame_decode!(
                decoder,
                RetiredProjectionIdentity {
                    projection: super::ProjectionId,
                    instance: super::IndexInstanceId,
                }
            ),
            NativeSourceFrameKindV1::InventoryComplete => native_source_frame_decode!(
                decoder,
                InventoryComplete {
                    source_root_nonce: Digest,
                    source_routes: ObjectRef,
                    head_count: u64,
                    route_head_count: u64,
                    retired_identity_count: u64,
                    file_census_sha256: Digest,
                    transcript_sha256: Digest,
                }
            ),
        }
    }
}

struct NativeSourceFrameVisitorV1(Option<NativeSourceFrameKindV1>);

impl<'de> serde::de::Visitor<'de> for NativeSourceFrameVisitorV1 {
    type Value = NativeSourceSnapshotExportFrameV1;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an adjacent-tag Native Source export frame")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        const FIELDS: &[&str] = &["kind", "body"];
        let mut kind = None;
        let mut body = None;
        let mut deferred_body = None;
        let mut body_seen = false;
        while let Some(key) = map.next_key_seed(NativeSourceFieldKeyV1(FIELDS))? {
            match key {
                "kind" => {
                    if kind.is_some() {
                        return Err(serde::de::Error::duplicate_field("kind"));
                    }
                    let actual = map.next_value::<NativeSourceFrameKindV1>()?;
                    if self.0.is_some_and(|expected| expected != actual) {
                        return Err(serde::de::Error::custom(
                            "Native Source frame kind changed between passes",
                        ));
                    }
                    kind = Some(actual);
                }
                "body" => {
                    if body_seen {
                        return Err(serde::de::Error::duplicate_field("body"));
                    }
                    body_seen = true;
                    if let Some(kind) = kind.or(self.0) {
                        body = Some(map.next_value_seed(NativeSourceFrameBodySeedV1(kind))?);
                    } else {
                        deferred_body = Some(map.next_value::<BodyReplay<'de>>()?);
                    }
                }
                _ => unreachable!("closed frame field seed only returns schema keys"),
            }
        }
        let kind = kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?;
        if let Some(value) = deferred_body {
            body = Some(
                serde::de::DeserializeSeed::deserialize(NativeSourceFrameBodySeedV1(kind), value)
                    .map_err(<A::Error as serde::de::Error>::custom)?,
            );
        }
        body.ok_or_else(|| serde::de::Error::missing_field("body"))
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let kind = seq
            .next_element::<NativeSourceFrameKindV1>()?
            .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
        if self.0.is_some_and(|expected| expected != kind) {
            return Err(serde::de::Error::custom(
                "Native Source frame kind changed between passes",
            ));
        }
        let body = seq
            .next_element_seed(NativeSourceFrameBodySeedV1(kind))?
            .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(serde::de::Error::invalid_length(3, &self));
        }
        Ok(body)
    }
}

impl<'de> Deserialize<'de> for NativeSourceSnapshotExportFrameV1 {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_struct(
            "NativeSourceSnapshotExportFrameV1",
            &["kind", "body"],
            NativeSourceFrameVisitorV1(None),
        )
    }
}

struct NativeSourceFrameKindReadV1;

impl NativeSourceFrameKindReadV1 {
    fn decode(bytes: &[u8]) -> Result<NativeSourceFrameKindV1, serde_json::Error> {
        struct KindReader;
        impl<'de> serde::de::DeserializeSeed<'de> for KindReader {
            type Value = NativeSourceFrameKindV1;

            fn deserialize<D: serde::Deserializer<'de>>(
                self,
                decoder: D,
            ) -> Result<Self::Value, D::Error> {
                struct KindVisitor;
                impl<'de> serde::de::Visitor<'de> for KindVisitor {
                    type Value = NativeSourceFrameKindV1;

                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str("an adjacent-tag Native Source export frame")
                    }

                    fn visit_map<A: serde::de::MapAccess<'de>>(
                        self,
                        mut map: A,
                    ) -> Result<Self::Value, A::Error> {
                        const FIELDS: &[&str] = &["kind", "body"];
                        let mut kind = None;
                        let mut body_seen = false;
                        while let Some(key) = map.next_key_seed(NativeSourceFieldKeyV1(FIELDS))? {
                            match key {
                                "kind" => {
                                    if kind.is_some() {
                                        return Err(serde::de::Error::duplicate_field("kind"));
                                    }
                                    kind = Some(map.next_value::<NativeSourceFrameKindV1>()?);
                                }
                                "body" => {
                                    if body_seen {
                                        return Err(serde::de::Error::duplicate_field("body"));
                                    }
                                    if kind.is_none() {
                                        let _: FrameJsonValueCheckV1 = map.next_value()?;
                                    } else {
                                        let _: serde::de::IgnoredAny = map.next_value()?;
                                    }
                                    body_seen = true;
                                }
                                _ => {
                                    unreachable!("closed frame field seed only returns schema keys")
                                }
                            }
                        }
                        let kind = kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?;
                        if !body_seen {
                            return Err(serde::de::Error::missing_field("body"));
                        }
                        Ok(kind)
                    }

                    fn visit_seq<A: serde::de::SeqAccess<'de>>(
                        self,
                        mut seq: A,
                    ) -> Result<Self::Value, A::Error> {
                        let kind = seq
                            .next_element::<NativeSourceFrameKindV1>()?
                            .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                        let _: serde::de::IgnoredAny = seq
                            .next_element()?
                            .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                            return Err(serde::de::Error::invalid_length(3, &self));
                        }
                        Ok(kind)
                    }
                }
                decoder.deserialize_struct(
                    "NativeSourceSnapshotExportFrameV1",
                    &["kind", "body"],
                    KindVisitor,
                )
            }
        }

        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let kind = serde::de::DeserializeSeed::deserialize(KindReader, &mut decoder)?;
        decoder.end()?;
        Ok(kind)
    }
}

impl NativeSourceSnapshotExportFrameV1 {
    /// Decode one already framed JSON payload without retaining an untyped body.
    /// The caller owns frame-length and work/memory admission before this call.
    pub fn decode_json_v1(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let kind = NativeSourceFrameKindReadV1::decode(bytes)?;
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let frame = serde::Deserializer::deserialize_struct(
            &mut decoder,
            "NativeSourceSnapshotExportFrameV1",
            &["kind", "body"],
            NativeSourceFrameVisitorV1(Some(kind)),
        )?;
        decoder.end()?;
        Ok(frame)
    }
}

#[cfg(test)]
mod frame_wire_tests {
    use super::*;

    #[test]
    fn file_chunk_adjacent_tag_json_order_and_kind_contract() {
        let frame = NativeSourceSnapshotExportFrameV1::FileChunk {
            offset: 2,
            bytes: vec![4, 5],
        };
        assert_eq!(
            serde_json::to_string(&frame).unwrap(),
            r#"{"kind":"file_chunk","body":{"offset":2,"bytes":[4,5]}}"#
        );
        assert_eq!(
            NativeSourceSnapshotExportFrameV1::decode_json_v1(
                br#"{"kind":"file_chunk","body":{"offset":2,"bytes":[4,5]}}"#
            )
            .unwrap(),
            frame
        );
        let body_first: NativeSourceSnapshotExportFrameV1 =
            serde_json::from_str(r#"{"body":{"offset":2,"bytes":[4,5]},"kind":"file_chunk"}"#)
                .unwrap();
        assert_eq!(body_first, frame);
        assert!(
            serde_json::from_str::<NativeSourceSnapshotExportFrameV1>(
                r#"{"kind":2,"body":{"offset":2,"bytes":[4,5]}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn file_chunk_body_first_cbor_accepts_byte_field_names() {
        // {"body": {h'6f6666736574': 2, h'6279746573': []}, "kind": "file_chunk"}
        const WIRE: &[u8] = b"\xa2\x64body\xa2\x46offset\x02\x45bytes\x80\x64kind\x6afile_chunk";
        let frame: NativeSourceSnapshotExportFrameV1 = ciborium::from_reader(WIRE).unwrap();
        assert_eq!(
            frame,
            NativeSourceSnapshotExportFrameV1::FileChunk {
                offset: 2,
                bytes: vec![],
            }
        );
    }

    #[test]
    fn file_chunk_outer_positional_json_accepts_a_map_body() {
        const WIRE: &str = r#"["file_chunk",{"offset":2,"bytes":[4,5]}]"#;
        let frame: NativeSourceSnapshotExportFrameV1 = serde_json::from_str(WIRE).unwrap();
        assert_eq!(
            frame,
            NativeSourceSnapshotExportFrameV1::FileChunk {
                offset: 2,
                bytes: vec![4, 5],
            }
        );
    }

    #[test]
    fn adjacent_tag_requires_a_closed_map_body_in_either_key_order() {
        for rejected in [
            r#"{"kind":"file_chunk","body":[2,[4,5]]}"#,
            r#"{"body":[2,[4,5]],"kind":"file_chunk"}"#,
            r#"{"body":{"offset":2,"offset":3,"bytes":[]},"kind":"file_chunk"}"#,
            r#"{"body":{"offset":2,"bytes":[],"extra":false},"kind":"file_chunk"}"#,
            r#"{"kind":"file_chunk","body":{"offset":2,"bytes":[]},"extra":false}"#,
        ] {
            assert!(serde_json::from_str::<NativeSourceSnapshotExportFrameV1>(rejected).is_err());
        }
        const CBOR_BODY_FIRST: &[u8] = b"\xa2\x64body\x82\x02\x80\x64kind\x6afile_chunk";
        const CBOR_OUTER_SEQ: &[u8] = b"\x82\x6afile_chunk\x82\x02\x80";
        for rejected in [CBOR_BODY_FIRST, CBOR_OUTER_SEQ] {
            assert!(
                ciborium::from_reader::<NativeSourceSnapshotExportFrameV1, _>(rejected).is_err()
            );
        }
    }

    #[test]
    fn body_first_file_begin_preserves_optional_language() {
        let source = SourceFileRevision {
            file: crate::SourceFileKey {
                source_repo_id: RepoId::new("repo").unwrap(),
                repo_relative_path: crate::RepoRelativePath::new("src/lib.rs"),
            },
            revision_id: RevisionId::new("rev").unwrap(),
            source_sha256: [2; 32],
        };
        let input = serde_json::to_string(&source).unwrap();
        let expected = NativeSourceSnapshotExportFrameV1::FileBegin {
            source,
            text_admitted: true,
            source_bytes: 4,
            language: None,
        };
        for language in ["", ",\"language\":null"] {
            let wire = format!(
                "{{\"body\":{{\"source\":{input},\"text_admitted\":true,\"source_bytes\":4{language}}},\"kind\":\"file_begin\"}}"
            );
            let actual = serde_json::from_str::<NativeSourceSnapshotExportFrameV1>(&wire).unwrap();
            assert_eq!(actual, expected);
            assert_eq!(
                NativeSourceSnapshotExportFrameV1::decode_json_v1(wire.as_bytes()).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn body_first_json_decodes_without_replay_and_rejects_malformed_frames() {
        use crate::index::frame_body_replay::BODY_REPLAY_ENTRY_COUNT;

        let mut wire = Vec::with_capacity(2 * 64 * 1024 + 96);
        wire.extend_from_slice(br#"{"body":{"offset":0,"bytes":["#);
        for index in 0..64 * 1024 {
            if index != 0 {
                wire.push(b',');
            }
            wire.push(b'0');
        }
        wire.extend_from_slice(br#"]},"kind":"file_chunk"}"#);

        BODY_REPLAY_ENTRY_COUNT.with(|count| count.set(0));
        let frame = NativeSourceSnapshotExportFrameV1::decode_json_v1(&wire).unwrap();
        assert!(
            matches!(frame, NativeSourceSnapshotExportFrameV1::FileChunk { offset: 0, bytes } if bytes.len() == 64 * 1024)
        );
        BODY_REPLAY_ENTRY_COUNT.with(|count| assert_eq!(count.get(), 0));
        let _: NativeSourceSnapshotExportFrameV1 =
            serde_json::from_slice(br#"{"body":{"offset":0,"bytes":[]},"kind":"file_chunk"}"#)
                .unwrap();
        BODY_REPLAY_ENTRY_COUNT.with(|count| assert!(count.get() > 0));

        assert_eq!(
            NativeSourceSnapshotExportFrameV1::decode_json_v1(
                br#"["file_chunk",{"offset":2,"bytes":[4,5]}]"#
            )
            .unwrap(),
            NativeSourceSnapshotExportFrameV1::FileChunk {
                offset: 2,
                bytes: vec![4, 5],
            }
        );

        for malformed in [
            &br#"{"body":{"offset":0,"bytes":[]},"kind":"file_chunk","kind":"file_end"}"#[..],
            br#"{"body":{"offset":0,"offset":1,"bytes":[]},"kind":"file_chunk"}"#,
            br#"{"body":{"offset":0,"bytes":[],"extra":1},"kind":"file_chunk"}"#,
            br#"{"body":{"offset":0,"bytes":[]},"kind":"file_chunk","extra":1}"#,
            br#"{"body":{"offset":0,"bytes":[},"kind":"file_chunk"}"#,
            br#"{"body":{"offset":0,"bytes":[]},"kind":"file_chunk"}{}"#,
            br#"{"body":{"offset":0,"bytes":[]},"kind":2}"#,
            br#"{"kind":"file_chunk","body":[0,[]]}"#,
            br#"{"body":[0,[]],"kind":"file_chunk"}"#,
            br#"["file_chunk",[0,[]]]"#,
        ] {
            assert!(NativeSourceSnapshotExportFrameV1::decode_json_v1(malformed).is_err());
        }
    }
}
