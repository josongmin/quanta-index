//! Provisional Native Source-only export frames. The selected predecessor
//! lease and physical Catalog, not these DTOs, establish source authority.

use super::{
    CatalogView, Digest, ImmutableOperation, NativeProtectedSourceHeadV1, ObjectRef, UpdateRequest,
};
use crate::native_cutover_evidence_v1::{
    NativeCutoverEvidenceSourcePublicationBindingV1, NativeCutoverEvidenceTargetV1,
    canonical_digest64,
};
use crate::{RepoId, RevisionId, SourceFileRevision};
use quanta_index_contract_base::NativeCutoverDigest64V1;
use serde::Serialize;
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
            && self.old_routes_ref.0 .0 != [0; 32]
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
            && self.origin.old_routes_ref.0 .0 != [0; 32]
            && self.origin.old_source_head_sha256.0 != [0; 32]
            && self.origin.file_census_sha256.0 != [0; 32]
            && self.origin.accepted_transcript_sha256.0 != [0; 32]
            && self.candidate_root_nonce.0 != [0; 32]
            && self.source_publication_sha256.0 != [0; 32]
            && self.completed_journal_digest.0 != [0; 32]
            && self.original_request_sha256.0 != [0; 32]
            && self.source_operation.key == target.operation
            && self.original_snapshot_id != 0
            && self.source_binding.target_snapshot_id == self.original_snapshot_id
            && canonical_digest64(&self.source_binding.transition_digest)
            && self.source_binding.completion_pointer_digest
                == digest64_lower_v1(self.completed_journal_digest)
            && self.new_source_manifest_sha256
                == native_source_target_manifest_digest_v1(self.origin.file_census_sha256)
            && self.source_binding.physical_target_source_digest
                == digest64_lower_v1(self.new_source_manifest_sha256)
            && json_sha256_v1(request) == Some(self.original_request_sha256)
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

mod native_source_frame_codec_v1;
