//! Restore a prepared Native root while retaining both offline root leases.
//! No physical Q observation can replace the signed Source replay scope.

use super::*;
use quanta_index_contract::{
    native_cutover_evidence_v1::{
        NativeCutoverEvidenceSourceFactV1 as Fact, NativeCutoverEvidenceV1,
        NativeCutoverReservationEvidenceV1, NativeCutoverEvidenceLegacyRepoMapV1 as RepoMap,
        NativeCutoverEvidencePreviousNativeSourceCoverageV1,
        NativeCutoverEvidenceSourcePublicationBindingV1,
    },
    native_cutover_source_join_v1::NativeCutoverJoinedSourceFactsV1,
};

#[path = "restore_forward/held_previous_native_v1.rs"]
mod held_previous_native_v1;

#[cfg(all(test, target_os = "linux"))]
pub(super) use held_previous_native_v1::HeldPreviousNativeV1;

#[derive(Debug)]
pub(crate) enum RestoreForwardSourceRefusalV1 {
    MissingCurrentLegacyPublication,
    LegacyHistoryBootstrapRequired,
    CandidateIsNotFreshFull,
    MissingPreviousNativeSourceAssociation,
    MissingPreviousNativeSourceCoverage,
    PreviousNativeSourceCoverageMismatch,
    NativeRepoMapVerificationUnavailable,
}

impl std::fmt::Display for RestoreForwardSourceRefusalV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingCurrentLegacyPublication => {
                "Source did not bind the previous Legacy publication to the fresh SDK snapshot"
            }
            Self::LegacyHistoryBootstrapRequired => {
                "Legacy Catalog retains History rows and requires a verified Native History bootstrap"
            }
            Self::CandidateIsNotFreshFull => {
                "restore-forward candidate is not the fresh initial FULL Source scope"
            }
            Self::MissingPreviousNativeSourceAssociation => {
                "previous Native head has no completed signed Source association"
            }
            Self::MissingPreviousNativeSourceCoverage => {
                "previous Native head has no completed Source replay coverage"
            }
            Self::PreviousNativeSourceCoverageMismatch => {
                "previous Native head to fresh FULL Source replay coverage differs"
            }
            Self::NativeRepoMapVerificationUnavailable => {
                "previous Native RepoMap selection has no verified physical authority"
            }
        })
    }
}

impl std::error::Error for RestoreForwardSourceRefusalV1 {}

/// Build candidate-local Q expectations from the exact current Initial row.
/// The global baseline can belong to a different, historical Native root.
fn fresh_initial_source_inputs_v1<'a>(
    evidence: &'a NativeCutoverEvidenceV1,
    joined: &'a NativeCutoverJoinedSourceFactsV1<'a>,
    budget: &mut WorkBudget,
) -> Result<RecoverySourceInputs<'a>> {
    let operation = joined.current_native_target.operation;
    if joined.current_legacy_replay.is_some()
        || evidence.legacy_publication.is_some()
        || joined.ast_published_work.is_some()
        || joined
            .native_initial_source_for_operation_v1(operation)
            .is_none()
    {
        return Err(RestoreForwardSourceRefusalV1::CandidateIsNotFreshFull.into());
    }
    let mut nonce = None;
    for fact in &evidence.source_facts {
        budget.charge(quanta_index_core::index::WorkUsage {
            reads: 1,
            bytes: u64::try_from(std::mem::size_of::<quanta_index_core::index::OperationKey>())?,
            ..Default::default()
        })?;
        if let Fact::Reservation {
            target,
            evidence: NativeCutoverReservationEvidenceV1::Initial,
            candidate_root_nonce: Some(actual),
        } = fact
            && target.operation == operation
        {
            if nonce.replace(*actual).is_some() || target != joined.current_native_target {
                return Err(IndexError::DigestConflict.into());
            }
        }
    }
    let nonce = nonce.ok_or(RestoreForwardSourceRefusalV1::CandidateIsNotFreshFull)?;
    let request = joined
        .current_native_update_request_v1()
        .ok_or(RestoreForwardSourceRefusalV1::CandidateIsNotFreshFull)?;
    if request.operation != operation {
        return Err(IndexError::DigestConflict.into());
    }
    Ok(RecoverySourceInputs {
        target: joined.current_native_target,
        baseline_candidate_root_nonce: Some(nonce),
        request,
        current_target: joined.current_native_target,
        current_request: request,
        legacy_publication: None,
        joined_source: Some(joined),
        ast_work: None,
    })
}

fn require_previous_native_source_association_v1<'a>(
    rows: &held_previous_native_v1::PreviousNativeSourceRowsV1,
    joined: &NativeCutoverJoinedSourceFactsV1<'a>,
) -> Result<&'a NativeCutoverEvidenceSourcePublicationBindingV1> {
    let initial = joined.native_initial_source_for_operation_v1(rows.old_head_operation);
    let watch = joined.completed_watch_source_for_operation_v1(rows.old_head_operation);
    let sdk = joined.native_sdk_source_for_operation_v1(rows.old_head_operation);
    if usize::from(initial.is_some()) + usize::from(watch.is_some())
        + usize::from(sdk.is_some()) != 1
    {
        return Err(RestoreForwardSourceRefusalV1::MissingPreviousNativeSourceAssociation.into());
    }
    if let Some((source, _)) = initial {
        return Ok(source);
    }
    if let Some((source, _)) = watch {
        return Ok(source);
    }
    if let Some((source, _)) = sdk {
        return Ok(source);
    }
    Err(RestoreForwardSourceRefusalV1::MissingPreviousNativeSourceAssociation.into())
}

fn require_previous_native_source_coverage_v1(
    coverage: Option<&NativeCutoverEvidencePreviousNativeSourceCoverageV1>,
    previous_root_nonce: quanta_index_contract::index::Digest,
    previous_operation: quanta_index_contract::index::OperationKey,
    previous_source: &NativeCutoverEvidenceSourcePublicationBindingV1,
    fresh_operation: quanta_index_contract::index::OperationKey,
    fresh_source: &NativeCutoverEvidenceSourcePublicationBindingV1,
) -> Result<()> {
    let coverage = coverage.ok_or(RestoreForwardSourceRefusalV1::MissingPreviousNativeSourceCoverage)?;
    if coverage.previous_root_nonce != previous_root_nonce
        || coverage.previous_source_operation != previous_operation
        || coverage.previous_source != *previous_source
        || coverage.fresh_full_operation != fresh_operation
        || coverage.fresh_source != *fresh_source
        || coverage.previous_source_generation == 0
        || coverage.fresh_source_generation < coverage.previous_source_generation
        || ((coverage.fresh_source_generation == coverage.previous_source_generation)
            != (coverage.previous_source == coverage.fresh_source))
    {
        return Err(RestoreForwardSourceRefusalV1::PreviousNativeSourceCoverageMismatch.into());
    }
    Ok(())
}

/// A signed V5 disposition is DATA until its saved physical selection, root
/// locator, two terminal receipts and active files have all been read under a
/// retained offline lease. Native roots cannot substitute for RepoMap roots.
pub(in crate::native_product::boot) fn verify_native_separate_repomap_v1(
    expected: &RepoMap,
    config: &ValidatedConfig,
    old_native_root: &std::path::Path,
    new_native_root: &std::path::Path,
    io: &crate::native_scratch_io::NativeScratchIo,
    locked: &LockedDeploymentSelectionV1,
    budget: &mut WorkBudget,
) -> Result<Option<VerifiedLegacyRepoMapV1>> {
    let RepoMap::Required { state_root, locator_digest, publish, activation } = expected else {
        if config.legacy_repomap_selection.is_some() {
            return Err(IndexError::DigestConflict.into());
        }
        return Ok(None);
    };
    let root = std::path::Path::new(state_root);
    budget.charge(quanta_index_core::index::WorkUsage {
        reads: 2,
        bytes: u64::try_from(state_root.len())?,
        ..Default::default()
    })?;
    if !root.is_absolute() || [old_native_root, new_native_root].into_iter()
        .any(|other| root.starts_with(other) || other.starts_with(root))
    {
        return Err(IndexError::DigestConflict.into());
    }
    let selection = config.legacy_repomap_selection.as_ref()
        .ok_or(RestoreForwardSourceRefusalV1::NativeRepoMapVerificationUnavailable)?;
    let saved = locked.read_legacy_repomap_selection_v1(
        &selection.path, quanta_index_core::index::Digest(selection.sha256), root, budget,
    )?;
    let (lease, memory) = acquire_recovery_root_v1(root, budget)?;
    let authority = legacy_read_factory::LegacyReadAuthorityV1::acquire(
        &saved, io, lease, memory, budget,
    )?;
    authority.require_repomap_locator_v1(*locator_digest, budget)?;
    let observed = quanta_index_catalog::verify_legacy_catalog_quiescence_v1(
        root, authority.catalog(), &publish.mutation.repo_id,
        &publish.mutation.revision_id, budget,
    )?;
    let files = authority.verify_active_repomap_v1(
        &observed, (publish, activation), budget,
    )?;
    drop(observed);
    authority.retire_catalog(budget)?;
    Ok(Some(VerifiedLegacyRepoMapV1 {
        files, separate: Some(authority), locator: *locator_digest,
    }))
}

fn require_previous_repomap_current_v1(
    verified: &VerifiedLegacyRepoMapV1,
    config: &ValidatedConfig,
    budget: &mut WorkBudget,
) -> Result<()> {
    let separate = verified.separate.as_ref()
        .ok_or(RestoreForwardSourceRefusalV1::NativeRepoMapVerificationUnavailable)?;
    let selection = config.legacy_repomap_selection.as_ref()
        .ok_or(RestoreForwardSourceRefusalV1::NativeRepoMapVerificationUnavailable)?;
    separate.require_repomap_locator_v1(verified.locator, budget)?;
    verified.files.require_current_v1(separate.root(), separate.objects(), budget)?;
    LockedDeploymentSelectionV1::require_legacy_repomap_selection_v1(
        &selection.path, quanta_index_core::index::Digest(selection.sha256), budget,
    )?;
    separate.require_current_and_retired(budget)
}

fn verify_candidate_install_attestation_v1(
    authority: &NativeBootAuthority,
    command: &NativeStateCommandV1,
    locked: &LockedDeploymentSelectionV1,
    budget: &mut WorkBudget,
) -> Result<()> {
    if locked.requires_candidate_install_attestation_v1() {
        let root = authority.lease.state_root_identity_v1();
        let io = NativeObjectReadIo::new(Arc::clone(&authority.physical));
        let expected = command
            .install_catalog_digest
            .ok_or(IndexError::SnapshotUnavailable)?;
        if catalog_digest::catalog_content_digest(root, &io, budget)? != expected {
            return Err(IndexError::DigestConflict.into());
        }
    }
    Ok(())
}

fn publish_previous_native_v1(
    command: &NativeStateCommandV1,
    config: &ValidatedConfig,
    registry: &AdapterRegistry,
    evidence: &NativeCutoverEvidenceV1,
    joined: &NativeCutoverJoinedSourceFactsV1<'_>,
    locked: &mut LockedDeploymentSelectionV1,
    budget: &mut WorkBudget,
    custody_checkpoint: &mut dyn FnMut(&mut WorkBudget) -> Result<()>,
) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
    let before = DeploymentSelectionErrorV1::RefusedBeforeRename;
    let source = fresh_initial_source_inputs_v1(evidence, joined, budget).map_err(before)?;
    let held =
        held_previous_native_v1::HeldPreviousNativeV1::acquire_v1(locked.current_v1(), budget)
            .map_err(before)?;
    custody_checkpoint(budget).map_err(before)?;
    let rows = held
        .observe_signed_source_rows_v1(evidence, joined, budget)
        .map_err(before)?;
    if rows.old_head != held.observed_v1().current_head.identity || rows.published_count == 0 {
        return Err(before(IndexError::DigestConflict.into()));
    }
    let previous_source =
        require_previous_native_source_association_v1(&rows, joined).map_err(before)?;
    let (fresh_source, _) = joined
        .native_initial_source_for_operation_v1(source.target.operation)
        .ok_or_else(|| before(RestoreForwardSourceRefusalV1::CandidateIsNotFreshFull.into()))?;
    budget.charge(quanta_index_core::index::WorkUsage {
        reads: 1,
        bytes: std::mem::size_of::<NativeCutoverEvidencePreviousNativeSourceCoverageV1>() as u64,
        ..Default::default()
    }).map_err(|error| before(error.into()))?;
    require_previous_native_source_coverage_v1(
        evidence.previous_native_source_coverage.as_ref(),
        rows.root_nonce,
        rows.old_head_operation,
        previous_source,
        source.target.operation,
        fresh_source,
    )
    .map_err(before)?;
    if source.baseline_candidate_root_nonce == Some(rows.root_nonce) {
        return Err(before(IndexError::DigestConflict.into()));
    }
    let old_v5 = joined.v5_artifact_for_operation_v1(rows.old_head_operation)
        .ok_or_else(|| before(RestoreForwardSourceRefusalV1::NativeRepoMapVerificationUnavailable.into()))?;
    if old_v5.operation != rows.old_head_operation
        || joined.v5_artifact.operation != source.target.operation
    {
        return Err(before(IndexError::DigestConflict.into()));
    }
    let old_repomap = &old_v5.repomap;
    let new_repomap = &joined.v5_artifact.repomap;
    with_recovery_boot_authority(config, budget, |authority, budget| {
        custody_checkpoint(budget)?;
        let candidate = verify_recovery_candidate_with_source_capture_v1(
            &authority,
            command,
            registry,
            &source,
            config.resources.physical.inventory.max_entries,
            budget,
        )?;
        verify_candidate_install_attestation_v1(&authority, command, locked, budget)?;
        let old_config = held.selected_config_v1();
        let old_verified = verify_native_separate_repomap_v1(
            old_repomap, old_config, locked.current_v1().state_root_v1(),
            authority.lease.state_root_identity_v1(), authority.io.as_ref(), locked, budget,
        )?;
        let same_repomap_root = matches!((old_repomap, new_repomap),
            (RepoMap::Required { state_root: old, .. },
             RepoMap::Required { state_root: new, .. }) if old == new);
        let new_verified = if same_repomap_root {
            if old_repomap != new_repomap {
                return Err(IndexError::DigestConflict.into());
            }
            let RepoMap::Required { state_root, .. } = new_repomap else {
                return Err(IndexError::DigestConflict.into());
            };
            let selection = config.legacy_repomap_selection.as_ref()
                .ok_or(RestoreForwardSourceRefusalV1::NativeRepoMapVerificationUnavailable)?;
            let _saved = locked.read_legacy_repomap_selection_v1(
                &selection.path, quanta_index_core::index::Digest(selection.sha256),
                std::path::Path::new(state_root), budget,
            )?;
            None
        } else {
            verify_native_separate_repomap_v1(
                new_repomap, config, locked.current_v1().state_root_v1(),
                authority.lease.state_root_identity_v1(), authority.io.as_ref(), locked, budget,
            )?
        };
        let mut previous_current = |budget: &mut WorkBudget| {
            if let Some(proof) = old_verified.as_ref() {
                require_previous_repomap_current_v1(proof, old_config, budget)?;
            }
            held.require_current_and_retired_v1(budget)
        };
        let published = publish_with_held_recovery_roots_v1(
            &authority,
            None,
            &mut previous_current,
            if same_repomap_root { old_verified.as_ref() } else { new_verified.as_ref() },
            config,
            locked,
            budget,
            custody_checkpoint,
        );
        drop(candidate);
        Ok(published)
    })
    .map_err(before)?
}

pub(in crate::native_product) fn publish_v1(
    command: &NativeStateCommandV1,
    config: &ValidatedConfig,
    registry: &AdapterRegistry,
    evidence: &NativeCutoverEvidenceV1,
    joined: &NativeCutoverJoinedSourceFactsV1<'_>,
    locked: &mut LockedDeploymentSelectionV1,
    budget: &mut WorkBudget,
    custody_checkpoint: &mut dyn FnMut(&mut WorkBudget) -> Result<()>,
    require_history_fence: &mut dyn FnMut(&mut WorkBudget) -> Result<()>,
    history: RestoreForwardHistoryV1<'_>,
) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
    use RestoreForwardSourceRefusalV1 as Refusal;
    let before = DeploymentSelectionErrorV1::RefusedBeforeRename;
    // A settled historical Native group is not a replay-coverage proof. The
    // Source owner must expose an exact old-root horizon before this mode can
    // replace a Native head, including one with post-install publications.
    if locked.current_v1().native_command_v1().is_some() {
        if !history.matches_retained_v1(None) {
            return Err(before(IndexError::DigestConflict.into()));
        }
        return publish_previous_native_v1(
            command,
            config,
            registry,
            evidence,
            joined,
            locked,
            budget,
            custody_checkpoint,
        );
    }
    let source = recovery_source_inputs_v1(evidence, joined).map_err(before)?;
    if source.target.operation != source.current_target.operation {
        return Err(before(Refusal::CandidateIsNotFreshFull.into()));
    }
    if source.legacy_publication.is_none()
        || joined.current_legacy_replay.is_none()
        || (joined.current_legacy_sdk_ack.is_none()
            && joined.current_legacy_ordinary_binding.is_none())
    {
        return Err(before(Refusal::MissingCurrentLegacyPublication.into()));
    }
    with_recovery_boot_authority(config, budget, |authority, budget| {
        let (lease, memory) =
            acquire_recovery_root_v1(locked.current_v1().state_root_v1(), budget)?;
        custody_checkpoint(budget)?;
        // Unlike a running or saved Native rollback source, this newly
        // prepared candidate must match the installation receipt attestation.
        verify_candidate_install_attestation_v1(&authority, command, locked, budget)?;
        let legacy = legacy_read_factory::LegacyReadAuthorityV1::acquire(
            locked.current_v1(),
            authority.io.as_ref(),
            lease,
            memory,
            budget,
        )?;
        let verified =
            verify_legacy_publication_v1(&legacy, &source, &authority, config, locked, budget)?;
        if verified.has_history_rows {
            require_history_fence(budget)?;
        }
        let (candidate, installed_history) = verify_recovery_candidate_with_legacy_history_v1(
            &authority,
            command,
            registry,
            &source,
            config.resources.physical.inventory.max_entries,
            verified.has_history_rows.then_some(LegacyHistoryRecoveryV1 {
                legacy: &legacy,
                producer: quanta_index_catalog::LegacyHistoryProducerScopeV1 {
                    writer_state_root: &evidence.header.writer_state_root,
                    workspace_root: &evidence.header.canonical_workspace_root,
                    contract_key: &evidence.header.contract_key,
                },
            }),
            budget,
        )
        .map_err(|error| {
            if verified.has_history_rows
                && matches!(
                    error.downcast_ref::<IndexError>(),
                    Some(IndexError::SnapshotUnavailable)
                )
            {
                Refusal::LegacyHistoryBootstrapRequired.into()
            } else {
                error
            }
        })?;
        if !history.matches_retained_v1(installed_history.as_ref().map(|selected| &selected.value)) {
            return Err(IndexError::DigestConflict.into());
        }
        legacy.retire_catalog(budget)?;
        let published = publish_with_held_recovery_roots_v1(
            &authority,
            Some(&legacy),
            &mut |budget| legacy.require_current_and_retired(budget),
            verified.repomap.as_ref(),
            config,
            locked,
            budget,
            custody_checkpoint,
        );
        if published.is_ok() {
            *history.selected = installed_history;
        }
        drop(candidate);
        Ok(published)
    })
    .map_err(before)?
}

#[cfg(test)]
mod source_coverage_tests {
    use super::*;
    use quanta_index_contract::{
        index::{CollectionId, Digest, OperationKey, ProducerId, SlotId, StreamId, TenantId,
            WorktreeId, WorkspaceKey},
        native_cutover_evidence_v1::NativeCutoverDigest64V1,
    };

    #[test]
    fn previous_native_source_coverage_binds_both_endpoints_and_completed_order() {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let previous_operation = OperationKey {
            workspace, epoch: 1, producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]), stream_epoch: 1, sequence: 7,
        };
        let fresh_operation = OperationKey { sequence: 9, ..previous_operation };
        let previous_source = NativeCutoverEvidenceSourcePublicationBindingV1 {
            transition_digest: NativeCutoverDigest64V1([b'a'; 64]),
            completion_pointer_digest: NativeCutoverDigest64V1([b'b'; 64]),
            physical_target_source_digest: NativeCutoverDigest64V1([b'c'; 64]),
            target_snapshot_id: 7,
        };
        let fresh_source = NativeCutoverEvidenceSourcePublicationBindingV1 {
            transition_digest: NativeCutoverDigest64V1([b'd'; 64]),
            completion_pointer_digest: NativeCutoverDigest64V1([b'e'; 64]),
            physical_target_source_digest: NativeCutoverDigest64V1([b'f'; 64]),
            target_snapshot_id: 9,
        };
        let root_nonce = Digest([8; 32]);
        let coverage = NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
            previous_root_nonce: root_nonce,
            previous_source_operation: previous_operation,
            previous_source,
            fresh_full_operation: fresh_operation,
            fresh_source,
            previous_source_generation: 7,
            fresh_source_generation: 9,
        };
        let accepts = |value: Option<&NativeCutoverEvidencePreviousNativeSourceCoverageV1>| {
            require_previous_native_source_coverage_v1(
                value, root_nonce, previous_operation, &previous_source,
                fresh_operation, &fresh_source,
            )
        };
        assert!(accepts(Some(&coverage)).is_ok());
        assert!(accepts(None).is_err());
        for foreign in [
            NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
                previous_root_nonce: Digest([9; 32]), ..coverage
            },
            NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
                previous_source_operation: fresh_operation, ..coverage
            },
            NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
                previous_source: fresh_source, ..coverage
            },
            NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
                fresh_full_operation: previous_operation, ..coverage
            },
            NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
                fresh_source: previous_source, ..coverage
            },
            NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
                previous_source_generation: 0, ..coverage
            },
            NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
                fresh_source_generation: 6, ..coverage
            },
            NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
                fresh_source_generation: 7, ..coverage
            },
        ] {
            assert!(accepts(Some(&foreign)).is_err());
        }
        let same_source = NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
            fresh_source: previous_source,
            fresh_source_generation: 7,
            ..coverage
        };
        assert!(require_previous_native_source_coverage_v1(
            Some(&same_source), root_nonce, previous_operation,
            &previous_source, fresh_operation, &previous_source,
        ).is_ok());
        let wrong_generation = NativeCutoverEvidencePreviousNativeSourceCoverageV1 {
            fresh_source_generation: 8, ..same_source
        };
        assert!(require_previous_native_source_coverage_v1(
            Some(&wrong_generation), root_nonce, previous_operation,
            &previous_source, fresh_operation, &previous_source,
        ).is_err());
    }
}
