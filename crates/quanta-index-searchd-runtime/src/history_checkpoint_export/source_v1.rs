//! Complete, provisional Legacy Source inventory under one selected root.
//! The output never grants publication authority to its consumer.

use std::{
    cell::{Cell, RefCell},
    io,
    num::NonZeroU64,
    os::fd::AsFd as _,
    path::Path,
};

use anyhow::{Result, anyhow, ensure};
use quanta_index_catalog::CatalogReadOnlyIoAdmission;
use quanta_index_contract::{
    CandidateCommitmentV1, ManifestGeneration, RepoId, RepoMapExpectedActiveV2, RevisionId,
    index::{
        Digest, LexicalSourceHeadsCommitmentV1, MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1,
        SourceSnapshotExportFrameV1,
    },
};
use quanta_index_core::{
    ExistingTreeMetadataIoPortV1, RequestBudgetV1,
    index::{IndexError, MemoryLifetime, ObjectReadIoPort, WorkBudget},
};
use quanta_index_lexical::legacy::SourceVisitFailureV1;
use quanta_index_searchd::cli::NativeSourceExportCommandV1;
use serde::Serialize;

use super::{
    super::{boot, with_native_recovery_control_v1},
    write_frame,
};
use crate::deployment_selection;

fn write_source_frame_v1<T: Serialize>(
    output: &mut impl io::Write,
    frame: &T,
    budget: &mut WorkBudget,
) -> std::result::Result<(), IndexError> {
    write_frame(output, frame, budget).map_err(|cause| {
        cause
            .downcast::<IndexError>()
            .unwrap_or_else(|cause| IndexError::Storage(format!("Source export frame: {cause}")))
    })
}

fn export_source_heads_v1(
    root: &Path,
    catalog: &dyn CatalogReadOnlyIoAdmission,
    objects: &dyn ObjectReadIoPort,
    metadata: &dyn ExistingTreeMetadataIoPortV1,
    maximum_objects: usize,
    repomap_max_bytes: u64,
    output: &mut impl io::Write,
    budget: &mut WorkBudget,
) -> Result<()> {
    let census = crate::native_product::legacy_source_census_v1::collect_active_source_heads_v1(
        root, catalog, objects, metadata, budget,
    )?;
    let heads = &census.heads;
    let total = u64::try_from(heads.len())?;
    let lexical_track = root.join("indexes/lexical");
    let _chunk_memory = budget
        .reserve_memory(
            u64::try_from(MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1)? * 4,
            MemoryLifetime::Temporary,
        )?
        .ok_or(IndexError::ResourceLimited)?;
    for head in heads {
        let ack = head
            .applied_ack
            .as_ref()
            .ok_or(IndexError::DigestConflict)?;
        crate::native_product::legacy_source_census_v1::verify_active_source_head_files_v1(
            root, head, objects, metadata, budget,
        )?;
        let shared_output = RefCell::new(&mut *output);
        let commitment = RefCell::<Option<LexicalSourceHeadsCommitmentV1>>::new(None);
        let file_count = Cell::new(0_u64);
        let mut begin = |event: &quanta_index_contract::SourcePublicationEvent,
                         source_heads: LexicalSourceHeadsCommitmentV1,
                         budget: &mut WorkBudget|
         -> std::result::Result<(), IndexError> {
            if commitment
                .borrow_mut()
                .replace(source_heads.clone())
                .is_some()
            {
                return Err(IndexError::DigestConflict);
            }
            let mut output = shared_output.borrow_mut();
            write_source_frame_v1(
                &mut **output,
                &SourceSnapshotExportFrameV1::HeadBegin {
                    repo_id: head.repo_id.clone(),
                    revision_id: head.revision_id.clone(),
                    active: head.active.clone(),
                    applied_ack: ack.clone(),
                    source_publication_event: event.clone(),
                    source_heads,
                    repomap_active: head.repomap_active.clone(),
                },
                budget,
            )
        };
        let mut file = |source: &quanta_index_contract::SourceFileRevision,
                        text_admitted: bool,
                        bytes: &[u8],
                        budget: &mut WorkBudget|
         -> std::result::Result<(), IndexError> {
            if commitment.borrow().is_none() {
                return Err(IndexError::DigestConflict);
            }
            let mut output = shared_output.borrow_mut();
            write_source_frame_v1(
                &mut **output,
                &SourceSnapshotExportFrameV1::FileBegin {
                    source: source.clone(),
                    text_admitted,
                    source_bytes: u64::try_from(bytes.len())
                        .map_err(|_| IndexError::ResourceLimited)?,
                },
                budget,
            )?;
            for (ordinal, chunk) in bytes.chunks(MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1).enumerate() {
                let offset = ordinal
                    .checked_mul(MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1)
                    .ok_or(IndexError::ResourceLimited)?;
                write_source_frame_v1(
                    &mut **output,
                    &SourceSnapshotExportFrameV1::FileChunk {
                        offset: u64::try_from(offset).map_err(|_| IndexError::ResourceLimited)?,
                        bytes: chunk.to_vec(),
                    },
                    budget,
                )?;
            }
            write_source_frame_v1(
                &mut **output,
                &SourceSnapshotExportFrameV1::FileEnd {
                    source: source.clone(),
                },
                budget,
            )?;
            file_count.set(
                file_count
                    .get()
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            Ok(())
        };
        let count = quanta_index_lexical::legacy::visit_existing_complete_source_files_v1(
            &lexical_track,
            &head.active.generation.lexical,
            rustix::process::geteuid().as_raw(),
            maximum_objects,
            metadata,
            objects,
            budget,
            &mut begin,
            &mut file,
        )
        .map_err(|cause| match cause {
            SourceVisitFailureV1::Visitor(cause) => {
                anyhow::Error::new(cause).context("Source visitor refused")
            }
            physical => anyhow::Error::new(physical),
        })?;
        ensure!(
            count == file_count.get(),
            "Source file frame count differs from sealed file census"
        );
        let source_heads = commitment
            .borrow()
            .clone()
            .ok_or(IndexError::DigestConflict)?;
        let mut output = shared_output.borrow_mut();
        write_frame(
            &mut **output,
            &SourceSnapshotExportFrameV1::HeadComplete {
                repo_id: head.repo_id.clone(),
                revision_id: head.revision_id.clone(),
                file_count: count,
                source_heads,
            },
            budget,
        )?;
    }
    let mut rechecked = 0_u64;
    let _visited =
        quanta_index_search_plane::readiness::visit_existing_active_search_corpus_heads_v1(
            &root.join("activations"),
            objects,
            metadata,
            budget,
            &mut |repo, revision, active, _budget| {
                let position = heads
                    .binary_search_by(|head| {
                        (head.repo_id.as_str(), head.revision_id.as_str())
                            .cmp(&(repo.as_str(), revision.as_str()))
                    })
                    .map_err(|_| IndexError::DigestConflict)?;
                if heads[position].active != *active {
                    return Err(IndexError::DigestConflict.into());
                }
                rechecked = rechecked
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                Ok(())
            },
        )?;
    ensure!(
        rechecked == total,
        "Source activation census changed during export"
    );
    for map in &census.repomap_heads {
        let candidate = &map.candidate;
        let activation = &map.activation;
        ensure!(
            activation.active
                && activation.repo_id == candidate.repo_id
                && activation.revision_id == candidate.revision_id
                && activation.manifest_generation == candidate.manifest_generation
                && activation.candidate_commitment == candidate.candidate_commitment,
            "active RepoMap candidate differs from Catalog census"
        );
        let repo_id = RepoId::new(&candidate.repo_id)?;
        let revision_id = RevisionId::new(&candidate.revision_id)?;
        let old_active = RepoMapExpectedActiveV2::new(
            NonZeroU64::new(activation.epoch).ok_or(IndexError::DigestConflict)?,
            CandidateCommitmentV1::from_bytes(activation.candidate_commitment),
        );
        let mut visited = false;
        let verified = quanta_index_repomap::visit_existing_active_repomap_compiled_payload_v1(
            root,
            candidate,
            None,
            rustix::process::geteuid().as_raw(),
            repomap_max_bytes,
            objects,
            budget,
            &mut |payload, budget| {
                if visited
                    || u64::try_from(payload.len()).map_err(|_| IndexError::ResourceLimited)?
                        != candidate.byte_size
                {
                    return Err(IndexError::DigestConflict.into());
                }
                visited = true;
                write_source_frame_v1(
                    output,
                    &SourceSnapshotExportFrameV1::RepoMapBegin {
                        repo_id: repo_id.clone(),
                        revision_id: revision_id.clone(),
                        generation: ManifestGeneration::new(candidate.manifest_generation),
                        old_active: old_active.clone(),
                        payload_bytes: candidate.byte_size,
                        payload_sha256: Digest(candidate.content_digest),
                    },
                    budget,
                )?;
                for (ordinal, chunk) in payload
                    .chunks(MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1)
                    .enumerate()
                {
                    let offset = ordinal
                        .checked_mul(MAX_SOURCE_SNAPSHOT_CHUNK_BYTES_V1)
                        .ok_or(IndexError::ResourceLimited)?;
                    write_source_frame_v1(
                        output,
                        &SourceSnapshotExportFrameV1::RepoMapChunk {
                            offset: u64::try_from(offset)
                                .map_err(|_| IndexError::ResourceLimited)?,
                            bytes: chunk.to_vec(),
                        },
                        budget,
                    )?;
                }
                Ok(())
            },
        )?;
        verified.require_current_v1(root, objects, budget)?;
        ensure!(visited, "active RepoMap payload was not visited");
        write_frame(
            output,
            &SourceSnapshotExportFrameV1::RepoMapComplete {
                repo_id,
                revision_id,
            },
            budget,
        )?;
    }
    write_frame(
        output,
        &SourceSnapshotExportFrameV1::InventoryComplete {
            head_count: total,
            repomap_head_count: u64::try_from(census.repomap_heads.len())?,
        },
        budget,
    )?;
    Ok(())
}

pub fn run_source_v1(
    command: &NativeSourceExportCommandV1,
    request: RequestBudgetV1,
) -> Result<()> {
    command.validate()?;
    ensure!(
        rustix::fs::FileType::from_raw_mode(rustix::fs::fstat(io::stdout().as_fd())?.st_mode)
            == rustix::fs::FileType::Fifo,
        "Source exporter requires a private inherited stdout pipe"
    );
    let _inherited_umask = quanta_index_searchd::app::harden_umask();
    with_native_recovery_control_v1(
        &command.config_path,
        command.config_digest,
        request,
        |_context, budget| {
            let stdout = io::stdout();
            let mut output = stdout.lock();
            deployment_selection::with_locked_export_selection_v1(
                &command.selection_path,
                command.expected_current_sha256,
                budget,
                |selection, budget| {
                    if command.source_only_native {
                        let selected = selection.native_command_v1().ok_or_else(|| {
                            anyhow!("Source-only Native export requires selected Native root")
                        })?;
                        ensure!(
                            selected.config_path == command.config_path
                                && selected.config_digest == Some(command.config_digest),
                            "Source-only Native export config differs from the held selection"
                        );
                        return super::native_source_v1::export_v1(selection, &mut output, budget);
                    }
                    ensure!(
                        selection.native_command_v1().is_none(),
                        "Legacy Source export requires selected Legacy root"
                    );
                    boot::with_legacy_source_export_v1(
                        selection,
                        budget,
                        |root,
                         catalog,
                         objects,
                         metadata,
                         maximum_objects,
                         repomap_max_bytes,
                         budget| {
                            export_source_heads_v1(
                                root,
                                catalog,
                                objects,
                                metadata,
                                maximum_objects,
                                repomap_max_bytes,
                                &mut output,
                                budget,
                            )
                        },
                    )
                },
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use quanta_index_core::index::WorkLimits;

    use super::*;

    #[test]
    fn source_frame_refuses_budget_and_cancellation_without_output() {
        let frame = SourceSnapshotExportFrameV1::FileChunk {
            offset: 0,
            bytes: vec![1, 2, 3],
        };
        let assert_refused = |mut budget, expected, phase| {
            let mut output = Vec::new();
            assert_eq!(
                write_source_frame_v1(&mut output, &frame, &mut budget),
                Err(expected),
                "{phase}"
            );
            assert!(output.is_empty(), "{phase}");
        };
        let limits = WorkLimits {
            reads: 1,
            writes: 2,
            bytes: 1024,
            mutations: 1,
        };
        for (limits, phase) in [
            (WorkLimits { bytes: 1, ..limits }, "frame census"),
            (
                WorkLimits {
                    writes: 1,
                    ..limits
                },
                "frame write",
            ),
        ] {
            assert_refused(
                WorkBudget::with_request(
                    limits,
                    RequestBudgetV1::for_duration(Duration::from_secs(30)),
                ),
                IndexError::ResourceLimited,
                phase,
            );
        }
        let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        request.cancel_handle().cancel();
        assert_refused(
            WorkBudget::with_request(limits, request),
            IndexError::Cancelled,
            "cancelled request",
        );
    }
}
