//! Read the selected, offline Native root under its own retained physical owner.
//! The observation is DATA; only a joined Source replay horizon can authorize a switch.

use super::*;
use quanta_index_catalog::{
    NativeBootQuiescentCatalogV1, verify_native_source_root_publications_v1,
    verify_native_source_root_quiescence_v1,
};
use quanta_index_contract::{
    native_cutover_evidence_v1::{
        MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1, NativeCutoverEvidenceSourceFactV1 as Fact,
        NativeCutoverEvidenceV1, NativeCutoverReplayPhaseV1,
        NativeCutoverAstWorkPhaseV1,
    },
    native_cutover_source_join_v1::NativeCutoverJoinedSourceFactsV1,
};
use quanta_index_core::index::{MemoryLease, MemoryLifetime, OperationKey, ViewRef, WorkUsage};
use quanta_index_searchd::cli::NativeBootIdentityV1;

struct FundedSelectedConfigV1 {
    // Owned DATA drops before its retained accounting grant.
    config: super::super::super::ValidatedConfig,
    command: NativeStateCommandV1,
    _memory: MemoryLease,
}

fn read_selected_config_v1(
    command: &NativeStateCommandV1,
    budget: &mut WorkBudget,
) -> Result<FundedSelectedConfigV1> {
    let digest = command
        .config_digest
        .ok_or(IndexError::SnapshotUnavailable)?;
    let maximum = super::super::super::MAX_CONFIG_BYTES;
    let path_bytes = u64::try_from(command.config_path.as_os_str().len())?;
    let backing = maximum
        .checked_mul(16)
        .and_then(|bytes| bytes.checked_add(path_bytes.checked_mul(2)?))
        .and_then(|bytes| {
            bytes.checked_add(u64::try_from(std::mem::size_of::<FundedSelectedConfigV1>()).ok()?)
        })
        .ok_or(IndexError::ResourceLimited)?;
    let memory = budget
        .reserve_memory(backing, MemoryLifetime::Retained)?
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 2,
        bytes: maximum,
        ..Default::default()
    })?;
    let config = super::super::super::read_config_checked(&command.config_path, Some(digest))?;
    budget.checkpoint()?;
    Ok(FundedSelectedConfigV1 {
        config,
        command: command.clone(),
        _memory: memory,
    })
}

pub(in crate::native_product::boot) struct HeldPreviousNativeV1 {
    // Drop the funded Catalog snapshot before its physical read owner.
    observed: NativeBootQuiescentCatalogV1,
    authority: NativeBootAuthority,
    selected: FundedSelectedConfigV1,
}

/// Physical Q/settled Source row correspondence only. The fresh initial FULL
/// predecessor M is not established by these fields and this is no permit.
pub(in crate::native_product::boot) struct PreviousNativeSourceRowsV1 {
    pub root_nonce: quanta_index_core::index::Digest,
    pub old_head: ViewRef,
    pub old_head_operation: OperationKey,
    pub published_count: u64,
}

impl HeldPreviousNativeV1 {
    pub(in crate::native_product::boot) fn selected_config_v1(&self) -> &super::super::super::ValidatedConfig {
        &self.selected.config
    }

    pub(in crate::native_product::boot) fn acquire_v1(
        selected: &crate::deployment_selection::DeploymentSelectionDocumentV1,
        budget: &mut WorkBudget,
    ) -> Result<Self> {
        let command = selected
            .native_command_v1()
            .ok_or(IndexError::SnapshotUnavailable)?;
        command.validate()?;
        let expected = command.identity.ok_or(IndexError::SnapshotUnavailable)?;
        let funded = read_selected_config_v1(command, budget)?;
        let config = &funded.config;
        if config.state_root != selected.state_root_v1() {
            return Err(IndexError::DigestConflict.into());
        }
        let registry = super::super::super::compose_boot_registry(config)?;
        let (observed, authority) =
            with_recovery_boot_authority(config, budget, |authority, budget| {
                let observed = verify_native_source_root_quiescence_v1(
                    authority.lease.state_root_identity_v1(),
                    authority.io.as_ref(),
                    expected.manifest_operation.workspace,
                    budget,
                )?;
                Self::check_selected_v1(
                    &authority, &observed, expected, config, &registry, budget,
                )?;
                Ok((observed, authority))
            })?;
        budget.checkpoint()?;
        Ok(Self {
            observed,
            authority,
            selected: funded,
        })
    }

    fn check_selected_v1(
        authority: &NativeBootAuthority,
        observed: &NativeBootQuiescentCatalogV1,
        expected: NativeBootIdentityV1,
        config: &ValidatedConfig,
        registry: &AdapterRegistry,
        budget: &mut WorkBudget,
    ) -> Result<()> {
        let selected = observed
            .snapshot
            .selected
            .as_ref()
            .ok_or(IndexError::SnapshotUnavailable)?;
        if identity(observed.snapshot.root_nonce, selected)? != expected {
            return Err(IndexError::DigestConflict.into());
        }
        authenticate(
            authority,
            &observed.snapshot,
            selected,
            registry,
            config.resources.physical.inventory.max_entries,
            None,
            budget,
        )?;
        require_retired(authority)?;
        authority.lease.require_state_root_v1(&config.state_root)?;
        budget.checkpoint()?;
        Ok(())
    }

    pub(in crate::native_product::boot) fn observed_v1(&self) -> &NativeBootQuiescentCatalogV1 {
        &self.observed
    }

    /// Reconcile every actual Published Q row with the canonical joined Source
    /// reservation/V9 inventory for this old root incarnation. The independent
    /// old M to fresh N Source completion bridge is still required by caller.
    pub(in crate::native_product::boot) fn observe_signed_source_rows_v1(
        &self,
        evidence: &NativeCutoverEvidenceV1,
        joined: &NativeCutoverJoinedSourceFactsV1<'_>,
        budget: &mut WorkBudget,
    ) -> Result<PreviousNativeSourceRowsV1> {
        if !std::ptr::eq(joined.baseline, &evidence.baseline)
            || !std::ptr::eq(joined.fresh_sdk, &evidence.fresh_sdk)
            || evidence.source_facts.len() > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1
        {
            return Err(IndexError::DigestConflict.into());
        }
        let root_nonce = quanta_index_core::index::Digest(self.observed.snapshot.root_nonce);
        let mut source_count = 0_u64;
        let mut latest_source = None;
        for fact in &evidence.source_facts {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: u64::try_from(std::mem::size_of::<OperationKey>())?,
                ..Default::default()
            })?;
            if let Fact::Reservation {
                target,
                candidate_root_nonce: Some(nonce),
                ..
            } = fact
                && *nonce == root_nonce
            {
                source_count = source_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                if latest_source
                    .is_none_or(|(event, _): (u64, OperationKey)| target.event_sequence > event)
                {
                    latest_source = Some((target.event_sequence, target.operation));
                }
            }
        }
        let (_, latest_operation) = latest_source.ok_or(IndexError::SnapshotUnavailable)?;
        let root = self.authority.lease.state_root_identity_v1();
        let object_root = root.join("native-objects");
        let io = Arc::new(NativeObjectReadIo::new(Arc::clone(
            &self.authority.physical,
        )));
        let lock = io.open(&object_root.join(".object-namespace.lease"), budget)?;
        let reader = FileObjectReader::open_existing(&object_root, lock, io, budget)?;
        if self.observed.snapshot.namespace != Some(reader.namespace_binding()?) {
            return Err(IndexError::DigestConflict.into());
        }
        let mut published_count = 0_u64;
        let mut ast_count = 0_u64;
        let mut found_boot = false;
        let mut found_head = None;
        let observed = verify_native_source_root_publications_v1(
            root,
            self.authority.io.as_ref(),
            latest_operation.workspace,
            budget,
            |snapshot, head, operation, publication, budget| {
                if snapshot.root_nonce != self.observed.snapshot.root_nonce
                    || snapshot.selected != self.observed.snapshot.selected
                    || snapshot.namespace != self.observed.snapshot.namespace
                    || head != &self.observed.current_head
                {
                    return Err(IndexError::DigestConflict);
                }
                let mut ast = None;
                for fact in &evidence.source_facts {
                    budget.charge(WorkUsage {
                        reads: 1,
                        bytes: u64::try_from(std::mem::size_of::<OperationKey>())
                            .map_err(|_| IndexError::ResourceLimited)?,
                        ..Default::default()
                    })?;
                    if let Fact::AstWork {
                        ast_operation: Some(key),
                        candidate_root_nonce,
                        phase: NativeCutoverAstWorkPhaseV1::Published,
                        source_operation,
                        source_view,
                        source_cut,
                        published_view: Some(view),
                        request: Some(request),
                        ..
                    } = fact && *key == operation.key {
                        if *candidate_root_nonce != Some(root_nonce)
                            || ast.replace((*source_operation, *source_view, *source_cut,
                                *view, request)).is_some()
                        {
                            return Err(IndexError::DigestConflict);
                        }
                    }
                }
                if let Some((source_operation, source_view, source_cut, ast_view, request)) = ast {
                    let source = quanta_index_catalog::verify_native_published_operation(
                        root, self.authority.io.as_ref(), source_operation, budget,
                    )?;
                    if source.snapshot().root_nonce != snapshot.root_nonce
                        || source.publication() != source_view
                        || source.publication_metadata().source != source_cut
                        || publication.identity != ast_view
                        || publication.source != source_cut
                    {
                        return Err(IndexError::DigestConflict);
                    }
                    let max_pending = u32::try_from(
                        self.selected.config.resources.physical.inventory.max_entries,
                    ).map_err(|_| IndexError::ResourceLimited)?;
                    let [stored_request, _profile] = reader.verify_closure_with_selected_roots(
                        &[operation.request, publication.profile],
                        [operation.request, publication.profile], max_pending, budget,
                    )?;
                    quanta_index_kernel::verify_native_boot_captured_request_v1(
                        request, operation, &stored_request, budget,
                    )?;
                    ast_count = ast_count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    published_count = published_count.checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                    if snapshot.selected.as_ref()
                        .is_some_and(|selected| selected.operation.key == operation.key)
                    {
                        found_boot = true;
                    }
                    if publication.identity == head.identity {
                        if found_head.replace(source_operation).is_some() {
                            return Err(IndexError::DigestConflict);
                        }
                    }
                    return Ok(());
                }
                let mut matched = None;
                for fact in &evidence.source_facts {
                    budget.charge(WorkUsage {
                        reads: 1,
                        bytes: u64::try_from(std::mem::size_of::<OperationKey>())
                            .map_err(|_| IndexError::ResourceLimited)?,
                        ..Default::default()
                    })?;
                    if let Fact::Reservation {
                        target,
                        candidate_root_nonce,
                        ..
                    } = fact
                        && target.operation == operation.key
                    {
                        if *candidate_root_nonce != Some(root_nonce)
                            || matched.replace(target).is_some()
                        {
                            return Err(IndexError::DigestConflict);
                        }
                    }
                }
                // Only lexical SourceAdvance reaches this path. Published AST
                // was checked against its own signed work and physical request.
                let target = matched.ok_or(IndexError::DigestConflict)?;
                let mut request = None;
                for fact in &evidence.source_facts {
                    budget.charge(WorkUsage {
                        reads: 1,
                        bytes: u64::try_from(std::mem::size_of::<OperationKey>())
                            .map_err(|_| IndexError::ResourceLimited)?,
                        ..Default::default()
                    })?;
                    if let Fact::Replay {
                        native_update_request: Some(next),
                        phase,
                        ..
                    } = fact
                        && next.operation == operation.key
                    {
                        if !matches!(
                            phase,
                            NativeCutoverReplayPhaseV1::Acked | NativeCutoverReplayPhaseV1::Cleaned
                        ) || request.replace(next).is_some()
                        {
                            return Err(IndexError::DigestConflict);
                        }
                    }
                }
                let request = request.ok_or(IndexError::DigestConflict)?;
                let identified = quanta_index_kernel::identify_update_request_with(
                    request,
                    budget,
                    |payload, children, budget| reader.identify(payload, children, budget),
                )?;
                quanta_index_kernel::verify_native_published_source_target_v1(
                    target,
                    request,
                    operation,
                    publication.identity,
                    publication.profile,
                    identified,
                    budget,
                )?;
                published_count = published_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                if snapshot
                    .selected
                    .as_ref()
                    .is_some_and(|selected| selected.operation.key == operation.key)
                {
                    found_boot = true;
                }
                if publication.identity == head.identity {
                    if found_head.replace(operation.key).is_some() {
                        return Err(IndexError::DigestConflict);
                    }
                }
                Ok(())
            },
        )?;
        drop(reader);
        if observed.snapshot.root_nonce != self.observed.snapshot.root_nonce
            || observed.current_head != self.observed.current_head
            || published_count == 0
            || published_count != source_count.checked_add(ast_count)
                .ok_or(IndexError::ResourceLimited)?
            || !found_boot
            || found_head != Some(latest_operation)
        {
            return Err(IndexError::DigestConflict.into());
        }
        require_retired(&self.authority)?;
        self.authority.lease.require_state_root_v1(root)?;
        budget.checkpoint()?;
        Ok(PreviousNativeSourceRowsV1 {
            root_nonce,
            old_head: observed.current_head.identity,
            old_head_operation: latest_operation,
            published_count,
        })
    }

    pub(in crate::native_product::boot) fn require_current_and_retired_v1(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<()> {
        let current = read_selected_config_v1(&self.selected.command, budget)?;
        let config = &current.config;
        if config.state_root != self.authority.lease.state_root_identity_v1() {
            return Err(IndexError::DigestConflict.into());
        }
        let expected = self
            .selected
            .command
            .identity
            .ok_or(IndexError::SnapshotUnavailable)?;
        let registry = super::super::super::compose_boot_registry(config)?;
        let observed = verify_native_source_root_quiescence_v1(
            self.authority.lease.state_root_identity_v1(),
            self.authority.io.as_ref(),
            expected.manifest_operation.workspace,
            budget,
        )?;
        if observed.snapshot.root_nonce != self.observed.snapshot.root_nonce
            || observed.current_head != self.observed.current_head
        {
            return Err(IndexError::DigestConflict.into());
        }
        Self::check_selected_v1(
            &self.authority,
            &observed,
            expected,
            config,
            &registry,
            budget,
        )
    }
}
