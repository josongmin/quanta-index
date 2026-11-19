//! Recheck an already selected offline root before settling a retained terminal.
//! The staged witness supplies history; these reads cannot issue Source or publish.

use super::*;
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

/// Hold the selected root and recheck its immutable Q facts through settlement.
/// A missing or changed physical identity fails closed before the callback.
pub(crate) fn with_verified_settlement_root_v1<T>(
    selected: &crate::deployment_selection::DeploymentSelectionDocumentV1,
    config: &ValidatedConfig,
    expected_history: Option<&NativeCutoverInstalledHistoryV1>,
    expected_root: InodeKey,
    legacy: Option<&crate::deployment_selection::LegacyPublishedRecoveryWitnessV1>,
    budget: &mut WorkBudget,
    settle: impl FnOnce(&mut WorkBudget) -> Result<T>,
) -> Result<T> {
    if expected_root.inode == 0 {
        return Err(IndexError::DigestConflict.into());
    }
    if selected.native_command_v1().is_some() {
        if legacy.is_some() || selected.state_root_v1() != config.state_root.as_path() {
            return Err(IndexError::DigestConflict.into());
        }
        let held = HeldPreviousNativeV1::acquire_v1(selected, budget)?;
        require_root_inode_v1(selected.state_root_v1(), expected_root, budget)?;
        if let Some(history) = expected_history {
            require_history_v1(&held, history, budget)?;
        }
        let result = settle(budget)?;
        require_root_inode_v1(selected.state_root_v1(), expected_root, budget)?;
        held.require_current_and_retired_v1(budget)?;
        return Ok(result);
    }
    if expected_history.is_some() || selected.state_root_v1() == config.state_root.as_path() {
        return Err(IndexError::DigestConflict.into());
    }
    let legacy = legacy.ok_or(IndexError::SnapshotUnavailable)?;
    let (lease, memory) = super::acquire_recovery_root_v1(selected.state_root_v1(), budget)?;
    let authority =
        super::legacy_read_factory::LegacyReadAuthorityV1::acquire_for_checkpoint_export_v1(
            selected, lease, memory, budget,
        )?;
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
    let result = settle(budget)?;
    require_root_inode_v1(authority.root(), expected_root, budget)?;
    authority.require_current_and_retired(budget)?;
    Ok(result)
}
