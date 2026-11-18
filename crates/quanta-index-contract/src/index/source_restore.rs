//! Private, provisional export of one physically sealed Legacy Source census.
//! Frames do not authorize a Native publication. The receiver must compare the
//! complete inventory, each original file and the applied Catalog ACK against
//! the held Legacy source before it stages a replacement.

use super::{CatalogView, Digest, ImmutableOperation, OperationKey, UpdateRequest, ViewRef};
use crate::{
    BatchPublishReceipt, ManifestGeneration, RepoId, RevisionId, SearchCorpusActiveHeadV1, SourceFileRevision,
    SourcePublicationEvent,
    native_cutover_evidence_v1::{
        NativeCutoverEvidenceSourcePublicationBindingV1, NativeCutoverEvidenceTargetV1,
    },
    repomap::RepoMapExpectedActiveV2,
};
use sha2::{Digest as _, Sha256};
use std::io::{self, Write};

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
#[allow(clippy::too_many_arguments)]
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

fn digest64_lower_v1(digest: Digest) -> crate::native_cutover_evidence_v1::NativeCutoverDigest64V1 {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 64];
    for (index, byte) in digest.0.iter().enumerate() {
        bytes[2 * index] = HEX[usize::from(byte >> 4)];
        bytes[2 * index + 1] = HEX[usize::from(byte & 15)];
    }
    crate::native_cutover_evidence_v1::NativeCutoverDigest64V1(bytes)
}

/// Exact source-head map root decoded from the committed lexical file authority.
/// The committed file-authority artifact and generation seal authenticate this
/// descriptor; its hash is never derived from a streamed file list.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LexicalSourceHeadsCommitmentV1 {
    pub root_sha256: Option<[u8; 32]>,
    pub entries: u64,
}

impl LexicalSourceHeadsCommitmentV1 {
    #[must_use]
    pub fn is_canonical_v1(&self) -> bool {
        self.root_sha256.is_some() == (self.entries != 0)
    }
}

/// Bounded private child stdout stream. A HeadBegin and its file frames are
/// provisional until HeadComplete; all heads are provisional until InventoryComplete.
/// A file body is emitted in <=64 KiB chunks without retaining the whole corpus.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
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

pub const MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1: usize = 64 * 1024;

/// Source identity issued only after a producer Original has completed the
/// entire physically exported Legacy inventory. This DTO is not an issuer or
/// a substitute for the FD6 retained Source fact that carries it.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LegacySourceOriginV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub active: SearchCorpusActiveHeadV1,
    pub applied_ack: BatchPublishReceipt,
    pub source_publication_event: SourcePublicationEvent,
    pub source_heads: LexicalSourceHeadsCommitmentV1,
}

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
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
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
        LegacySourceManifestDigestV1, LexicalSourceHeadsCommitmentV1,
        imported_source_publication_scope_v1,
    };
    use crate::index::{
        CollectionId, Digest, OperationKey, ProducerId, SlotId, StreamId, TenantId, ViewRef,
        WorkspaceKey, WorktreeId,
    };
    use crate::{RepoId, RepoRelativePath, RevisionId, SourceFileKey, SourceFileRevision};
    use sha2::{Digest as _, Sha256};

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
