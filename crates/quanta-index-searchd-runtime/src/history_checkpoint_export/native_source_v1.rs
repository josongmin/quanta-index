//! Source-only export from a selected Native predecessor. RepoMap objects are
//! authenticated in the old Catalog but never emitted as candidate payload.

use std::io::{self, Write};

use anyhow::{Result, ensure};
use quanta_index_contract::index::{
    Digest, MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1, NativeSourceSnapshotExportFrameV1 as Frame,
    RetiredProjectionIdentityV1, SourceTextAdmission,
};
use quanta_index_contract::{RepoRelativePath, RevisionId, SourceFileKey, SourceFileRevision};
use quanta_index_core::index::{
    IndexError, MapPageEntry, MapValue, MemoryLifetime, SourceRecordOwner, WorkBudget,
    owner_from_map_key,
};
use quanta_index_storage::FileObjectReader;
use sha2::{Digest as _, Sha256};

use super::{boot, write_frame};
use super::boot::retired_policy_profile_v1::old_repomap_identity_v1;
use crate::deployment_selection::DeploymentSelectionDocumentV1;

struct TranscriptWriter<'a, W> {
    output: &'a mut W,
    digest: Sha256,
}

impl<W: Write> Write for TranscriptWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.output.write(bytes)?;
        self.digest.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

fn frame(output: &mut impl Write, value: &Frame, budget: &mut WorkBudget) -> Result<()> {
    write_frame(output, value, budget)
}

fn frame_index(
    output: &mut impl Write,
    value: &Frame,
    budget: &mut WorkBudget,
) -> std::result::Result<(), IndexError> {
    frame(output, value, budget).map_err(|cause| {
        cause
            .downcast::<IndexError>()
            .unwrap_or_else(|cause| IndexError::Storage(format!("Native Source frame: {cause}")))
    })
}

fn hash_text(hash: &mut Sha256, text: &str) -> std::result::Result<(), IndexError> {
    let bytes = text.as_bytes();
    hash.update(
        u32::try_from(bytes.len())
            .map_err(|_| IndexError::ResourceLimited)?
            .to_le_bytes(),
    );
    hash.update(bytes);
    Ok(())
}

fn hex_revision(bytes: &[u8; 32]) -> std::result::Result<RevisionId, IndexError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut id = String::from("native-source-sha256:");
    for byte in bytes {
        id.push(char::from(HEX[usize::from(byte >> 4)]));
        id.push(char::from(HEX[usize::from(byte & 15)]));
    }
    RevisionId::new(id).map_err(|cause| {
        IndexError::InvalidInput(format!("Native Source file identity: {cause}").into())
    })
}

fn export_head_files_v1(
    head: &quanta_index_contract::index::NativeProtectedSourceHeadV1,
    reader: &FileObjectReader,
    output: &mut impl Write,
    budget: &mut WorkBudget,
) -> std::result::Result<(u64, Digest), IndexError> {
    let cut = &head.source_publication.source;
    let expected = reader.source_file_count_v1(cut, budget)?;
    let mut digest = Sha256::new();
    digest.update(b"quanta-index/native-source-file-census/v1\0");
    let mut after: Option<MapPageEntry> = None;
    let mut count = 0_u64;
    loop {
        let page = reader.scan_map(
            cut.owners,
            after.as_ref().map(|entry| entry.key()).transpose()?,
            128,
            budget,
        )?;
        for (key, value) in &page.entries {
            let owner = owner_from_map_key(key)?;
            let MapValue::Object(reference) = value else {
                return Err(IndexError::DigestConflict);
            };
            let stored = reader.get(*reference, budget)?;
            let record =
                SourceRecordOwner::decode_json(&stored.payload, budget, MemoryLifetime::Temporary)?;
            let source_repo = record.source_repo_id.as_ref().ok_or(IndexError::DigestConflict)?;
            if stored.registration.children.as_slice() != [record.bytes] {
                return Err(IndexError::DigestConflict);
            }
            reader.with_repository_source_file_v1(
                cut,
                source_repo,
                &record.path,
                budget,
                |actual_owner, original, bytes, budget| {
                    if actual_owner != owner || original != &*record {
                        return Err(IndexError::DigestConflict);
                    }
                    // Reserve for the identity copies and JSON escaping before
                    // constructing the SourceFileRevision and its census row.
                    let language = record.language.as_ref().map(|code| code.as_str());
                    let identity_bytes = source_repo.as_str().len()
                        .checked_add(record.path.len())
                        .and_then(|length| length.checked_add(language.map_or(0, str::len)))
                        .and_then(|length| length.checked_mul(8))
                        .and_then(|length| length.checked_add(4096))
                        .ok_or(IndexError::ResourceLimited)?;
                    let identity_limit = u64::try_from(identity_bytes)
                        .map_err(|_| IndexError::ResourceLimited)?;
                    let _identity_memory = budget
                        .reserve_memory(identity_limit, MemoryLifetime::Temporary)?
                        .ok_or(IndexError::ResourceLimited)?;
                    let source_sha256: [u8; 32] = Sha256::digest(bytes).into();
                    let source = SourceFileRevision {
                        file: SourceFileKey {
                            source_repo_id: source_repo.clone(),
                            repo_relative_path: RepoRelativePath::new(record.path.clone()),
                        },
                        revision_id: hex_revision(&source_sha256)?,
                        source_sha256,
                    };
                    source
                        .validate()
                        .map_err(|cause| IndexError::InvalidInput(cause.into()))?;
                    let text_admitted = matches!(record.text, SourceTextAdmission::Utf8 { .. });
                    let source_bytes =
                        u64::try_from(bytes.len()).map_err(|_| IndexError::ResourceLimited)?;
                    let encoded = serde_json::to_vec(&source)
                        .map_err(|cause| IndexError::Storage(cause.to_string()))?;
                    if encoded.len() > identity_bytes {
                        return Err(IndexError::ResourceLimited);
                    }
                    digest.update(
                        u32::try_from(encoded.len())
                            .map_err(|_| IndexError::ResourceLimited)?
                            .to_le_bytes(),
                    );
                    digest.update(&encoded);
                    digest.update([u8::from(text_admitted)]);
                    digest.update(source_bytes.to_le_bytes());
                    if let Some(language) = language {
                        digest.update(u32::try_from(language.len()).map_err(|_| IndexError::ResourceLimited)?.to_le_bytes());
                        digest.update(language.as_bytes());
                    } else {
                        digest.update(u32::MAX.to_le_bytes());
                    }
                    frame_index(
                        output,
                        &Frame::FileBegin {
                            source: source.clone(),
                            text_admitted,
                            source_bytes,
                            language: language.map(str::to_owned),
                        },
                        budget,
                    )?;
                    for (ordinal, chunk) in
                        bytes.chunks(MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1).enumerate()
                    {
                        let chunk_bytes = u64::try_from(chunk.len())
                            .map_err(|_| IndexError::ResourceLimited)?;
                        let _chunk_memory = budget
                            .reserve_memory(chunk_bytes, MemoryLifetime::Temporary)?
                            .ok_or(IndexError::ResourceLimited)?;
                        let mut owned_chunk = Vec::new();
                        owned_chunk.try_reserve_exact(chunk.len())
                            .map_err(|_| IndexError::ResourceLimited)?;
                        if owned_chunk.capacity() > chunk.len() {
                            return Err(IndexError::ResourceLimited);
                        }
                        owned_chunk.extend_from_slice(chunk);
                        let offset = ordinal
                            .checked_mul(MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1)
                            .ok_or(IndexError::ResourceLimited)?;
                        frame_index(
                            output,
                            &Frame::FileChunk {
                                offset: u64::try_from(offset)
                                    .map_err(|_| IndexError::ResourceLimited)?,
                                bytes: owned_chunk,
                            },
                            budget,
                        )?;
                    }
                    frame_index(output, &Frame::FileEnd { source }, budget)
                },
            )?;
            count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        if !page.has_more {
            break;
        }
        let next = page.cursor().last().ok_or(IndexError::ResourceLimited)?;
        if after
            .as_ref()
            .map(|entry| entry.key())
            .transpose()?
            .is_some_and(|prior| next.key().is_ok_and(|key| key <= prior))
        {
            return Err(IndexError::DigestConflict);
        }
        after = Some(next);
    }
    if count != expected {
        return Err(IndexError::DigestConflict);
    }
    Ok((count, Digest(digest.finalize().into())))
}

pub(super) fn export_v1(
    selection: &DeploymentSelectionDocumentV1,
    output: &mut impl Write,
    budget: &mut WorkBudget,
) -> Result<()> {
    boot::with_selected_native_history_v1(selection, budget, |held, budget| {
        let observed = held.observed_v1();
        let selected = observed
            .snapshot
            .selected
            .as_ref()
            .ok_or(IndexError::SnapshotUnavailable)?;
        let route = selected.protected_source_routes;
        let mut transcript = TranscriptWriter {
            output,
            digest: {
                let mut digest = Sha256::new();
                digest.update(b"quanta-index/native-source-transcript/v1\0");
                digest
            },
        };
        let mut inventory = Sha256::new();
        inventory.update(b"quanta-index/native-source-inventory/v1\0");
        let mut count = 0_u64;
        let mut retired_count = 0_u64;
        if let Some(route) = route {
            held.with_protected_routes_v1(budget, |routes, reader, budget| {
                let witness = quanta_index_catalog::read_native_protected_snapshot_witness_v1(
                    &held.selected_config_v1().state_root,
                    held.catalog_io_v1(),
                    selected,
                    routes,
                    budget,
                )?;
                for reference in witness.object_references_v1() {
                    reader.verify_object(reference, budget)?;
                }
                for head in &routes.heads {
                    let file_count =
                        reader.source_file_count_v1(&head.source_publication.source, budget)?;
                    frame(
                        &mut transcript,
                        &Frame::HeadBegin {
                            source_root_nonce: Digest(observed.snapshot.root_nonce),
                            source_routes: route.object,
                            repo_id: head.repo_id.clone(),
                            revision_id: head.revision_id.clone(),
                            source_head: head.clone(),
                            file_count,
                        },
                        budget,
                    )?;
                    let (actual_count, census) =
                        export_head_files_v1(head, reader, &mut transcript, budget)?;
                    if actual_count != file_count {
                        return Err(IndexError::DigestConflict.into());
                    }
                    frame(
                        &mut transcript,
                        &Frame::HeadComplete {
                            repo_id: head.repo_id.clone(),
                            revision_id: head.revision_id.clone(),
                            file_count,
                            file_census_sha256: census,
                        },
                        budget,
                    )?;
                    hash_text(&mut inventory, head.repo_id.as_str())?;
                    hash_text(&mut inventory, head.revision_id.as_str())?;
                    inventory.update(file_count.to_le_bytes());
                    inventory.update(census.0);
                    count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                }
                let retired_len = routes.repomap_heads.len();
                let retired_bytes = retired_len
                    .checked_mul(std::mem::size_of::<RetiredProjectionIdentityV1>())
                    .and_then(|size| size.checked_add(retired_len.checked_mul(16)?))
                    .and_then(|size| u64::try_from(size).ok())
                    .ok_or(IndexError::ResourceLimited)?;
                let _retired_memory = budget
                    .reserve_memory(retired_bytes.max(1), MemoryLifetime::Temporary)?
                    .ok_or(IndexError::ResourceLimited)?;
                let mut identities = Vec::new();
                identities
                    .try_reserve_exact(retired_len)
                    .map_err(|_| IndexError::ResourceLimited)?;
                if identities.capacity() > retired_len {
                    return Err(IndexError::ResourceLimited.into());
                }
                for head in &routes.repomap_heads {
                    budget.checkpoint()?;
                    identities.push(old_repomap_identity_v1(head, reader, budget)?);
                }
                identities.sort_unstable_by_key(|identity| identity.projection.0);
                if identities.windows(2).any(|pair| pair[0].projection == pair[1].projection
                    && pair[0].instance != pair[1].instance) {
                    return Err(IndexError::DigestConflict.into());
                }
                identities.dedup();
                let mut instances = Vec::new();
                instances
                    .try_reserve_exact(identities.len())
                    .map_err(|_| IndexError::ResourceLimited)?;
                if instances.capacity() > identities.len() {
                    return Err(IndexError::ResourceLimited.into());
                }
                instances.extend(identities.iter().map(|identity| identity.instance));
                instances.sort_unstable_by_key(|instance| instance.0);
                if instances.windows(2).any(|pair| pair[0] == pair[1]) {
                    return Err(IndexError::DigestConflict.into());
                }
                for identity in identities {
                    budget.checkpoint()?;
                    frame(&mut transcript, &Frame::RetiredProjectionIdentity {
                        projection: identity.projection,
                        instance: identity.instance,
                    }, budget)?;
                    retired_count = retired_count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                }
                Ok(())
            })?;
        }
        let transcript_sha256 = Digest(transcript.digest.finalize().into());
        let file_census_sha256 = Digest(inventory.finalize().into());
        frame(
            transcript.output,
            &Frame::InventoryComplete {
                source_root_nonce: Digest(observed.snapshot.root_nonce),
                source_routes: route
                    .map(|route| route.object)
                    .unwrap_or(quanta_index_contract::index::ObjectRef(Digest([0; 32]))),
                head_count: count,
                route_head_count: u64::try_from(route.map_or(0, |route| route.count))
                    .map_err(|_| IndexError::ResourceLimited)?,
                retired_identity_count: retired_count,
                file_census_sha256,
                transcript_sha256,
            },
            budget,
        )?;
        ensure!(
            count == u64::try_from(route.map_or(0, |route| route.count))?,
            "Native Source route count changed"
        );
        Ok(())
    })
}
