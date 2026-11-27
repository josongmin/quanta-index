//! Recheck an already selected offline root before settling a retained terminal.
//! The staged witness supplies history; these reads cannot issue Source or publish.

use super::*;
use crate::deployment_selection::{
    LegacyPublishedRecoveryWitnessV1, PublishedRecoveryDispositionV1,
    PublishedRecoveryRootWitnessV1, SourceOnlyRecoveryIdentityV2,
};
use quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceLegacyRepoMapV1 as RepoMap;
use quanta_index_core::{
    index::{IndexError, WorkUsage},
    publication_capacity::InodeKey,
};
use std::os::unix::fs::MetadataExt;

fn require_root_inode_v1(
    root: &std::path::Path,
    expected: InodeKey,
    budget: &mut WorkBudget,
) -> Result<()> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 256,
        ..Default::default()
    })?;
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.file_type().is_dir()
        || (InodeKey {
            device: metadata.dev(),
            inode: metadata.ino(),
        }) != expected
    {
        return Err(IndexError::DigestConflict.into());
    }
    budget.checkpoint()?;
    Ok(())
}

fn require_history_v1(
    held: &HeldPreviousNativeV1,
    expected: &NativeCutoverInstalledHistoryV1,
    budget: &mut WorkBudget,
) -> Result<()> {
    expected
        .validate_v1()
        .map_err(|error| IndexError::CorruptObject(error.to_string()))?;
    let root = held.selected_config_v1().state_root.as_path();
    let published = quanta_index_catalog::verify_native_published_operation(
        root,
        held.catalog_io_v1(),
        expected.operation,
        budget,
    )?;
    if published.operation().key != expected.operation || published.publication() != expected.view {
        return Err(IndexError::DigestConflict.into());
    }
    let mut matched = false;
    let _visited =
        held.visit_history_members_v1(None, budget, |input, _reader, source_input, _budget| {
            if input.instance != expected.instance {
                return Ok(());
            }
            if matched
                || input.member != expected.member
                || input.cut != expected.cut
                || source_input != expected.source_input
            {
                return Err(IndexError::DigestConflict.into());
            }
            matched = true;
            Ok(())
        })?;
    if !matched {
        return Err(IndexError::SnapshotUnavailable.into());
    }
    held.require_current_and_retired_v1(budget)?;
    Ok(())
}

fn require_source_only_v2(
    held: &HeldPreviousNativeV1,
    selected: &crate::deployment_selection::DeploymentSelectionDocumentV1,
    config: &ValidatedConfig,
    expected: &SourceOnlyRecoveryIdentityV2,
    budget: &mut WorkBudget,
) -> Result<()> {
    let command = selected
        .native_command_v1()
        .ok_or(IndexError::SnapshotUnavailable)?;
    let boot = command.identity.ok_or(IndexError::SnapshotUnavailable)?;
    let observed = held.observed_v1();
    if command.install_catalog_digest != Some(expected.install_catalog_digest)
        || boot.root_nonce != expected.boot_root_nonce.0
        || boot.manifest_operation != expected.boot_operation
        || observed.snapshot.root_nonce != expected.boot_root_nonce.0
        || observed
            .snapshot
            .selected
            .as_ref()
            .is_none_or(|selected| selected.operation.key != expected.boot_operation)
        || config.legacy_repomap_selection.is_some()
        || held.selected_config_v1().legacy_repomap_selection.is_some()
    {
        return Err(IndexError::DigestConflict.into());
    }
    let published = quanta_index_catalog::verify_native_published_operation(
        held.selected_config_v1().state_root.as_path(),
        held.catalog_io_v1(),
        expected.source_operation,
        budget,
    )?;
    if published.operation().key != expected.source_operation
        || published.operation().request != expected.source_request
        || published.publication() != expected.source_view
        || published.publication_metadata().source != expected.source_cut
    {
        return Err(IndexError::DigestConflict.into());
    }
    held.require_current_and_retired_v1(budget)?;
    Ok(())
}

/// Hold the selected root and recheck its immutable Q facts through settlement.
/// A missing or changed physical identity fails closed before the callback.
pub(crate) fn with_verified_settlement_root_v1<T>(
    locked: &LockedDeploymentSelectionV1,
    config: &ValidatedConfig,
    witness: PublishedRecoveryRootWitnessV1<'_>,
    budget: &mut WorkBudget,
    settle: impl FnOnce(&mut WorkBudget) -> Result<T>,
) -> Result<T> {
    let selected = locked.candidate_v1();
    let expected_root = witness.root;
    if expected_root.inode == 0 {
        return Err(IndexError::DigestConflict.into());
    }
    if selected.native_command_v1().is_some() {
        if selected.state_root_v1() != config.state_root.as_path() {
            return Err(IndexError::DigestConflict.into());
        }
        let held = HeldPreviousNativeV1::acquire_v1(selected, budget)?;
        require_root_inode_v1(selected.state_root_v1(), expected_root, budget)?;
        return match &witness.disposition {
            PublishedRecoveryDispositionV1::HistoricalV1 { legacy, repomap } => {
                if let Some(history) = witness.history {
                    require_history_v1(&held, history, budget)?;
                }
                let (proof, old_legacy) =
                    verify_native_repomap_v1(&held, locked, config, *legacy, repomap, budget)?;
                if let Some(proof) = &proof {
                    proof.require_current_v1(old_legacy.as_ref(), config, budget)?;
                }
                let result = settle(budget)?;
                require_root_inode_v1(selected.state_root_v1(), expected_root, budget)?;
                if let Some(proof) = &proof {
                    proof.require_current_v1(old_legacy.as_ref(), config, budget)?;
                }
                if let Some(old_legacy) = &old_legacy {
                    old_legacy.require_current_and_retired(budget)?;
                }
                held.require_current_and_retired_v1(budget)?;
                Ok(result)
            }
            PublishedRecoveryDispositionV1::SourceOnlyV2(expected) => {
                require_source_only_v2(&held, selected, config, expected, budget)?;
                let history = held.measure_boot_installed_history_v7(budget)?;
                if history.as_ref().map(|measured| &measured.value) != witness.history {
                    return Err(IndexError::DigestConflict.into());
                }
                let result = settle(budget)?;
                require_root_inode_v1(selected.state_root_v1(), expected_root, budget)?;
                require_source_only_v2(&held, selected, config, expected, budget)?;
                Ok(result)
            }
        };
    }
    if witness.history.is_some() || selected.state_root_v1() == config.state_root.as_path() {
        return Err(IndexError::DigestConflict.into());
    }
    let PublishedRecoveryDispositionV1::HistoricalV1 { legacy, repomap } = witness.disposition
    else {
        return Err(IndexError::DigestConflict.into());
    };
    if matches!(repomap, RepoMap::Required { state_root, .. }
        if std::path::Path::new(state_root) != selected.state_root_v1())
    {
        return with_recovery_boot_authority(config, budget, |native, budget| {
            settle_legacy_v1(
                locked,
                config,
                Some(native.io.as_ref()),
                expected_root,
                legacy,
                repomap,
                budget,
                settle,
            )
        });
    }
    settle_legacy_v1(
        locked,
        config,
        None,
        expected_root,
        legacy,
        repomap,
        budget,
        settle,
    )
}

fn verify_native_repomap_v1(
    held: &HeldPreviousNativeV1,
    locked: &LockedDeploymentSelectionV1,
    config: &ValidatedConfig,
    legacy: Option<&LegacyPublishedRecoveryWitnessV1>,
    repomap: &RepoMap,
    budget: &mut WorkBudget,
) -> Result<(
    Option<VerifiedLegacyRepoMapV1>,
    Option<legacy_read_factory::LegacyReadAuthorityV1>,
)> {
    let previous = locked.current_v1();
    if legacy.is_some() != previous.native_command_v1().is_none() {
        return Err(IndexError::DigestConflict.into());
    }
    let inspect_previous_legacy = match repomap {
        RepoMap::NotApplicable { .. } => true,
        RepoMap::Required { state_root, .. } => {
            std::path::Path::new(state_root) == previous.state_root_v1()
        }
    };
    if let Some(scope) = legacy.filter(|_| inspect_previous_legacy) {
        if config.legacy_repomap_selection.is_some() {
            return Err(IndexError::DigestConflict.into());
        }
        let (lease, memory) = acquire_recovery_root_v1(previous.state_root_v1(), budget)?;
        let legacy = legacy_read_factory::LegacyReadAuthorityV1::acquire(
            previous,
            held.native_io_v1(),
            lease,
            memory,
            budget,
        )?;
        let proof = match repomap {
            RepoMap::NotApplicable { .. } => {
                quanta_index_catalog::require_legacy_repomap_inapplicable_v1(
                    legacy.root(),
                    legacy.catalog(),
                    &scope.repo_id,
                    &scope.revision_id,
                    budget,
                )?;
                None
            }
            RepoMap::Required {
                locator_digest,
                publish,
                activation,
                ..
            } => {
                legacy.require_repomap_locator_v1(*locator_digest, budget)?;
                let files =
                    quanta_index_catalog::with_legacy_active_repomap_receipts_with_siblings_v1(
                        legacy.root(),
                        legacy.catalog(),
                        publish,
                        activation,
                        budget,
                        |candidate, budget| {
                            legacy.visit_active_repomap_compiled_v1(
                                candidate,
                                Some((publish, activation)),
                                budget,
                                &mut |_, _| Ok(()),
                            )
                        },
                    )?;
                Some(VerifiedLegacyRepoMapV1 {
                    files,
                    separate: None,
                    locator: *locator_digest,
                })
            }
        };
        legacy.retire_catalog(budget)?;
        return Ok((proof, Some(legacy)));
    }
    let proof = restore_forward::verify_native_separate_repomap_v1(
        repomap,
        config,
        previous.state_root_v1(),
        locked.candidate_v1().state_root_v1(),
        held.native_io_v1(),
        locked,
        budget,
    )?;
    Ok((proof, None))
}

fn settle_legacy_v1<T>(
    locked: &LockedDeploymentSelectionV1,
    config: &ValidatedConfig,
    native_io: Option<&crate::native_scratch_io::NativeScratchIo>,
    expected_root: InodeKey,
    legacy: Option<&LegacyPublishedRecoveryWitnessV1>,
    repomap: &RepoMap,
    budget: &mut WorkBudget,
    settle: impl FnOnce(&mut WorkBudget) -> Result<T>,
) -> Result<T> {
    let selected = locked.candidate_v1();
    let legacy = legacy.ok_or(IndexError::SnapshotUnavailable)?;
    let (lease, memory) = super::acquire_recovery_root_v1(selected.state_root_v1(), budget)?;
    let authority = match native_io {
        Some(io) => legacy_read_factory::LegacyReadAuthorityV1::acquire(
            selected, io, lease, memory, budget,
        )?,
        None => legacy_read_factory::LegacyReadAuthorityV1::acquire_for_checkpoint_export_v1(
            selected, lease, memory, budget,
        )?,
    };
    require_root_inode_v1(authority.root(), expected_root, budget)?;
    let observed = quanta_index_catalog::verify_legacy_catalog_quiescence_v1(
        authority.root(),
        authority.catalog(),
        &legacy.repo_id,
        &legacy.revision_id,
        budget,
    )?;
    if observed.latest_retained_search_corpus_sequence != Some(legacy.durable_sequence) {
        return Err(IndexError::DigestConflict.into());
    }
    let repomap = verify_legacy_repomap_v1(
        &authority, &observed, repomap, native_io, config, locked, budget,
    )?;
    drop(observed);
    let activation =
        quanta_index_search_plane::readiness::inspect_existing_activation_repository_v1(
            &authority.root().join("activations"),
            &legacy.repo_id,
            &legacy.revision_id,
            authority.objects(),
            authority.metadata(),
            budget,
        )?;
    let head = activation
        .head_v1()
        .ok_or(IndexError::SnapshotUnavailable)?;
    if crate::deployment_selection::legacy_recovery_activation_digest_v1(head, budget)?
        != legacy.activation
    {
        return Err(IndexError::DigestConflict.into());
    }
    super::verify_legacy_active_search_corpus_v1(
        &authority,
        head,
        head.generation.lexical.manifest_generation,
        budget,
    )?;
    drop(activation);
    authority.retire_catalog(budget)?;
    if let Some(proof) = &repomap {
        proof.require_current_v1(Some(&authority), config, budget)?;
    }
    let result = settle(budget)?;
    require_root_inode_v1(authority.root(), expected_root, budget)?;
    if let Some(proof) = &repomap {
        proof.require_current_v1(Some(&authority), config, budget)?;
    }
    authority.require_current_and_retired(budget)?;
    Ok(result)
}
