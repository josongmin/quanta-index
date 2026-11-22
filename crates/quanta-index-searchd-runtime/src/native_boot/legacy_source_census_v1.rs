//! Paid, complete Legacy Source activation and Catalog ACK census.
//! It lends no publication authority and keeps every retained head grant alive.

use std::{cell::RefCell, num::NonZeroU64, path::Path};

use anyhow::{Result, ensure};
use quanta_index_catalog::{
    CatalogReadOnlyIoAdmission, LegacyActiveCatalogCensusV1, RepoMapActivationRowV1,
    RepoMapCandidateRowV1,
};
use quanta_index_contract::{
    BatchPublishReceipt, RepoId, RevisionId, SearchCorpusActiveHeadV1,
    repomap::{CandidateCommitmentV1, RepoMapExpectedActiveV2},
};
use quanta_index_core::{
    ExistingTreeMetadataIoPortV1,
    index::{IndexError, MemoryLease, MemoryLifetime, ObjectReadIoPort, WorkBudget},
};

pub(crate) struct ActiveSourceHeadV1 {
    pub(crate) repo_id: RepoId,
    pub(crate) revision_id: RevisionId,
    pub(crate) active: SearchCorpusActiveHeadV1,
    pub(crate) applied_ack: Option<BatchPublishReceipt>,
    latest_committed_sequence: Option<u64>,
    pub(crate) repomap_active: Option<RepoMapExpectedActiveV2>,
    _memory: MemoryLease,
}

pub(crate) struct ActiveSourceCensusV1 {
    pub(crate) heads: Vec<ActiveSourceHeadV1>,
    pub(crate) repomap_heads: Vec<ActiveRepoMapHeadV1>,
    pub(crate) catalog: LegacyActiveCatalogCensusV1,
}

pub(crate) struct ActiveRepoMapHeadV1 {
    pub(crate) candidate: RepoMapCandidateRowV1,
    pub(crate) activation: RepoMapActivationRowV1,
    _memory: MemoryLease,
}

fn find_head<'a>(
    heads: &'a mut [ActiveSourceHeadV1],
    repo: &str,
    revision: &str,
) -> Option<&'a mut ActiveSourceHeadV1> {
    heads
        .binary_search_by(|head| {
            (head.repo_id.as_str(), head.revision_id.as_str()).cmp(&(repo, revision))
        })
        .ok()
        .map(|position| &mut heads[position])
}

pub(crate) fn collect_active_source_heads_v1(
    root: &Path,
    catalog: &dyn CatalogReadOnlyIoAdmission,
    objects: &dyn ObjectReadIoPort,
    metadata: &dyn ExistingTreeMetadataIoPortV1,
    budget: &mut WorkBudget,
) -> Result<ActiveSourceCensusV1> {
    let mut heads = Vec::<ActiveSourceHeadV1>::new();
    let observed =
        quanta_index_search_plane::readiness::visit_existing_active_search_corpus_heads_v1(
            &root.join("activations"),
            objects,
            metadata,
            budget,
            &mut |repo, revision, active, budget| {
                active
                    .validate_v1()
                    .map_err(|_| IndexError::DigestConflict)?;
                if &active.generation.lexical.repo_id != repo
                    || &active.generation.lexical.revision_id != revision
                {
                    return Err(IndexError::DigestConflict.into());
                }
                let width = repo
                    .as_str()
                    .len()
                    .checked_add(revision.as_str().len())
                    .and_then(|value| {
                        value.checked_add(active.generation.lexical.manifest_digest.len())
                    })
                    .and_then(|value| value.checked_mul(16))
                    .and_then(|value| value.checked_add(32 * 1024))
                    .ok_or(IndexError::ResourceLimited)?;
                let memory = budget
                    .reserve_memory(
                        u64::try_from(width).map_err(|_| IndexError::ResourceLimited)?,
                        MemoryLifetime::Temporary,
                    )?
                    .ok_or(IndexError::ResourceLimited)?;
                heads
                    .try_reserve(1)
                    .map_err(|_| IndexError::ResourceLimited)?;
                heads.push(ActiveSourceHeadV1 {
                    repo_id: repo.clone(),
                    revision_id: revision.clone(),
                    active: active.clone(),
                    applied_ack: None,
                    latest_committed_sequence: None,
                    repomap_active: None,
                    _memory: memory,
                });
                Ok(())
            },
        )?;
    ensure!(
        usize::try_from(observed)? == heads.len(),
        "Source activation census count changed"
    );
    heads.sort_unstable_by(|left, right| {
        (left.repo_id.as_str(), left.revision_id.as_str())
            .cmp(&(right.repo_id.as_str(), right.revision_id.as_str()))
    });
    ensure!(
        !heads
            .windows(2)
            .any(|pair| pair[0].repo_id == pair[1].repo_id
                && pair[0].revision_id == pair[1].revision_id),
        "Source activation census contains a duplicate scope"
    );

    let heads = RefCell::new(heads);
    let repomap_heads = RefCell::new(Vec::<ActiveRepoMapHeadV1>::new());
    let catalog = quanta_index_catalog::visit_legacy_active_catalog_heads_v1(
        root,
        catalog,
        budget,
        &mut |key, receipt, sequence, _budget| {
            let mut borrowed = heads
                .try_borrow_mut()
                .map_err(|_| IndexError::ResourceLimited)?;
            let Some(head) = find_head(
                &mut borrowed,
                key.repo_id.as_str(),
                key.revision_id.as_str(),
            ) else {
                return Err(IndexError::Conflict);
            };
            // The journal stores the fresh, pre-sequence receipt. Its actual
            // commit sequence is authenticated separately by the Catalog row
            // and event; only the returned ACK is stamped with that sequence.
            if sequence == 0
                || receipt.durable_sequence != 0
                || !receipt.applied
                || receipt.generation != key.generation
                || receipt.batch_digest != key.batch_digest
            {
                return Err(IndexError::DigestConflict);
            }
            head.latest_committed_sequence = Some(
                head.latest_committed_sequence
                    .map_or(sequence, |prior| prior.max(sequence)),
            );
            let lexical = &head.active.generation.lexical;
            if key.generation != lexical.manifest_generation
                || !receipt.sealed
                || receipt.manifest_digest.as_deref() != Some(lexical.manifest_digest.as_str())
                || receipt.semantic_content.as_ref()
                    != Some(&head.active.generation.semantic_content)
            {
                return Ok(());
            }
            if head.applied_ack.is_some() {
                return Err(IndexError::DigestConflict);
            }
            head.applied_ack = Some(receipt.clone().recorded_at(sequence));
            Ok(())
        },
        &mut |candidate, activation, budget| {
            if !activation.active
                || activation.repo_id != candidate.repo_id
                || activation.revision_id != candidate.revision_id
                || activation.candidate_commitment != candidate.candidate_commitment
                || activation.manifest_generation != candidate.manifest_generation
            {
                return Err(IndexError::DigestConflict);
            }
            let width = candidate
                .repo_id
                .len()
                .checked_add(candidate.revision_id.len())
                .and_then(|n| n.checked_add(candidate.projection_meta.len()))
                .and_then(|n| {
                    n.checked_add(
                        activation
                            .invalidation_reason
                            .as_ref()
                            .map_or(0, String::len),
                    )
                })
                .and_then(|n| n.checked_mul(8))
                .and_then(|n| n.checked_add(8_192))
                .ok_or(IndexError::ResourceLimited)?;
            let memory = budget
                .reserve_memory(
                    u64::try_from(width).map_err(|_| IndexError::ResourceLimited)?,
                    MemoryLifetime::Temporary,
                )?
                .ok_or(IndexError::ResourceLimited)?;
            let mut retained = repomap_heads
                .try_borrow_mut()
                .map_err(|_| IndexError::ResourceLimited)?;
            retained
                .try_reserve(1)
                .map_err(|_| IndexError::ResourceLimited)?;
            retained.push(ActiveRepoMapHeadV1 {
                candidate: candidate.clone(),
                activation: activation.clone(),
                _memory: memory,
            });
            let mut borrowed = heads
                .try_borrow_mut()
                .map_err(|_| IndexError::ResourceLimited)?;
            let Some(head) = find_head(&mut borrowed, &activation.repo_id, &activation.revision_id)
            else {
                return Ok(());
            };
            if head.repomap_active.is_some() {
                return Err(IndexError::DigestConflict);
            }
            let epoch = NonZeroU64::new(activation.epoch).ok_or(IndexError::DigestConflict)?;
            head.repomap_active = Some(RepoMapExpectedActiveV2::new(
                epoch,
                CandidateCommitmentV1::from_bytes(activation.candidate_commitment),
            ));
            Ok(())
        },
    )?;
    let heads = heads.into_inner();
    let mut repomap_heads = repomap_heads.into_inner();
    repomap_heads.sort_unstable_by(|left, right| {
        (&left.activation.repo_id, &left.activation.revision_id)
            .cmp(&(&right.activation.repo_id, &right.activation.revision_id))
    });
    ensure!(
        heads.iter().all(|head| head
            .applied_ack
            .as_ref()
            .is_some_and(|ack| head.latest_committed_sequence == Some(ack.durable_sequence))),
        "Source activation lacks the latest committed Catalog ACK"
    );
    ensure!(
        u64::try_from(repomap_heads.len())? == catalog.active_repomap_heads
            && !repomap_heads
                .windows(2)
                .any(
                    |pair| pair[0].activation.repo_id == pair[1].activation.repo_id
                        && pair[0].activation.revision_id == pair[1].activation.revision_id
                ),
        "RepoMap active Catalog census is partial or duplicated"
    );
    Ok(ActiveSourceCensusV1 {
        heads,
        repomap_heads,
        catalog,
    })
}

/// Authenticate the selected Source seal and Semantic physical roots under the
/// caller's held Legacy root and read ports. The lexical body visitor is a
/// separate, feature-gated export step.
pub(crate) fn verify_active_source_head_files_v1(
    root: &Path,
    head: &ActiveSourceHeadV1,
    objects: &dyn ObjectReadIoPort,
    metadata: &dyn ExistingTreeMetadataIoPortV1,
    budget: &mut WorkBudget,
) -> Result<()> {
    let ack = head
        .applied_ack
        .as_ref()
        .ok_or(IndexError::DigestConflict)?;
    quanta_index_search_plane::readiness::inspect_existing_sealed_search_corpus_v1(
        root,
        &head.repo_id,
        &head.revision_id,
        ack.generation,
        &head.active.generation.lexical.manifest_digest,
        objects,
        budget,
    )?;
    let semantic = &head.active.generation.semantic;
    let semantic_dir = quanta_index_core::GenerationStorageKeyV1::for_repo_revision(
        &semantic.repo_id,
        &semantic.revision_id,
    )
    .generation_dir(&root.join("indexes/semantic"), semantic.manifest_generation);
    quanta_index_semantic::legacy::verify_existing_semantic_generation_read_only(
        &semantic_dir,
        semantic,
        rustix::process::geteuid().as_raw(),
        metadata,
        objects,
        budget,
        |roots, _budget| {
            if roots != &head.active.generation.semantic_content {
                return Err(IndexError::DigestConflict.into());
            }
            Ok(())
        },
    )?;
    Ok(())
}

// The physical activation census uses Common's Linux-only directory reader;
// other platforms deliberately refuse that port with RebuildRequired.
#[cfg(all(test, target_os = "linux"))]
#[path = "legacy_source_census_v1/tests.rs"]
mod tests;
