//! Existing-only Source body loan through the caller's held physical read port.
//! This reuses the canonical map, object, blob and packed-record decoders.

use super::{
    blob_codec::{BLOCK_BYTES, RecordGroupIndex, decode_blob_node, packed_dependencies_valid},
    map_codec::{Page, decode_page},
    object_codec::{BlobRef, ObjectKind, ObjectRef, PageRef},
    source_head::{SourceHead, source_key_digest},
};
use super::{
    coverage_root, file_root::AuthorityRoot, manifest::LexicalSealedManifest, object_binary,
};
use quanta_index_contract::{
    GenerationSnapshot, SourceFileRevision, SourcePublicationEvent,
    index::LexicalSourceHeadsCommitmentV1,
};
use quanta_index_core::{
    ExistingTreeMetadataIoPortV1,
    index::{
        IndexError, MemoryLease, MemoryLifetime, ObjectReadIoPort, WorkBudget,
        existing_file::{
            ExistingFileReadFailureV1 as Failure, read_existing_committed_file_v1,
            read_existing_file_v1,
        },
    },
};
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum SourceVisitFailureV1<E> {
    Physical(Failure),
    Visitor(E),
}

impl<E: std::fmt::Display> std::fmt::Display for SourceVisitFailureV1<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Physical(cause) => write!(f, "physical Source read refused: {cause}"),
            Self::Visitor(cause) => write!(f, "Source visitor refused: {cause}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for SourceVisitFailureV1<E> {}

struct PhysicalObject {
    dependencies: Vec<ObjectRef>,
    payload: Vec<u8>,
    _memory: MemoryLease,
}

struct ExistingSourceReader<'a> {
    pool: PathBuf,
    owner: u32,
    io: &'a dyn ObjectReadIoPort,
}

impl ExistingSourceReader<'_> {
    fn object(
        &self,
        reference: &ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<PhysicalObject, Failure> {
        if reference.bytes > object_binary::MAX_OBJECT_BYTES as u64 {
            return Err(IndexError::DigestConflict.into());
        }
        let hex = object_binary::hex_digest(&reference.sha256);
        let prefix = hex.get(..2).ok_or(IndexError::DigestConflict)?;
        let artifact = quanta_index_core::SealedArtifactCommitmentV1 {
            name: format!("{prefix}/{hex}"),
            bytes: reference.bytes,
            sha256: reference.sha256,
        };
        let memory_bytes = reference
            .bytes
            .checked_mul(4)
            .and_then(|n| n.checked_add(8192))
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget
            .reserve_memory(memory_bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        let input =
            read_existing_committed_file_v1(&self.pool, &artifact, self.owner, self.io, budget)?;
        let (kind, dependencies, payload) = object_binary::decode_object(input.bytes_v1())?;
        if kind != reference.kind {
            return Err(IndexError::DigestConflict.into());
        }
        Ok(PhysicalObject {
            dependencies,
            payload,
            _memory: memory,
        })
    }

    fn blob_node(
        &self,
        reference: &ObjectRef,
        logical: Option<(u64, &[u8; 32])>,
        depth: u8,
        output: &mut Vec<u8>,
        maximum: usize,
        budget: &mut WorkBudget,
    ) -> Result<(), Failure> {
        if depth > 16 {
            return Err(IndexError::DigestConflict.into());
        }
        let object = self.object(reference, budget)?;
        if logical.is_none() && reference.kind == ObjectKind::BlobBlock {
            if !object.dependencies.is_empty()
                || object.payload.is_empty()
                || object.payload.len() > BLOCK_BYTES
                || output
                    .len()
                    .checked_add(object.payload.len())
                    .is_none_or(|n| n > maximum)
            {
                return Err(IndexError::DigestConflict.into());
            }
            output.extend_from_slice(&object.payload);
            return Ok(());
        }
        if !(logical.is_some() && reference.kind.is_payload()
            || logical.is_none() && reference.kind == ObjectKind::BlobNode)
        {
            return Err(IndexError::DigestConflict.into());
        }
        let (&tag, rest) = object
            .payload
            .split_first()
            .ok_or(IndexError::DigestConflict)?;
        match tag {
            0 if logical.is_some() && rest.len() <= BLOCK_BYTES => {
                if output
                    .len()
                    .checked_add(rest.len())
                    .is_none_or(|n| n > maximum)
                {
                    return Err(IndexError::DigestConflict.into());
                }
                output.extend_from_slice(rest);
            }
            1 => {
                if logical.is_some() && maximum <= BLOCK_BYTES {
                    return Err(IndexError::DigestConflict.into());
                }
                let children = decode_blob_node(rest)?;
                if children
                    .iter()
                    .any(|(child, _)| !object.dependencies.contains(child))
                    || logical.is_none()
                        && object
                            .dependencies
                            .iter()
                            .any(|dep| !children.iter().any(|(child, _)| child == dep))
                {
                    return Err(IndexError::DigestConflict.into());
                }
                for (child, length) in children {
                    let before = output.len();
                    self.blob_node(&child, None, depth + 1, output, maximum, budget)?;
                    if u64::try_from(output.len().saturating_sub(before)).ok() != Some(length) {
                        return Err(IndexError::DigestConflict.into());
                    }
                }
            }
            2 if logical.is_some() => {
                if !packed_dependencies_valid(reference.kind, &object.dependencies) {
                    return Err(IndexError::DigestConflict.into());
                }
                let (expected_len, digest) = logical.ok_or(IndexError::DigestConflict)?;
                let index = RecordGroupIndex::decode(rest)?;
                let body = index.select(rest, expected_len, digest)?;
                if output
                    .len()
                    .checked_add(body.len())
                    .is_none_or(|n| n > maximum)
                {
                    return Err(IndexError::DigestConflict.into());
                }
                output.extend_from_slice(body);
            }
            _ => return Err(IndexError::DigestConflict.into()),
        }
        Ok(())
    }

    fn blob(
        &self,
        reference: &BlobRef,
        maximum: usize,
        budget: &mut WorkBudget,
    ) -> Result<(Vec<u8>, MemoryLease), Failure> {
        let length = usize::try_from(reference.bytes).map_err(|_| IndexError::ResourceLimited)?;
        if length > maximum || !reference.root.kind.is_payload() {
            return Err(IndexError::DigestConflict.into());
        }
        let memory_bytes = reference
            .bytes
            .checked_mul(2)
            .and_then(|n| n.checked_add(8192))
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget
            .reserve_memory(memory_bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(length)
            .map_err(|_| IndexError::ResourceLimited)?;
        self.blob_node(
            &reference.root,
            Some((reference.bytes, &reference.sha256)),
            0,
            &mut output,
            length,
            budget,
        )?;
        if output.len() != length || <[u8; 32]>::from(Sha256::digest(&output)) != reference.sha256 {
            return Err(IndexError::DigestConflict.into());
        }
        Ok((output, memory))
    }

    fn visit_page<E>(
        &self,
        reference: &PageRef,
        previous: &mut Option<Vec<u8>>,
        count: &mut u64,
        budget: &mut WorkBudget,
        visit: &mut dyn FnMut(&SourceFileRevision, bool, &[u8], &mut WorkBudget) -> Result<(), E>,
    ) -> Result<(), SourceVisitFailureV1<E>> {
        let object = self
            .object(&reference.object, budget)
            .map_err(SourceVisitFailureV1::Physical)?;
        let page = decode_page(&object.payload, reference, &object.dependencies)
            .map_err(|cause| SourceVisitFailureV1::Physical(cause.into()))?;
        match page {
            Page::Branch(children) => {
                for child in children {
                    self.visit_page(&child, previous, count, budget, visit)?;
                }
            }
            Page::Leaf(rows) => {
                for (key, value) in rows {
                    if previous.as_ref().is_some_and(|prior| prior >= &key) {
                        return Err(SourceVisitFailureV1::Physical(
                            IndexError::DigestConflict.into(),
                        ));
                    }
                    let (record, _record_memory) = self
                        .blob(&value, super::source_head::MAX_RECORD_BYTES, budget)
                        .map_err(SourceVisitFailureV1::Physical)?;
                    let head = SourceHead::decode(&record)
                        .map_err(|cause| SourceVisitFailureV1::Physical(cause.into()))?;
                    let digest = source_key_digest(&head.coverage.source.file)
                        .map_err(|cause| SourceVisitFailureV1::Physical(cause.into()))?;
                    if key.as_slice() != digest {
                        return Err(SourceVisitFailureV1::Physical(
                            IndexError::DigestConflict.into(),
                        ));
                    }
                    let (body, _body_memory) = self
                        .blob(&head.body, super::source_head::MAX_FILE_BYTES, budget)
                        .map_err(SourceVisitFailureV1::Physical)?;
                    if u64::try_from(body.len()).ok() != Some(head.source_bytes)
                        || <[u8; 32]>::from(Sha256::digest(&body))
                            != head.coverage.source.source_sha256
                    {
                        return Err(SourceVisitFailureV1::Physical(
                            IndexError::DigestConflict.into(),
                        ));
                    }
                    if head.coverage.text_admitted && std::str::from_utf8(&body).is_err() {
                        return Err(SourceVisitFailureV1::Physical(
                            IndexError::DigestConflict.into(),
                        ));
                    }
                    visit(
                        &head.coverage.source,
                        head.coverage.text_admitted,
                        &body,
                        budget,
                    )
                    .map_err(SourceVisitFailureV1::Visitor)?;
                    *count = count.checked_add(1).ok_or_else(|| {
                        SourceVisitFailureV1::Physical(IndexError::ResourceLimited.into())
                    })?;
                    *previous = Some(key);
                }
            }
        }
        Ok(())
    }
}

/// Lend every original Source file from one sealed generation through the same
/// held physical I/O port used by the canonical complete-generation verifier.
/// Header and file callbacks are provisional until this function returns Ok.
pub fn visit_existing_complete_source_files_v1<E>(
    lexical_track_root: &Path,
    expected: &GenerationSnapshot,
    owner: u32,
    maximum_objects: usize,
    metadata: &dyn ExistingTreeMetadataIoPortV1,
    io: &dyn ObjectReadIoPort,
    budget: &mut WorkBudget,
    begin: &mut dyn FnMut(
        &SourcePublicationEvent,
        LexicalSourceHeadsCommitmentV1,
        &mut WorkBudget,
    ) -> Result<(), E>,
    visit: &mut dyn FnMut(&SourceFileRevision, bool, &[u8], &mut WorkBudget) -> Result<(), E>,
) -> Result<u64, SourceVisitFailureV1<E>> {
    super::existing::verify_existing_lexical_generation_read_only(
        lexical_track_root,
        expected,
        owner,
        maximum_objects,
        metadata,
        io,
        budget,
    )
    .map_err(SourceVisitFailureV1::Physical)?;
    let generation = quanta_index_core::GenerationStorageKeyV1::for_repo_revision(
        &expected.repo_id,
        &expected.revision_id,
    )
    .generation_dir(lexical_track_root, expected.manifest_generation);
    let manifest_path = generation.join(super::manifest::LEXICAL_SEALED_MANIFEST_FILE_NAME);
    let manifest_input = read_existing_file_v1(
        &manifest_path,
        owner,
        super::manifest::MAX_SEALED_MANIFEST_BYTES as u64,
        io,
        budget,
    )
    .map_err(SourceVisitFailureV1::Physical)?;
    let _manifest_decode = super::existing::decoder_memory(manifest_input.bytes_v1().len(), budget)
        .map_err(SourceVisitFailureV1::Physical)?;
    let manifest = LexicalSealedManifest::decode(manifest_input.bytes_v1(), &manifest_path)
        .map_err(|cause| SourceVisitFailureV1::Physical(cause.into()))?;
    if manifest.manifest_digest != expected.manifest_digest {
        return Err(SourceVisitFailureV1::Physical(
            IndexError::DigestConflict.into(),
        ));
    }
    let file_artifact = manifest
        .file_authority
        .first()
        .cloned()
        .ok_or_else(|| SourceVisitFailureV1::Physical(IndexError::DigestConflict.into()))?;
    let coverage_artifact = manifest
        .source_coverage
        .clone()
        .ok_or_else(|| SourceVisitFailureV1::Physical(IndexError::DigestConflict.into()))?;
    drop(manifest);
    drop(_manifest_decode);
    drop(manifest_input);
    let file = read_existing_committed_file_v1(&generation, &file_artifact, owner, io, budget)
        .map_err(SourceVisitFailureV1::Physical)?;
    let _file_root_decode = super::existing::decoder_memory(file.bytes_v1().len(), budget)
        .map_err(SourceVisitFailureV1::Physical)?;
    let root = AuthorityRoot::decode(file.bytes_v1())
        .map_err(|cause| SourceVisitFailureV1::Physical(cause.into()))?;
    let coverage =
        read_existing_committed_file_v1(&generation, &coverage_artifact, owner, io, budget)
            .map_err(SourceVisitFailureV1::Physical)?;
    let _coverage_decode = super::existing::decoder_memory(coverage.bytes_v1().len(), budget)
        .map_err(SourceVisitFailureV1::Physical)?;
    let ((_, observed, event, pages), _heap_charge) =
        coverage_root::decode_root(coverage.bytes_v1(), &generation)
            .map_err(|cause| SourceVisitFailureV1::Physical(cause.into()))?;
    let coverage_count = pages.iter().try_fold(0_u64, |total, (_, _, _, rows)| {
        total.checked_add(u64::from(*rows))
    });
    if observed != *expected
        || root.source_heads.entries != root.live_sources
        || coverage_count != Some(root.live_sources)
    {
        return Err(SourceVisitFailureV1::Physical(
            IndexError::DigestConflict.into(),
        ));
    }
    let commitment = LexicalSourceHeadsCommitmentV1 {
        root_sha256: root
            .source_heads
            .page
            .as_ref()
            .map(|page| page.object.sha256),
        entries: root.source_heads.entries,
    };
    if !commitment.is_canonical_v1() {
        return Err(SourceVisitFailureV1::Physical(
            IndexError::DigestConflict.into(),
        ));
    }
    begin(&event, commitment, budget).map_err(SourceVisitFailureV1::Visitor)?;
    let reader = ExistingSourceReader {
        pool: lexical_track_root
            .join(object_binary::POOL_DIR)
            .join(object_binary::OBJECTS_DIR),
        owner,
        io,
    };
    let mut previous = None;
    let mut count = 0_u64;
    if let Some(page) = root.source_heads.page.as_ref() {
        reader.visit_page(page, &mut previous, &mut count, budget, visit)?;
    }
    if count != root.live_sources {
        return Err(SourceVisitFailureV1::Physical(
            IndexError::DigestConflict.into(),
        ));
    }
    Ok(count)
}
