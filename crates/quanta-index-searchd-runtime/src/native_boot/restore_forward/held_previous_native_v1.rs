//! Read the selected, offline Native root under its own retained physical owner.
//! The observation is DATA; only a joined Source replay horizon can authorize a switch.

use super::*;
use quanta_index_catalog::{
    NativeBootPublicationReader, NativeBootQuiescentCatalogV1, verify_native_boot_catalog,
    verify_native_source_root_publications_v1,
    verify_native_source_root_publications_with_routes_v1, verify_native_source_root_quiescence_v1,
    verify_native_source_root_quiescence_with_routes_v1,
};
use quanta_index_contract::{
    native_cutover_evidence_v1::{
        MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1, NativeCutoverAstWorkPhaseV1,
        NativeCutoverEvidenceSourceFactV1 as Fact, NativeCutoverEvidenceV1,
        NativeCutoverReplayPhaseV1,
    },
    native_cutover_source_join_v1::{
        NativeCutoverJoinedSourceFactsV1, NativeCutoverPublicationFactsV1,
        NativeCutoverSourceJoinErrorV1,
    },
};
use quanta_index_core::index::{
    MemoryLease, MemoryLifetime, NativeProtectedSourceRoutesV1, OperationKey, ViewRef, WorkUsage,
    with_native_protected_source_routes_v1,
};
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

pub(crate) struct HeldPreviousNativeV1 {
    // Drop the funded Catalog snapshot before its physical read owner.
    observed: NativeBootQuiescentCatalogV1,
    authority: NativeBootAuthority,
    selected: FundedSelectedConfigV1,
    selection_sha256: quanta_index_core::index::Digest,
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
    pub(crate) fn selected_config_v1(&self) -> &super::super::super::ValidatedConfig {
        &self.selected.config
    }

    pub(crate) fn selection_sha256_v1(&self) -> quanta_index_core::index::Digest {
        self.selection_sha256
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
                let root = authority.lease.state_root_identity_v1();
                let initial = verify_native_boot_catalog(root, authority.io.as_ref(), budget)?;
                let observed = if let Some(route) = initial
                    .selected
                    .as_ref()
                    .and_then(|selected| selected.protected_source_routes)
                {
                    let objects = root.join("native-objects");
                    let io = Arc::new(NativeObjectReadIo::new(Arc::clone(&authority.physical)));
                    let lock = io.open(&objects.join(".object-namespace.lease"), budget)?;
                    let reader = FileObjectReader::open_existing(&objects, lock, io, budget)?;
                    if Some(reader.namespace_binding()?) != initial.namespace {
                        return Err(IndexError::DigestConflict.into());
                    }
                    let stored = reader.get(route.object, budget)?;
                    with_native_protected_source_routes_v1(
                        route.object,
                        Some(route.count),
                        stored,
                        budget,
                        |routes, budget| {
                            let observed = verify_native_source_root_quiescence_with_routes_v1(
                                root,
                                authority.io.as_ref(),
                                expected.manifest_operation.workspace,
                                routes,
                                budget,
                            )?;
                            if observed.snapshot.root_nonce != initial.root_nonce
                                || observed.snapshot.selected != initial.selected
                                || observed.snapshot.namespace != initial.namespace
                            {
                                return Err(IndexError::DigestConflict);
                            }
                            Ok(observed)
                        },
                    )?
                } else {
                    verify_native_source_root_quiescence_v1(
                        root,
                        authority.io.as_ref(),
                        expected.manifest_operation.workspace,
                        budget,
                    )?
                };
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
            selection_sha256: selected.fingerprint_v1(),
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
        if let Some(tree) = config.retained_tree() {
            authenticate_retained_source_v20(
                authority,
                &observed.snapshot,
                selected,
                registry,
                config.resources.physical.inventory.max_entries,
                tree,
                budget,
            )?;
        } else {
            authenticate(
                authority,
                &observed.snapshot,
                selected,
                registry,
                config.resources.physical.inventory.max_entries,
                None,
                false,
                false,
                budget,
            )?;
        }
        require_retired(authority)?;
        authority.lease.require_state_root_v1(&config.state_root)?;
        budget.checkpoint()?;
        Ok(())
    }

    pub(crate) fn observed_v1(&self) -> &NativeBootQuiescentCatalogV1 {
        &self.observed
    }

    pub(crate) fn catalog_io_v1(&self) -> &dyn quanta_index_catalog::CatalogReadOnlyIoAdmission {
        self.authority.io.as_ref()
    }

    pub(in crate::native_product::boot) fn native_io_v1(
        &self,
    ) -> &crate::native_scratch_io::NativeScratchIo {
        self.authority.io.as_ref()
    }

    fn with_route_object_from_reader_v1<T>(
        &self,
        reader: &FileObjectReader,
        budget: &mut WorkBudget,
        visit: impl FnOnce(&NativeProtectedSourceRoutesV1, &mut WorkBudget) -> Result<T>,
    ) -> Result<T> {
        let selected = self
            .observed
            .snapshot
            .selected
            .as_ref()
            .ok_or(IndexError::SnapshotUnavailable)?;
        let route = selected
            .protected_source_routes
            .ok_or(IndexError::SnapshotUnavailable)?;
        if Some(reader.namespace_binding()?) != self.observed.snapshot.namespace {
            return Err(IndexError::DigestConflict.into());
        }
        let stored = reader.get(route.object, budget)?;
        with_native_protected_source_routes_v1(
            route.object,
            Some(route.count),
            stored,
            budget,
            |routes, budget| Ok(visit(routes, budget)),
        )?
    }

    pub(crate) fn with_protected_routes_v1<T>(
        &self,
        budget: &mut WorkBudget,
        visit: impl FnOnce(
            &NativeProtectedSourceRoutesV1,
            &FileObjectReader,
            &mut WorkBudget,
        ) -> Result<T>,
    ) -> Result<T> {
        self.with_protected_routes_and_quiescence_v1(budget, |routes, reader, _, budget| {
            visit(routes, reader, budget)
        })
    }

    /// Lend the selected current Source under the retained old root even when
    /// BOOT has no additional protected historical route.
    pub(crate) fn with_current_source_reader_v1<T>(
        &self,
        budget: &mut WorkBudget,
        visit: impl FnOnce(
            &quanta_index_core::index::CatalogView,
            &FileObjectReader,
            &mut WorkBudget,
        ) -> Result<T>,
    ) -> Result<T> {
        if self
            .observed
            .snapshot
            .selected
            .as_ref()
            .and_then(|selected| selected.protected_source_routes)
            .is_some()
        {
            return self.with_protected_routes_v1(budget, |_routes, reader, budget| {
                visit(&self.observed.current_head, reader, budget)
            });
        }
        let root = self.authority.lease.state_root_identity_v1();
        let objects = root.join("native-objects");
        let io = Arc::new(NativeObjectReadIo::new(Arc::clone(
            &self.authority.physical,
        )));
        let lock = io.open(&objects.join(".object-namespace.lease"), budget)?;
        let reader = FileObjectReader::open_existing(&objects, lock, io, budget)?;
        if self.observed.snapshot.namespace != Some(reader.namespace_binding()?) {
            return Err(IndexError::DigestConflict.into());
        }
        let workspace = self
            .observed
            .snapshot
            .selected
            .as_ref()
            .ok_or(IndexError::SnapshotUnavailable)?
            .operation
            .key
            .workspace;
        let current = verify_native_source_root_quiescence_v1(
            root,
            self.authority.io.as_ref(),
            workspace,
            budget,
        )?;
        if current.snapshot.root_nonce != self.observed.snapshot.root_nonce
            || current.snapshot.selected != self.observed.snapshot.selected
            || current.snapshot.namespace != self.observed.snapshot.namespace
            || current.current_head != self.observed.current_head
        {
            return Err(IndexError::DigestConflict.into());
        }
        visit(&self.observed.current_head, &reader, budget)
    }

    fn with_protected_routes_and_quiescence_v1<T>(
        &self,
        budget: &mut WorkBudget,
        visit: impl FnOnce(
            &NativeProtectedSourceRoutesV1,
            &FileObjectReader,
            NativeBootQuiescentCatalogV1,
            &mut WorkBudget,
        ) -> Result<T>,
    ) -> Result<T> {
        let root = self.authority.lease.state_root_identity_v1();
        let objects = root.join("native-objects");
        let io = Arc::new(NativeObjectReadIo::new(Arc::clone(
            &self.authority.physical,
        )));
        let lock = io.open(&objects.join(".object-namespace.lease"), budget)?;
        let reader = FileObjectReader::open_existing(&objects, lock, io, budget)?;
        self.with_route_object_from_reader_v1(&reader, budget, |routes, budget| {
            let workspace = self
                .observed
                .snapshot
                .selected
                .as_ref()
                .ok_or(IndexError::SnapshotUnavailable)?
                .operation
                .key
                .workspace;
            let observed = verify_native_source_root_quiescence_with_routes_v1(
                root,
                self.authority.io.as_ref(),
                workspace,
                routes,
                budget,
            )?;
            if observed.snapshot.root_nonce != self.observed.snapshot.root_nonce
                || observed.snapshot.selected != self.observed.snapshot.selected
                || observed.snapshot.namespace != self.observed.snapshot.namespace
                || observed.current_head != self.observed.current_head
            {
                return Err(IndexError::DigestConflict.into());
            }
            visit(routes, &reader, observed, budget)
        })
    }

    pub(crate) fn visit_history_members_v1(
        &self,
        repo: Option<&quanta_index_contract::RepoId>,
        budget: &mut WorkBudget,
        mut visit: impl FnMut(
            &quanta_index_contract::index::NativeHistoryRestoreInputV1,
            &FileObjectReader,
            ObjectRef,
            &mut WorkBudget,
        ) -> Result<()>,
    ) -> Result<u64> {
        use quanta_index_core::index::{InputDomain, decode_member_binding, member_map_key};

        let view = &self.observed.current_head;
        let root = self.authority.lease.state_root_identity_v1();
        let objects = root.join("native-objects");
        let io = Arc::new(NativeObjectReadIo::new(Arc::clone(
            &self.authority.physical,
        )));
        let lock = io.open(&objects.join(".object-namespace.lease"), budget)?;
        let reader = FileObjectReader::open_existing(&objects, lock, io, budget)?;
        if Some(reader.namespace_binding()?) != self.observed.snapshot.namespace {
            return Err(IndexError::DigestConflict.into());
        }
        let profile_object = reader.get(view.profile, budget)?;
        if !profile_object.registration.children.is_empty() {
            return Err(IndexError::DigestConflict.into());
        }
        let size = u64::try_from(profile_object.payload.len())?
            .checked_mul(32)
            .and_then(|size| size.checked_add(4096))
            .ok_or(IndexError::ResourceLimited)?;
        let _profile_memory = budget
            .reserve_memory(size, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        let mut profile: quanta_index_core::index::IndexProfile =
            serde_json::from_slice(&profile_object.payload)?;
        if profile.instances.len()
            > quanta_index_contract::native_protocol::MAX_NATIVE_READ_PROFILE_INSTANCES
        {
            return Err(IndexError::DigestConflict.into());
        }
        // Persisted profiles need not be sorted. Census order is independent
        // from profile identity and cannot reject a valid unsorted profile.
        profile.instances.sort_unstable_by_key(|spec| spec.instance);
        if profile
            .instances
            .windows(2)
            .any(|pair| pair[0].instance == pair[1].instance)
        {
            return Err(IndexError::DigestConflict.into());
        }
        let mut count = 0_u64;
        for instance in &profile.instances {
            budget.checkpoint()?;
            if instance.input_domain != InputDomain::History {
                continue;
            }
            let key = member_map_key(instance.instance);
            let reference = reader
                .lookup_map_object(view.members, &key, budget)?
                .ok_or(IndexError::SnapshotUnavailable)?;
            let object = reader.get(reference, budget)?;
            let member = decode_member_binding(&object.payload, budget)?;
            if member.instance != instance.instance
                || member.format != instance.format
                || member.input.schema != instance.schema
                || member.input.policy != instance.policy
                || !object.registration.children.iter().copied().eq([
                    Some(member.artifact),
                    member.input.root.0,
                ]
                .into_iter()
                .flatten())
            {
                return Err(IndexError::DigestConflict.into());
            }
            quanta_index_search_plane::native_history::with_native_history_restore_member_v1(
                member.artifact,
                instance,
                |reference, budget| reader.get(reference, budget),
                |root, after, limit, budget| reader.scan_map(root, after, limit, budget),
                budget,
                |cut, rows, source_input, budget| {
                    if member.input.context
                        != cut
                            .input_context_v1()
                            .map_err(|message| IndexError::InvalidInput(message.into()))?
                        || member.logical_commitment != cut.snapshot_digest
                        || member.input.root != rows
                    {
                        return Err(IndexError::DigestConflict);
                    }
                    if repo.is_some_and(|repo| repo != &cut.generation.repo_id) {
                        return Ok(());
                    }
                    let bytes = cut
                        .generation
                        .repo_id
                        .as_str()
                        .len()
                        .checked_add(cut.generation.revision_id.as_str().len())
                        .and_then(|n| u64::try_from(n).ok())
                        .ok_or(IndexError::ResourceLimited)?;
                    let _cut_memory = budget
                        .reserve_memory(bytes, MemoryLifetime::Temporary)?
                        .ok_or(IndexError::ResourceLimited)?;
                    let input = quanta_index_contract::index::NativeHistoryRestoreInputV1 {
                        wire_version: 3,
                        source_root_nonce: quanta_index_core::index::Digest(
                            self.observed.snapshot.root_nonce,
                        ),
                        source_view: view.identity,
                        instance: instance.instance,
                        member: member.artifact,
                        cut: cut.clone(),
                    };
                    visit(&input, &reader, source_input, budget).map_err(|cause| {
                        cause
                            .downcast::<IndexError>()
                            .unwrap_or_else(|cause| IndexError::Storage(cause.to_string()))
                    })?;
                    count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    Ok(())
                },
            )?;
        }
        self.require_current_and_retired_v1(budget)?;
        Ok(count)
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
                let cursor = (target.operation.stream_epoch, target.event_sequence);
                if latest_source.is_none_or(|(prior, _): ((u64, u64), OperationKey)| cursor > prior)
                {
                    latest_source = Some((cursor, target.operation));
                }
            }
        }
        let (_, latest_operation) = latest_source.ok_or(IndexError::SnapshotUnavailable)?;
        let count = evidence.source_facts.len();
        let bytes = count
            .checked_mul(std::mem::size_of::<usize>())
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(IndexError::ResourceLimited)?;
        let facts_memory = budget
            .reserve_memory(bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        let mut fact_rows = Vec::new();
        fact_rows
            .try_reserve_exact(count)
            .map_err(|_| IndexError::ResourceLimited)?;
        if fact_rows.capacity() > count {
            return Err(IndexError::ResourceLimited.into());
        }
        let publication_facts = NativeCutoverPublicationFactsV1::from_facts_v1(
            &evidence.source_facts,
            &mut fact_rows,
            &mut |bytes| {
                budget.charge(WorkUsage {
                    reads: 1,
                    bytes,
                    ..Default::default()
                })
            },
        )
        .map_err(|cause| match cause {
            NativeCutoverSourceJoinErrorV1::Refused(_) => IndexError::DigestConflict,
            NativeCutoverSourceJoinErrorV1::Work(cause) => cause,
        })?;
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
        let mut history_count = 0_u64;
        let mut sibling_count = 0_u64;
        let mut found_boot = false;
        let mut found_head = None;
        let mut visit = |snapshot: &quanta_index_catalog::NativeBootCatalogSnapshot,
                         publication_reader: &NativeBootPublicationReader<'_>,
                         head: &quanta_index_core::index::CatalogView,
                         operation: &quanta_index_core::index::ImmutableOperation,
                         publication: &quanta_index_core::index::CatalogView,
                         budget: &mut WorkBudget| {
            if snapshot.root_nonce != self.observed.snapshot.root_nonce
                || snapshot.selected != self.observed.snapshot.selected
                || snapshot.namespace != self.observed.snapshot.namespace
            {
                return Err(IndexError::DigestConflict);
            }
            if operation.key.workspace != latest_operation.workspace {
                // The route-aware Catalog cursor admits only workspaces
                // whose exact selected head/basis have been authenticated by
                // check_selected_v1. Historical terminal requests can already
                // be reclaimed; their canonical persisted ledger is copied
                // and independently compared by preserved_native_v1.
                if publication.identity.workspace != operation.key.workspace
                    || head.identity.workspace != operation.key.workspace
                {
                    return Err(IndexError::DigestConflict);
                }
                sibling_count = sibling_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                published_count = published_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                return Ok(());
            }
            if head != &self.observed.current_head {
                return Err(IndexError::DigestConflict);
            }
            let matching_facts = publication_facts.for_operation_v1(operation.key);
            let mut ast = None;
            for fact in matching_facts.clone() {
                budget.charge(WorkUsage {
                    reads: 1,
                    bytes: u64::try_from(std::mem::size_of::<OperationKey>())
                        .map_err(|_| IndexError::ResourceLimited)?,
                    ..Default::default()
                })?;
                if let Fact::AstWork {
                    candidate_root_nonce,
                    phase: NativeCutoverAstWorkPhaseV1::Published,
                    source_operation,
                    source_view,
                    source_cut,
                    published_view: Some(view),
                    request: Some(request),
                    ..
                } = fact
                {
                    if *candidate_root_nonce != Some(root_nonce)
                        || ast
                            .replace((*source_operation, *source_view, *source_cut, *view, request))
                            .is_some()
                    {
                        return Err(IndexError::DigestConflict);
                    }
                }
            }
            if let Some((source_operation, source_view, source_cut, ast_view, request)) = ast {
                let source = publication_reader.published_operation(source_operation, budget)?;
                if source.identity != source_view
                    || source.source != source_cut
                    || publication.identity != ast_view
                    || publication.source != source_cut
                {
                    return Err(IndexError::DigestConflict);
                }
                let max_pending = u32::try_from(
                    self.selected
                        .config
                        .resources
                        .physical
                        .inventory
                        .max_entries,
                )
                .map_err(|_| IndexError::ResourceLimited)?;
                let [stored_request, _profile] = reader.verify_closure_with_selected_roots(
                    &[operation.request, publication.profile],
                    [operation.request, publication.profile],
                    max_pending,
                    budget,
                )?;
                quanta_index_kernel::verify_native_boot_captured_request_v1(
                    request,
                    operation,
                    &stored_request,
                    budget,
                )?;
                ast_count = ast_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
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
                    if found_head.replace(source_operation).is_some() {
                        return Err(IndexError::DigestConflict);
                    }
                }
                return Ok(());
            }
            let mut matched = None;
            for fact in matching_facts.clone() {
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
                {
                    if *candidate_root_nonce != Some(root_nonce)
                        || matched.replace(target).is_some()
                    {
                        return Err(IndexError::DigestConflict);
                    }
                }
            }
            // History-only publications do not issue Source events. Their
            // immutable request must preserve the exact prior Source and
            // projection roots; every Source publication still requires
            // the signed reservation/replay correspondence below.
            let Some(target) = matched else {
                let maximum = u32::try_from(
                    self.selected
                        .config
                        .resources
                        .physical
                        .inventory
                        .max_entries,
                )
                .map_err(|_| IndexError::ResourceLimited)?;
                let [stored_request, profile] = reader.verify_closure_with_selected_roots(
                    &[operation.request, publication.profile],
                    [operation.request, publication.profile],
                    maximum,
                    budget,
                )?;
                quanta_index_kernel::with_native_boot_history_request_v1(
                    operation,
                    publication,
                    &stored_request,
                    budget,
                    |request, budget| {
                        let prior =
                            publication_reader.published_view(request.expected_view, budget)?;
                        if prior.source != publication.source
                            || prior.projections != publication.projections
                        {
                            return Err(IndexError::DigestConflict);
                        }
                        match &request.kind {
                            quanta_index_core::index::UpdateKind::HistoryAdvance(transition)
                                if transition.expected.is_none() =>
                            {
                                let previous_profile = reader.get(prior.profile, budget)?;
                                quanta_index_kernel::with_native_boot_history_full_v1(
                                    &prior,
                                    operation,
                                    publication,
                                    &stored_request,
                                    &previous_profile,
                                    &profile,
                                    budget,
                                    |_, _| Ok(()),
                                )?;
                            }
                            quanta_index_core::index::UpdateKind::HistoryAdvance(_)
                            | quanta_index_core::index::UpdateKind::HistoryAppend(_)
                                if prior.profile == publication.profile => {}
                            _ => return Err(IndexError::DigestConflict),
                        }
                        Ok(())
                    },
                )?;
                history_count = history_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
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
                    let source =
                        publication_reader.published_operation(latest_operation, budget)?;
                    if source.source != publication.source
                        || found_head.replace(latest_operation).is_some()
                    {
                        return Err(IndexError::DigestConflict);
                    }
                }
                return Ok(());
            };
            let mut request = None;
            for fact in matching_facts {
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
        };
        let observed = if self
            .observed
            .snapshot
            .selected
            .as_ref()
            .and_then(|selected| selected.protected_source_routes)
            .is_some()
        {
            self.with_route_object_from_reader_v1(&reader, budget, |routes, budget| {
                Ok(verify_native_source_root_publications_with_routes_v1(
                    root,
                    self.authority.io.as_ref(),
                    latest_operation.workspace,
                    routes,
                    budget,
                    |snapshot, publication_reader, head, operation, publication, budget| {
                        visit(
                            snapshot,
                            publication_reader,
                            head,
                            operation,
                            publication,
                            budget,
                        )
                    },
                )?)
            })?
        } else {
            verify_native_source_root_publications_v1(
                root,
                self.authority.io.as_ref(),
                latest_operation.workspace,
                budget,
                |snapshot, publication_reader, head, operation, publication, budget| {
                    visit(
                        snapshot,
                        publication_reader,
                        head,
                        operation,
                        publication,
                        budget,
                    )
                },
            )?
        };
        drop(fact_rows);
        drop(facts_memory);
        drop(reader);
        if observed.snapshot.root_nonce != self.observed.snapshot.root_nonce
            || observed.snapshot.selected != self.observed.snapshot.selected
            || observed.snapshot.namespace != self.observed.snapshot.namespace
            || observed.current_head != self.observed.current_head
            || published_count == 0
            || published_count
                != source_count
                    .checked_add(ast_count)
                    .and_then(|count| count.checked_add(history_count))
                    .and_then(|count| count.checked_add(sibling_count))
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
        let observed = if self
            .observed
            .snapshot
            .selected
            .as_ref()
            .and_then(|selected| selected.protected_source_routes)
            .is_some()
        {
            self.with_protected_routes_and_quiescence_v1(budget, |_, _, observed, _| Ok(observed))?
        } else {
            verify_native_source_root_quiescence_v1(
                self.authority.lease.state_root_identity_v1(),
                self.authority.io.as_ref(),
                expected.manifest_operation.workspace,
                budget,
            )?
        };
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
