//! Selection, physical authentication and same-lease service handoff.
//! Catalog is the sole selection issuer; no second durable head is introduced.

#[path = "native_boot/catalog_digest.rs"]
mod catalog_digest;
#[path = "native_boot/receipt.rs"]
mod receipt;
#[path = "native_boot/restore_forward.rs"]
mod restore_forward;

pub(crate) use restore_forward::{RestoreForwardSourceRefusalV1, HeldPreviousNativeV1};
pub(super) use restore_forward::publish_v1 as publish_restore_forward_under_locked_selection_v1;

use std::{fs, sync::Arc};

use anyhow::{Result, anyhow, bail};
use quanta_index_catalog::{
    NativeBootManifest, SqliteCatalog, verify_native_boot_candidate, verify_native_boot_catalog,
    verify_native_boot_current_candidate, verify_native_legacy_history_recovery_publications_v1,
    verify_native_rollback_with_source_publications,
};
use quanta_index_contract::native_cutover_evidence_v1::{
    NativeCutoverControlTerminalV1, NativeCutoverInstalledHistoryV1,
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        IndexError, MemoryLease, ObjectReadIoPort, ObjectRef, RegisteredObject, WorkBudget,
        effort::ReservationGuard,
    },
};
use quanta_index_kernel::{AdapterRegistry, verify_native_boot_inputs};
use quanta_index_searchd::{
    app::runtime::StateRootLease,
    cli::{NativeBootIdentityV1, NativeCommandModeV1, NativeStateCommandV1},
};
use quanta_index_storage::FileObjectReader;

use super::ValidatedConfig;
use crate::deployment_selection::{
    DeploymentSelectionErrorV1, DeploymentSelectionPublishedV1, LockedDeploymentSelectionV1,
};
use crate::{
    native::{
        NativeBootAuthority, prepare_native_boot_authority, prepare_native_recovery_boot_authority,
        verify_existing_native_format,
    },
    native_object_read_io::NativeObjectReadIo,
    native_resources::{NativeResourceClass, NativeResourceLedger},
};

fn identity(nonce: [u8; 32], manifest: &NativeBootManifest) -> Result<NativeBootIdentityV1> {
    Ok(NativeBootIdentityV1 {
        root_nonce: nonce,
        manifest_operation: manifest.operation.key,
        manifest_object: manifest.operation.request,
        source_cut_digest: manifest.source_cut_digest()?,
        protected_source_routes: manifest.protected_source_routes.map(|routes| routes.object),
    })
}

/// Borrowed Q expectations from the already joined Original producer DATA.
/// This is neither a producer permit nor an independently usable root owner.
struct RecoverySourceInputs<'a> {
    target: &'a quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceTargetV1,
    // Selected original FULL scope, copied only from the canonical Source join.
    baseline_candidate_root_nonce: Option<quanta_index_contract::index::Digest>,
    request: &'a quanta_index_core::index::UpdateRequest,
    // Exact current Native reservation, independently borrowed from Source.
    // These fields must not silently alias the historical FULL accessor.
    current_target: &'a quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceTargetV1,
    current_request: &'a quanta_index_core::index::UpdateRequest,
    legacy_publication: Option<&'a quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceLegacyPublicationV1>,
    joined_source: Option<&'a quanta_index_contract::native_cutover_source_join_v1::NativeCutoverJoinedSourceFactsV1<'a>>,
    ast_work: Option<RecoveryAstInputs<'a>>,
}

/// Only the fields used by Q, borrowed from the already joined AST record.
/// This private projection cannot issue Source or publication authority.
struct RecoveryAstInputs<'a> {
    operation: quanta_index_core::index::OperationKey,
    publication: quanta_index_core::index::ViewRef,
    source_view: quanta_index_core::index::ViewRef,
    source_cut: quanta_index_core::index::SourceCut,
    request: &'a quanta_index_core::index::UpdateRequest,
}

fn joined_ast_inputs(
    fact: Option<
        &quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceSourceFactV1,
    >,
) -> Result<Option<RecoveryAstInputs<'_>>> {
    use quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceSourceFactV1 as Fact;
    match fact {
        None => Ok(None),
        Some(Fact::AstWork {
            ast_operation: Some(operation),
            published_view: Some(publication),
            source_view,
            source_cut,
            request: Some(request),
            ..
        }) => Ok(Some(RecoveryAstInputs {
            operation: *operation,
            publication: *publication,
            source_view: *source_view,
            source_cut: *source_cut,
            request,
        })),
        Some(_) => Err(IndexError::SnapshotUnavailable.into()),
    }
}

struct RecoveryCapture<'a, 'facts> {
    source: &'a RecoverySourceInputs<'facts>,
    full: &'a quanta_index_catalog::NativeBootPublishedOperation,
}

#[derive(Clone, Copy)]
struct LegacyHistoryRecoveryV1<'a> {
    legacy: &'a legacy_read_factory::LegacyReadAuthorityV1,
    producer: quanta_index_catalog::LegacyHistoryProducerScopeV1<'a>,
}

pub(crate) struct FundedInstalledHistoryV1 {
    pub(crate) value: NativeCutoverInstalledHistoryV1,
    pub(crate) _memory: Option<MemoryLease>,
}

pub(super) struct FundedNativeRestoreInputV1 {
    pub(super) value: quanta_index_contract::index::NativeHistoryRestoreInputV1,
    pub(super) _memory: Option<MemoryLease>,
}

pub(crate) struct RestoreForwardHistoryV1<'a> {
    pub(crate) retained_terminal: Option<&'a NativeCutoverControlTerminalV1>,
    pub(crate) selected: &'a mut Option<FundedInstalledHistoryV1>,
}

impl RestoreForwardHistoryV1<'_> {
    fn matches_retained_v1(&self, selected: Option<&NativeCutoverInstalledHistoryV1>) -> bool {
        use NativeCutoverControlTerminalV1 as Terminal;
        match self.retained_terminal {
            Some(
                terminal @ (Terminal::Applied { .. }
                | Terminal::Reconciled { .. }
                | Terminal::AppliedHistory { .. }
                | Terminal::ReconciledHistory { .. }),
            ) => terminal.installed_history_v1() == selected,
            _ => true,
        }
    }
}

/// Keep the saved Native selection, physical root lease and Catalog snapshot
/// alive while an installed producer borrows its immutable History inventory.
pub(crate) fn with_selected_native_history_v1<T>(
    selection: &crate::deployment_selection::DeploymentSelectionDocumentV1,
    budget: &mut WorkBudget,
    read: impl FnOnce(&restore_forward::HeldPreviousNativeV1, &mut WorkBudget) -> Result<T>,
) -> Result<T> {
    let held = restore_forward::HeldPreviousNativeV1::acquire_v1(selection, budget)?;
    let result = read(&held, budget)?;
    held.require_current_and_retired_v1(budget)?;
    Ok(result)
}

fn authenticate(
    authority: &NativeBootAuthority,
    snapshot: &quanta_index_catalog::NativeBootCatalogSnapshot,
    manifest: &NativeBootManifest,
    registry: &AdapterRegistry,
    maximum: u64,
    recovery: Option<&RecoveryCapture<'_, '_>>,
    require_current_routes: bool,
    budget: &mut WorkBudget,
) -> Result<()> {
    authenticate_with_legacy_history_v1(
        authority,
        snapshot,
        manifest,
        registry,
        maximum,
        recovery,
        None,
        None,
        require_current_routes,
        budget,
    )
    .map(|_| ())
}

fn authenticate_with_legacy_history_v1(
    authority: &NativeBootAuthority,
    snapshot: &quanta_index_catalog::NativeBootCatalogSnapshot,
    manifest: &NativeBootManifest,
    registry: &AdapterRegistry,
    maximum: u64,
    recovery: Option<&RecoveryCapture<'_, '_>>,
    legacy_history: Option<LegacyHistoryRecoveryV1<'_>>,
    native_history: Option<&[FundedNativeRestoreInputV1]>,
    require_current_routes: bool,
    budget: &mut WorkBudget,
) -> Result<Option<FundedInstalledHistoryV1>> {
    let root = authority.lease.state_root_identity_v1();
    if let Some(capture) = recovery {
        budget.charge(quanta_index_core::index::WorkUsage {
            bytes: 64,
            ..Default::default()
        })?;
        let expected = capture
            .source
            .baseline_candidate_root_nonce
            .ok_or(IndexError::SnapshotUnavailable)?;
        // Operation/SourceCut/object equality cannot identify an incarnation.
        // This stamp was read under the actual held Q lease and transaction.
        if expected.0 != snapshot.root_nonce {
            return Err(IndexError::DigestConflict.into());
        }
    }
    let object_root = root.join("native-objects");
    let io = Arc::new(NativeObjectReadIo::new(Arc::clone(&authority.physical)));
    let lock = io.open(&object_root.join(".object-namespace.lease"), budget)?;
    let reader = FileObjectReader::open_existing(&object_root, lock, io, budget)?;
    if snapshot.namespace != Some(reader.namespace_binding()?) {
        return Err(IndexError::DigestConflict.into());
    }
    let mut roots = [manifest.operation.request; 6];
    let mut count = 0usize;
    for root in manifest.all_protected_roots() {
        *roots.get_mut(count).ok_or(IndexError::ResourceLimited)? = root;
        count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    // Every pending reference comes from one extant immutable edge. The explicit
    // whole-root entry ceiling also bounds this separately admitted frontier.
    let maximum = u32::try_from(maximum).map_err(|_error| IndexError::ResourceLimited)?;
    let [request, profile] = reader.verify_closure_with_selected_roots(
        roots.get(..count).ok_or(IndexError::ResourceLimited)?,
        [manifest.operation.request, manifest.publication.profile],
        maximum,
        budget,
    )?;
    if let Some(route) = manifest.protected_source_routes {
        let stored = reader.get(route.object, budget)?;
        quanta_index_core::index::with_native_protected_source_routes_v1(
            route.object,
            Some(route.count),
            &stored,
            budget,
            |routes, budget| {
                quanta_index_catalog::verify_native_protected_source_routes_v1(
                    root,
                    authority.io.as_ref(),
                    manifest,
                    routes,
                    budget,
                )?;
                if require_current_routes {
                    quanta_index_catalog::verify_native_protected_source_routes_if_current_v1(
                        root,
                        authority.io.as_ref(),
                        manifest,
                        routes,
                        budget,
                    )?;
                }
                Ok(())
            },
        )?;
    }
    verify_native_boot_inputs(
        &manifest.operation,
        &manifest.publication,
        &request,
        &profile,
        registry,
        budget,
    )?;
    let installed_history = if let Some(capture) = recovery {
        authenticate_recovery_capture(
            &reader,
            manifest,
            &request,
            &profile,
            capture,
            legacy_history,
            native_history,
            maximum,
            budget,
        )?
    } else {
        None
    };
    drop(profile);
    drop(request);
    drop(reader);
    authority.physical.sweep_retired()?;
    authority.lease.require_state_root_v1(root)?;
    budget.checkpoint()?;
    Ok(installed_history)
}

/// Borrow only metadata actually present in this Catalog observation.
/// Same-key initial FULL is deduplication, never a historical/current alias.
fn current_native_publication_v1<'a>(
    selected: &'a NativeBootManifest,
    capture: &'a RecoveryCapture<'_, '_>,
) -> Result<(
    &'a quanta_index_core::index::ImmutableOperation,
    &'a quanta_index_core::index::CatalogView,
)> {
    let key = capture.source.current_target.operation;
    if let Some(actual) = capture.full.published_for(key) {
        return Ok(actual);
    }
    if selected.operation.key == key {
        return Ok((&selected.operation, &selected.publication));
    }
    // An AST selection cannot stand in for the independently keyed current
    // Native publication. That row must be retained by this same snapshot.
    Err(IndexError::SnapshotUnavailable.into())
}

fn authenticate_recovery_capture(
    reader: &FileObjectReader,
    selected: &NativeBootManifest,
    selected_request: &quanta_index_core::index::StoredObject,
    selected_profile: &quanta_index_core::index::StoredObject,
    capture: &RecoveryCapture<'_, '_>,
    legacy_history: Option<LegacyHistoryRecoveryV1<'_>>,
    native_history: Option<&[FundedNativeRestoreInputV1]>,
    maximum: u32,
    budget: &mut WorkBudget,
) -> Result<Option<FundedInstalledHistoryV1>> {
    let full_request = capture.source.request;
    // Bind SourceCut and profile to the actual historical FULL row, rather
    // than substituting fields from the selected derived publication.
    let full_publication = capture.full.publication_metadata();
    if full_publication.identity != capture.full.publication()
        || full_publication.profile != capture.source.target.expected_profile
    {
        return Err(IndexError::DigestConflict.into());
    }
    let identified = quanta_index_kernel::identify_update_request_with(
        full_request,
        budget,
        |payload, children, budget| reader.identify(payload, children, budget),
    )?;
    quanta_index_kernel::verify_native_published_full_target_v1(
        capture.source.target,
        full_request,
        capture.full.operation(),
        capture.full.publication(),
        full_publication.profile,
        identified,
        budget,
    )?;
    let (current_operation, current_publication) =
        current_native_publication_v1(selected, capture)?;
    let current_request = capture.source.current_request;
    let current_identified = if current_request == full_request {
        identified
    } else {
        quanta_index_kernel::identify_update_request_with(
            current_request,
            budget,
            |payload, children, budget| reader.identify(payload, children, budget),
        )?
    };
    quanta_index_kernel::verify_native_published_source_target_v1(
        capture.source.current_target,
        current_request,
        current_operation,
        current_publication.identity,
        current_publication.profile,
        current_identified,
        budget,
    )?;
    let has_history =
        legacy_history.is_some() || native_history.is_some_and(|expected| !expected.is_empty());
    if selected.publication.source != current_publication.source
        || (!has_history && selected.publication.profile != current_publication.profile)
    {
        return Err(IndexError::DigestConflict.into());
    }
    let history_prior = if let Some(ast) = capture.source.ast_work.as_ref() {
        let ast_request = ast.request;
        let (operation, publication) = if has_history {
            capture
                .full
                .published_for(ast.operation)
                .ok_or(IndexError::SnapshotUnavailable)?
        } else {
            (&selected.operation, &selected.publication)
        };
        if operation.key != ast.operation
            || publication.identity != ast.publication
            || ast.source_view != current_publication.identity
            || ast.source_cut != current_publication.source
            || publication.profile != current_publication.profile
            || publication.source != current_publication.source
        {
            return Err(IndexError::DigestConflict.into());
        }
        let quanta_index_core::index::UpdateKind::ProjectionAdvance(transition) = &ast_request.kind
        else {
            return Err(IndexError::DigestConflict.into());
        };
        // Projection `expected` is the prior projection binding. Its source
        // fence is `target_source`, which the canonical projection admission
        // requires to equal the current published SourceCut.
        if transition.target_source != selected.publication.source {
            return Err(IndexError::DigestConflict.into());
        }
        if has_history {
            let ast_object = reader.get(operation.request, budget)?;
            quanta_index_kernel::verify_native_boot_captured_request_v1(
                ast_request,
                operation,
                &ast_object,
                budget,
            )?;
        } else {
            quanta_index_kernel::verify_native_boot_captured_request_v1(
                ast_request,
                operation,
                selected_request,
                budget,
            )?;
        }
        publication
    } else {
        if !has_history {
            if selected.operation != *current_operation
                || selected.publication.identity != current_publication.identity
            {
                return Err(IndexError::DigestConflict.into());
            }
            quanta_index_kernel::verify_native_boot_captured_request_v1(
                current_request,
                &selected.operation,
                selected_request,
                budget,
            )?;
        }
        current_publication
    };
    let mut installed_history = None;
    if let Some(LegacyHistoryRecoveryV1 { legacy, producer }) = legacy_history {
        let publication = capture
            .source
            .legacy_publication
            .ok_or(MissingLegacyPublicationInputV1)?;
        let expected = &publication.activation.active.generation.lexical;
        let required = publication.history.as_ref();
        let mut found_required = required.is_none();
        let predecessors = capture.full.snapshot().history_predecessors();
        quanta_index_catalog::verify_legacy_history_source_inventory_v1(
            legacy.root(),
            legacy.catalog(),
            &expected.repo_id,
            producer,
            budget,
            |budget, verify| {
                let mut prior = history_prior;
                for step in predecessors.iter().chain(std::iter::once(selected)) {
                    let prior_profile = reader.get(prior.profile, budget)?;
                    let intermediate = if step.operation.key != selected.operation.key {
                        Some((
                            reader.get(step.operation.request, budget)?,
                            reader.get(step.publication.profile, budget)?,
                        ))
                    } else {
                        None
                    };
                    let (request, profile) = intermediate.as_ref().map_or(
                        (selected_request, selected_profile),
                        |(request, profile)| (request, profile),
                    );
                    quanta_index_kernel::with_native_boot_history_full_v1(
                        prior,
                        &step.operation,
                        &step.publication,
                        request,
                        &prior_profile,
                        profile,
                        budget,
                        |transition, budget| {
                            let stored = reader.get(transition.input, budget)?;
                            use quanta_index_catalog::LegacyHistorySourceProofV1;
                            use quanta_index_search_plane::native_history::NativeHistoryBootSourceV1;
                            let prepared = quanta_index_search_plane::native_history::with_native_history_boot_source_v1(
                                transition.input,
                                &stored,
                                budget,
                                |input, budget| {
                                    let matching_required = required.filter(|required| {
                                        required.generation == transition.next.generation
                                    });
                                    let (proof, proves_required) = match &input {
                                        NativeHistoryBootSourceV1::Replay(input) => {
                                            if let Some(required) = matching_required {
                                                for original in &input.publications {
                                                    budget.checkpoint()?;
                                                    found_required |= original.ack.durable_sequence
                                                        == required.durable_sequence
                                                        && original.ack.batch_digest == required.batch_digest;
                                                }
                                            }
                                            (LegacyHistorySourceProofV1::Replay(input), false)
                                        }
                                        NativeHistoryBootSourceV1::Snapshot(checkpoint) => (
                                            LegacyHistorySourceProofV1::Snapshot {
                                                checkpoint,
                                                required: matching_required,
                                            },
                                            matching_required.is_some(),
                                        ),
                                        NativeHistoryBootSourceV1::NativeRestore(_) => {
                                            return Err(IndexError::DigestConflict);
                                        }
                                    };
                                    verify(&transition.next, proof, budget)?;
                                    found_required |= proves_required;
                                    // Older FULLs must remain in the final member map even
                                    // when their original query views have retired.
                                    verify_installed_history_member_v1(
                                        reader,
                                        &selected.publication,
                                        transition,
                                        &stored.registration,
                                        &input,
                                        budget,
                                    )
                                },
                            )?;
                            drop(stored);
                            let member = prepared.verify_boot_rows_v1(
                                |reference, budget| reader.get(reference, budget),
                                |root, after, limit, budget| {
                                    reader.scan_map(root, after, limit, budget)
                                },
                                budget,
                            )?;
                            if step.operation.key == selected.operation.key {
                                installed_history = Some(fund_installed_history_v1(
                                    &step.operation,
                                    &selected.publication,
                                    transition,
                                    member,
                                    budget,
                                )?);
                            }
                            Ok(())
                        },
                    )?;
                    prior = &step.publication;
                }
                if !found_required {
                    return Err(IndexError::DigestConflict);
                }
                Ok(())
            },
        )?;
        let members = selected
            .publication
            .members
            .0
            .ok_or(IndexError::SnapshotUnavailable)?;
        reader.verify_closure(&[members], maximum, budget)?;
        if installed_history.is_none() {
            return Err(IndexError::SnapshotUnavailable.into());
        }
    }
    if let Some(expected) = native_history {
        use quanta_index_search_plane::native_history::NativeHistoryBootSourceV1;
        if !expected.is_empty() {
            let seen_bytes =
                u64::try_from(expected.len()).map_err(|_| IndexError::ResourceLimited)?;
            let _seen_memory = budget
                .reserve_memory(
                    seen_bytes,
                    quanta_index_core::index::MemoryLifetime::Temporary,
                )?
                .ok_or(IndexError::ResourceLimited)?;
            let mut seen = Vec::new();
            seen.try_reserve_exact(expected.len())
                .map_err(|_| IndexError::ResourceLimited)?;
            seen.resize(expected.len(), false);
            let mut prior = history_prior;
            for step in capture
                .full
                .snapshot()
                .history_predecessors()
                .iter()
                .chain(std::iter::once(selected))
            {
                let prior_profile = reader.get(prior.profile, budget)?;
                let intermediate = if step.operation.key != selected.operation.key {
                    Some((
                        reader.get(step.operation.request, budget)?,
                        reader.get(step.publication.profile, budget)?,
                    ))
                } else {
                    None
                };
                let (request, profile) = intermediate.as_ref().map_or(
                    (selected_request, selected_profile),
                    |(request, profile)| (request, profile),
                );
                quanta_index_kernel::with_native_boot_history_full_v1(
                    prior,
                    &step.operation,
                    &step.publication,
                    request,
                    &prior_profile,
                    profile,
                    budget,
                    |transition, budget| {
                        let index = expected
                            .binary_search_by(|old| {
                                old.value
                                    .cut
                                    .generation
                                    .repo_id
                                    .cmp(&transition.next.generation.repo_id)
                                    .then_with(|| {
                                        old.value.cut.generation.bootstrap_order_key_v1().cmp(
                                            &transition.next.generation.bootstrap_order_key_v1(),
                                        )
                                    })
                            })
                            .map_err(|_| IndexError::DigestConflict)?;
                        let old = &expected[index];
                        if old.value.instance != transition.instance {
                            return Err(IndexError::DigestConflict);
                        }
                        if seen[index] || transition.next != old.value.cut {
                            return Err(IndexError::DigestConflict);
                        }
                        let stored = reader.get(transition.input, budget)?;
                        let prepared = quanta_index_search_plane::native_history::with_native_history_boot_source_v1(
                            transition.input,
                            &stored,
                            budget,
                            |input, budget| {
                                let NativeHistoryBootSourceV1::NativeRestore(actual) = &input else {
                                    return Err(IndexError::DigestConflict);
                                };
                                if *actual != &old.value {
                                    return Err(IndexError::DigestConflict);
                                }
                                verify_installed_history_member_v1(
                                    reader,
                                    &selected.publication,
                                    transition,
                                    &stored.registration,
                                    &input,
                                    budget,
                                )
                            },
                        )?;
                        let member = prepared.verify_boot_rows_v1(
                            |reference, budget| reader.get(reference, budget),
                            |root, after, limit, budget| {
                                reader.scan_map(root, after, limit, budget)
                            },
                            budget,
                        )?;
                        seen[index] = true;
                        if step.operation.key == selected.operation.key {
                            installed_history = Some(fund_installed_history_v1(
                                &step.operation,
                                &selected.publication,
                                transition,
                                member,
                                budget,
                            )?);
                        }
                        Ok(())
                    },
                )?;
                prior = &step.publication;
            }
            if seen.iter().any(|seen| !seen) || installed_history.is_none() {
                return Err(IndexError::DigestConflict.into());
            }
        }
        let profile_bytes = u64::try_from(selected_profile.payload.len())?
            .checked_mul(32)
            .and_then(|bytes| bytes.checked_add(4096))
            .ok_or(IndexError::ResourceLimited)?;
        let _profile_memory = budget
            .reserve_memory(
                profile_bytes,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )?
            .ok_or(IndexError::ResourceLimited)?;
        let final_profile: quanta_index_core::index::IndexProfile =
            serde_json::from_slice(&selected_profile.payload)?;
        let final_history_count = final_profile
            .instances
            .iter()
            .filter(|instance| {
                instance.input_domain == quanta_index_core::index::InputDomain::History
            })
            .count();
        if final_history_count != expected.len() {
            return Err(IndexError::DigestConflict.into());
        }
        if !expected.is_empty() {
            let members = selected
                .publication
                .members
                .0
                .ok_or(IndexError::SnapshotUnavailable)?;
            reader.verify_closure(&[members], maximum, budget)?;
        }
    }
    budget.checkpoint()?;
    Ok(installed_history)
}

fn fund_installed_history_v1(
    operation: &quanta_index_core::index::ImmutableOperation,
    publication: &quanta_index_core::index::CatalogView,
    transition: &quanta_index_core::index::HistoryTransition,
    member: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<FundedInstalledHistoryV1, IndexError> {
    let bytes = transition
        .next
        .generation
        .repo_id
        .as_str()
        .len()
        .checked_add(transition.next.generation.revision_id.as_str().len())
        .and_then(|size| u64::try_from(size).ok())
        .ok_or(IndexError::ResourceLimited)?;
    let memory =
        budget.reserve_memory(bytes, quanta_index_core::index::MemoryLifetime::Retained)?;
    let value = NativeCutoverInstalledHistoryV1 {
        operation: operation.key,
        view: publication.identity,
        instance: transition.instance,
        member,
        source_input: transition.input,
        cut: transition.next.clone(),
    };
    value
        .validate_v1()
        .map_err(|error| IndexError::CorruptObject(error.to_string()))?;
    Ok(FundedInstalledHistoryV1 {
        value,
        _memory: memory,
    })
}

fn verify_installed_history_member_v1(
    reader: &FileObjectReader,
    publication: &quanta_index_core::index::CatalogView,
    transition: &quanta_index_core::index::HistoryTransition,
    source: &RegisteredObject,
    input: &quanta_index_search_plane::native_history::NativeHistoryBootSourceV1<'_>,
    budget: &mut WorkBudget,
) -> Result<quanta_index_search_plane::native_history::PreparedNativeHistoryV1, IndexError> {
    let key = quanta_index_core::index::member_map_key(transition.instance);
    let reference = reader
        .lookup_map_object(publication.members, &key, budget)?
        .ok_or(IndexError::SnapshotUnavailable)?;
    let stored = reader.get(reference, budget)?;
    let member = quanta_index_core::index::decode_member_binding(&stored.payload, budget)?;
    if member.instance != transition.instance
        || !stored.registration.children.iter().copied().eq([
            Some(member.artifact),
            member.input.root.0,
        ]
        .into_iter()
        .flatten())
    {
        return Err(IndexError::DigestConflict);
    }
    let artifact = reader.get(member.artifact, budget)?;
    quanta_index_search_plane::native_history::verify_native_history_boot_member_v1(
        &member,
        &artifact,
        &transition.next,
        source,
        input,
        |reference, budget| reader.get(reference, budget),
        budget,
    )
}

// Close failures poison the Common owner even when they were observed in a
// destructor. No receipt or listener handoff may precede this witness.
fn require_retired(authority: &NativeBootAuthority) -> Result<()> {
    authority.io.drain_retirements()?;
    authority.physical.sweep_retired()?;
    if !authority.physical.is_quiescent()? {
        return Err(IndexError::BackendViolation(
            "native boot retained unresolved physical IO custody".into(),
        )
        .into());
    }
    Ok(())
}

fn writes_native_selection(mode: NativeCommandModeV1) -> bool {
    matches!(
        mode,
        NativeCommandModeV1::Select
            | NativeCommandModeV1::SelectCandidate {
                protected_source_routes: None,
                ..
            }
    )
}

/// The original Control reservation and its one WorkBudget precede the
/// deployment lock and Q root lease; this is the only runtime ledger for the
/// complete offline transaction.
pub(super) fn with_control_budget<T>(
    command: &NativeStateCommandV1,
    config: &ValidatedConfig,
    request: RequestBudgetV1,
    callback: impl FnOnce(&mut WorkBudget, Option<MemoryLease>) -> Result<T>,
) -> Result<T> {
    match command.mode {
        NativeCommandModeV1::Select | NativeCommandModeV1::SelectCandidate { .. }
            if command.select_operation.is_none() || command.receipt_path.is_none() =>
        {
            bail!("native selection requires an operation and receipt path")
        }
        NativeCommandModeV1::SelectCandidate { .. } if command.config_digest.is_none() => {
            bail!("native candidate selection requires a pinned config digest")
        }
        NativeCommandModeV1::Verify | NativeCommandModeV1::VerifyCandidate
            if command.identity.is_none()
                || command.config_digest.is_none()
                || command.install_catalog_digest.is_none() =>
        {
            bail!("native verification requires complete identity and install digests")
        }
        NativeCommandModeV1::Serve
            if command.identity.is_none() || command.config_digest.is_none() =>
        {
            bail!("selected Native service requires complete identity and config digest")
        }
        NativeCommandModeV1::Select
        | NativeCommandModeV1::SelectCandidate { .. }
        | NativeCommandModeV1::Verify
        | NativeCommandModeV1::VerifyCandidate
        | NativeCommandModeV1::Serve => {}
        NativeCommandModeV1::ServeCandidate => {
            bail!("candidate service does not enter the offline BOOT path")
        }
    }
    with_admitted_control_budget(
        config,
        request,
        writes_native_selection(command.mode),
        callback,
    )
}

/// Reuse the same Control ledger for recovery before reading its FD evidence
/// or acquiring the deployment/root locks. No second WorkBudget is created.
pub(super) fn with_admitted_control_budget<T>(
    config: &ValidatedConfig,
    request: RequestBudgetV1,
    reserve_select_cache: bool,
    callback: impl FnOnce(&mut WorkBudget, Option<MemoryLease>) -> Result<T>,
) -> Result<T> {
    if fs::symlink_metadata(&config.state_root)?
        .file_type()
        .is_symlink()
    {
        bail!("selected native state root must not be a symlink");
    }
    let ledger = NativeResourceLedger::with_maintenance(
        config.resources.execution,
        config.owned_work.is_some(),
        config.maintenance.is_some(),
    )?;
    let catalog_cache_memory =
        if reserve_select_cache {
            Some(ledger.reserve_catalog_cache(
                config.resources.catalog_writer_cache.reservation_bytes()?,
            )?)
        } else {
            None
        };
    let scheduler = ledger.scheduler(NativeResourceClass::Control);
    let control =
        ReservationGuard::try_acquire(Arc::clone(&scheduler), config.resources.execution.control)
            .ok_or_else(|| scheduler.denial())?;
    let mut budget = WorkBudget::with_request(config.limits, request);
    budget.bind_memory_scope(control.memory_scope());
    let result = callback(&mut budget, catalog_cache_memory);
    drop(control);
    result
}

/// The caller retains its deployment lock before entering this scope. The
/// actual StateRootLease, physical owner, and budget remain live through the
/// callback and any directory fsync it performs.
pub(super) fn with_boot_authority<T>(
    config: &ValidatedConfig,
    mut budget: &mut WorkBudget,
    callback: impl FnOnce(NativeBootAuthority, &mut WorkBudget) -> Result<T>,
) -> Result<T> {
    let lease = Arc::new(StateRootLease::acquire_existing(&config.state_root)?);
    let authority = prepare_native_boot_authority(
        lease,
        &config.socket_path,
        &config.resources,
        budget.request(),
    )?;
    verify_existing_native_format(&authority, &config.socket_path, &mut budget)?;
    callback(authority, budget)
}

/// Recovery's Native root uses the Original-funded inventory; ordinary
/// select/serve boot keeps its existing service resource path.
fn with_recovery_boot_authority<T>(
    config: &ValidatedConfig,
    budget: &mut WorkBudget,
    callback: impl FnOnce(NativeBootAuthority, &mut WorkBudget) -> Result<T>,
) -> Result<T> {
    let (lease, memory) = acquire_recovery_root_v1(&config.state_root, budget)?;
    let authority = prepare_native_recovery_boot_authority(
        Arc::new(lease),
        memory,
        &config.socket_path,
        &config.resources,
        budget,
    )?;
    verify_existing_native_format(&authority, &config.socket_path, budget)?;
    callback(authority, budget)
}

pub(super) fn require_unselected_candidate(
    authority: &NativeBootAuthority,
    config: &ValidatedConfig,
    request: &RequestBudgetV1,
) -> Result<()> {
    let root = authority.lease.state_root_identity_v1();
    let directory = quanta_index_catalog::catalog_dir(root);
    let catalog = directory.join(quanta_index_catalog::CATALOG_FILE_NAME);
    match fs::symlink_metadata(&catalog) {
        Ok(_) => with_admitted_control_budget(config, request.clone(), false, |budget, _| {
            verify_existing_native_format(authority, &config.socket_path, budget)?;
            let snapshot = verify_native_boot_catalog(root, authority.io.as_ref(), budget)?;
            if snapshot.selected.is_some() {
                return Err(IndexError::Conflict.into());
            }
            authority.lease.require_state_root_v1(root)?;
            budget.checkpoint()?;
            Ok(())
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::symlink_metadata(&directory) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Ok(metadata) if metadata.is_dir() && fs::read_dir(directory)?.next().is_none() => {
                    Ok(())
                }
                Ok(_) => Err(IndexError::RebuildRequired.into()),
                Err(error) => Err(error.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

/// None is offline completion; Some transfers the same root and Common owner.
pub(super) fn execute(
    command: &NativeStateCommandV1,
    config: &ValidatedConfig,
    registry: &AdapterRegistry,
    request: RequestBudgetV1,
) -> Result<Option<NativeBootAuthority>> {
    with_control_budget(
        command,
        config,
        request,
        |budget, mut catalog_cache_memory| {
            with_boot_authority(config, budget, |authority, mut budget| {
                let root = authority.lease.state_root_identity_v1().to_path_buf();
                let maximum = config.resources.physical.inventory.max_entries;
                if matches!(
                    command.mode,
                    NativeCommandModeV1::Select | NativeCommandModeV1::SelectCandidate { .. }
                ) {
                    let target = receipt::target(
                        command
                            .receipt_path
                            .as_ref()
                            .ok_or_else(|| anyhow!("missing native receipt"))?,
                        &root,
                    )?;
                    let operation = command
                        .select_operation
                        .ok_or_else(|| anyhow!("missing native operation"))?;
                    let candidate =
                        if matches!(
                            command.mode,
                            NativeCommandModeV1::SelectCandidate {
                                protected_source_routes: None,
                                ..
                            }
                        ) {
                            verify_native_boot_current_candidate(
                                &root,
                                authority.io.as_ref(),
                                operation,
                                &mut budget,
                            )?
                        } else {
                            verify_native_boot_candidate(
                                &root,
                                authority.io.as_ref(),
                                operation,
                                &mut budget,
                            )?
                        };
                    let presealed = match command.mode {
                        NativeCommandModeV1::SelectCandidate {
                            protected_source_routes: Some(reference),
                            ..
                        } => {
                            let selected = candidate
                                .snapshot()
                                .selected
                                .as_ref()
                                .ok_or(IndexError::SnapshotUnavailable)?;
                            if selected.protected_source_routes.map(|route| route.object)
                                != Some(reference)
                                || selected.operation != candidate.manifest().operation
                                || selected.publication != candidate.manifest().publication
                            {
                                return Err(IndexError::DigestConflict.into());
                            }
                            Some(selected.clone())
                        }
                        _ => None,
                    };
                    let manifest = presealed.as_ref().unwrap_or_else(|| candidate.manifest());
                    let expected = identity(candidate.snapshot().root_nonce, manifest)?;
                    if let NativeCommandModeV1::SelectCandidate {
                        root_nonce,
                        source_cut_digest,
                        protected_source_routes,
                    } = command.mode
                    {
                        if expected.root_nonce != root_nonce
                            || expected.source_cut_digest != source_cut_digest
                            || expected.protected_source_routes != protected_source_routes
                        {
                            return Err(IndexError::DigestConflict.into());
                        }
                        if candidate
                            .snapshot()
                            .selected
                            .as_ref()
                            .is_some_and(|selected| selected != manifest)
                        {
                            // Candidate preparation may retry the same selection after an
                            // uncertain receipt write. It cannot replace another BOOT.
                            return Err(IndexError::Conflict.into());
                        }
                    }
                    // A mutable selection needs physical authentication before
                    // the Catalog write. A presealed route is authenticated
                    // once from the receipt snapshot under the same root lease.
                    let writes_selection = writes_native_selection(command.mode);
                    if writes_selection {
                        authenticate(
                            &authority,
                            candidate.snapshot(),
                            manifest,
                            registry,
                            maximum,
                            None,
                            false,
                            &mut budget,
                        )?;
                    }
                    drop(candidate);
                    if writes_selection {
                        let _funding = authority.io.enter_maintenance_scope()?;
                        let mut catalog = SqliteCatalog::open_with_io_admission_and_writer_cache(
                            &root,
                            std::time::Duration::ZERO,
                            authority.io.clone(),
                            config.resources.catalog_writer_cache,
                            catalog_cache_memory.take().ok_or_else(|| {
                                IndexError::BackendViolation(
                                    "native selection Catalog cache lease is missing".into(),
                                )
                            })?,
                        )?;
                        if let Some(observation) = catalog.writer_cache_observation() {
                            crate::native_catalog_cache_diagnostics::record_catalog_writer_cache(
                                budget.request().diagnostic_context(),
                                &observation,
                            );
                        }
                        let selected = if matches!(
                            command.mode,
                            NativeCommandModeV1::SelectCandidate { .. }
                        ) {
                            catalog
                                .select_native_boot_manifest_if_current(operation, &mut budget)?
                        } else {
                            catalog.select_native_boot_manifest(operation, &mut budget)?
                        };
                        if identity(catalog.native_root_nonce(&mut budget)?, &selected)? != expected
                        {
                            return Err(IndexError::DigestConflict.into());
                        }
                        catalog.close_checked()?;
                    }
                    require_retired(&authority)?;
                    let snapshot =
                        verify_native_boot_catalog(&root, authority.io.as_ref(), &mut budget)?;
                    let selected = snapshot
                        .selected
                        .as_ref()
                        .ok_or(IndexError::SnapshotUnavailable)?;
                    if identity(snapshot.root_nonce, selected)? != expected {
                        return Err(IndexError::DigestConflict.into());
                    }
                    authenticate(
                        &authority,
                        &snapshot,
                        selected,
                        registry,
                        maximum,
                        None,
                        presealed.is_some(),
                        &mut budget,
                    )?;
                    let io = NativeObjectReadIo::new(Arc::clone(&authority.physical));
                    let digest = catalog_digest::catalog_content_digest(&root, &io, &mut budget)?;
                    drop(snapshot);
                    require_retired(&authority)?;
                    authority.lease.require_state_root_v1(&root)?;
                    budget.checkpoint()?;
                    // Publication is the final fallible operation. Catalog selection may
                    // already be committed on failure; postcommit uncertainty can also
                    // leave a durable receipt. Inspect that outcome before retrying with
                    // a fresh, absent target; installation must independently verify it.
                    target.write(expected, digest, &mut budget)?;
                    return Ok(None);
                }
                let verify_current = command.mode == NativeCommandModeV1::VerifyCandidate;
                // Protected candidates have their own current-head and
                // quiescence checks over all selected workspaces.
                let routeless_candidate = verify_current
                    && command
                        .identity
                        .is_none_or(|identity| identity.protected_source_routes.is_none());
                let catalog_snapshot;
                let current_candidate;
                let snapshot = if routeless_candidate {
                    let operation = command
                        .identity
                        .ok_or(IndexError::SnapshotUnavailable)?
                        .manifest_operation;
                    current_candidate = verify_native_boot_current_candidate(
                        &root,
                        authority.io.as_ref(),
                        operation,
                        &mut budget,
                    )?;
                    let snapshot = current_candidate.snapshot();
                    if snapshot.selected.as_ref() != Some(current_candidate.manifest()) {
                        return Err(IndexError::Conflict.into());
                    }
                    snapshot
                } else {
                    catalog_snapshot =
                        verify_native_boot_catalog(&root, authority.io.as_ref(), &mut budget)?;
                    &catalog_snapshot
                };
                let selected = snapshot
                    .selected
                    .as_ref()
                    .ok_or(IndexError::SnapshotUnavailable)?;
                if Some(identity(snapshot.root_nonce, selected)?) != command.identity {
                    return Err(IndexError::DigestConflict.into());
                }
                authenticate(
                    &authority,
                    snapshot,
                    selected,
                    registry,
                    maximum,
                    None,
                    verify_current,
                    &mut budget,
                )?;
                if let Some(expected) = command.install_catalog_digest {
                    let io = NativeObjectReadIo::new(Arc::clone(&authority.physical));
                    if catalog_digest::catalog_content_digest(&root, &io, &mut budget)? != expected
                    {
                        return Err(IndexError::DigestConflict.into());
                    }
                }
                require_retired(&authority)?;
                authority.lease.require_state_root_v1(&root)?;
                budget.checkpoint()?;
                if command.mode == NativeCommandModeV1::Serve {
                    Ok(Some(authority))
                } else {
                    Ok(None)
                }
            })
        },
    )
}

/// One borrowed projection for both recovery directions. Only the signed-parent
/// receiver may supply this already joined Original Source inventory.
fn recovery_source_inputs_v1<'a>(
    evidence: &'a quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceV1,
    joined: &'a quanta_index_contract::native_cutover_source_join_v1::NativeCutoverJoinedSourceFactsV1<'a>,
) -> Result<RecoverySourceInputs<'a>> {
    Ok(RecoverySourceInputs {
        target: &evidence.baseline,
        baseline_candidate_root_nonce: joined.baseline_candidate_root_nonce,
        request: joined
            .native_update_request_v1()
            .ok_or(IndexError::SnapshotUnavailable)?,
        current_target: joined.current_native_target,
        current_request: joined
            .current_native_update_request_v1()
            .ok_or(IndexError::SnapshotUnavailable)?,
        legacy_publication: evidence.legacy_publication.as_ref(),
        joined_source: Some(joined),
        ast_work: joined_ast_inputs(joined.ast_published_work)?,
    })
}

/// Authenticate a prepared candidate with its exact installed BOOT and Source
/// publications under one held Q root. The retained result is DATA, not a permit.
fn verify_recovery_candidate_with_source_capture_v1(
    authority: &NativeBootAuthority,
    command: &NativeStateCommandV1,
    registry: &AdapterRegistry,
    source: &RecoverySourceInputs<'_>,
    maximum: u64,
    budget: &mut WorkBudget,
) -> Result<quanta_index_catalog::NativeBootPublishedOperation> {
    verify_recovery_candidate_with_legacy_history_v1(
        authority, command, registry, source, maximum, None, None, budget,
    )
    .map(|(full, _)| full)
}

fn verify_recovery_candidate_with_legacy_history_v1(
    authority: &NativeBootAuthority,
    command: &NativeStateCommandV1,
    registry: &AdapterRegistry,
    source: &RecoverySourceInputs<'_>,
    maximum: u64,
    legacy_history: Option<LegacyHistoryRecoveryV1<'_>>,
    native_history: Option<&[FundedNativeRestoreInputV1]>,
    budget: &mut WorkBudget,
) -> Result<(
    quanta_index_catalog::NativeBootPublishedOperation,
    Option<FundedInstalledHistoryV1>,
)> {
    if legacy_history.is_some() && native_history.is_some() {
        return Err(IndexError::DigestConflict.into());
    }
    let root = authority.lease.state_root_identity_v1();
    let witness_operation = match (legacy_history, source.ast_work.as_ref()) {
        (Some(_), Some(ast)) => {
            if source.target.operation != source.current_target.operation {
                return Err(IndexError::DigestConflict.into());
            }
            ast.operation
        }
        _ => source.current_target.operation,
    };
    let capture_history =
        legacy_history.is_some() || native_history.is_some_and(|expected| !expected.is_empty());
    let full = if let Some(expected_routes) = command
        .identity
        .and_then(|identity| identity.protected_source_routes)
    {
        let snapshot = verify_native_boot_catalog(root, authority.io.as_ref(), budget)?;
        let selected = snapshot
            .selected
            .as_ref()
            .ok_or(IndexError::SnapshotUnavailable)?;
        if Some(identity(snapshot.root_nonce, selected)?) != command.identity {
            return Err(IndexError::DigestConflict.into());
        }
        let route = selected
            .protected_source_routes
            .ok_or(IndexError::SnapshotUnavailable)?;
        let objects = root.join("native-objects");
        let io = Arc::new(NativeObjectReadIo::new(Arc::clone(&authority.physical)));
        let lock = io.open(&objects.join(".object-namespace.lease"), budget)?;
        let reader = FileObjectReader::open_existing(&objects, lock, io, budget)?;
        if snapshot.namespace != Some(reader.namespace_binding()?) {
            return Err(IndexError::DigestConflict.into());
        }
        let stored = reader.get(expected_routes, budget)?;
        quanta_index_core::index::with_native_protected_source_routes_v1(
            expected_routes,
            Some(route.count),
            &stored,
            budget,
            |routes, budget| {
                quanta_index_catalog::verify_native_recovery_publications_with_routes_v1(
                    root,
                    authority.io.as_ref(),
                    source.target.operation,
                    witness_operation,
                    routes,
                    capture_history,
                    budget,
                )
            },
        )?
    } else {
        let observe = if capture_history {
            verify_native_legacy_history_recovery_publications_v1
        } else {
            verify_native_rollback_with_source_publications
        };
        observe(
            root,
            authority.io.as_ref(),
            source.target.operation,
            witness_operation,
            budget,
        )?
    };
    let snapshot = full.snapshot();
    let selected = snapshot
        .selected
        .as_ref()
        .ok_or(IndexError::SnapshotUnavailable)?;
    if Some(identity(snapshot.root_nonce, selected)?) != command.identity {
        return Err(IndexError::DigestConflict.into());
    }
    let installed_history = authenticate_with_legacy_history_v1(
        authority,
        snapshot,
        selected,
        registry,
        maximum,
        Some(&RecoveryCapture {
            source,
            full: &full,
        }),
        legacy_history,
        native_history,
        false,
        budget,
    )?;
    require_retired(authority)?;
    authority.lease.require_state_root_v1(root)?;
    budget.checkpoint()?;
    Ok((full, installed_history))
}

/// Private Q half of the resident transaction. The peer-bound helper
/// retains genuine producer/SDK custody while calling it; no Source or ACK
/// authorization is inferred from the Catalog snapshot or selection document.
pub(super) fn publish_rollback_under_locked_selection_v1(
    command: &NativeStateCommandV1,
    config: &ValidatedConfig,
    registry: &AdapterRegistry,
    evidence: &quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceV1,
    joined: &quanta_index_contract::native_cutover_source_join_v1::NativeCutoverJoinedSourceFactsV1<
        '_,
    >,
    locked: &mut LockedDeploymentSelectionV1,
    budget: &mut WorkBudget,
    custody_checkpoint: &mut dyn FnMut(&mut WorkBudget) -> Result<()>,
) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
    let source = recovery_source_inputs_v1(evidence, joined)
        .map_err(DeploymentSelectionErrorV1::RefusedBeforeRename)?;
    publish_rollback_with_source_capture_v1(
        command,
        config,
        registry,
        &source,
        locked,
        budget,
        custody_checkpoint,
    )
}

/// The optional typed Legacy input is absent. Presence alone is still DATA
/// and cannot replace the genuine Source and physical-root verification.
#[derive(Debug)]
struct MissingLegacyPublicationInputV1;

impl std::fmt::Display for MissingLegacyPublicationInputV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "Legacy source verification unavailable: typed Legacy publication input is missing",
        )
    }
}

impl std::error::Error for MissingLegacyPublicationInputV1 {}

#[derive(Debug)]
struct LegacyPhysicalVerificationUnavailableV1;
impl std::fmt::Display for LegacyPhysicalVerificationUnavailableV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Legacy publication DATA is present but its genuine Source and held-root physical verification are not connected")
    }
}
impl std::error::Error for LegacyPhysicalVerificationUnavailableV1 {}

#[path = "native_boot/legacy_read_factory.rs"]
mod legacy_read_factory;

struct VerifiedLegacyPublicationV1 {
    repomap: Option<VerifiedLegacyRepoMapV1>,
    has_history_rows: bool,
}

fn verify_legacy_publication_v1(
    authority: &legacy_read_factory::LegacyReadAuthorityV1,
    source: &RecoverySourceInputs<'_>,
    native: &NativeBootAuthority,
    config: &ValidatedConfig,
    locked: &LockedDeploymentSelectionV1,
    budget: &mut WorkBudget,
) -> Result<VerifiedLegacyPublicationV1> {
    let publication = source
        .legacy_publication
        .ok_or(MissingLegacyPublicationInputV1)?;
    // The Source association is borrowed from the canonical join made inside
    // the signed-parent/Original custody scope. Optional receipt DATA cannot
    // replace that current replay + acknowledged Source association.
    let joined = source
        .joined_source
        .ok_or(LegacyPhysicalVerificationUnavailableV1)?;
    if joined.current_legacy_replay.is_none()
        || (joined.current_legacy_sdk_ack.is_none()
            && joined.current_legacy_ordinary_binding.is_none())
    {
        return Err(LegacyPhysicalVerificationUnavailableV1.into());
    }
    let expected = &publication.activation.active;
    let generation = &expected.generation;
    let lexical = &generation.lexical;
    let semantic = &generation.semantic;
    let frame = authority
        .root()
        .as_os_str()
        .len()
        .checked_add(lexical.repo_id.as_str().len())
        .and_then(|n| n.checked_add(lexical.revision_id.as_str().len()))
        .and_then(|n| n.checked_add(semantic.repo_id.as_str().len()))
        .and_then(|n| n.checked_add(semantic.revision_id.as_str().len()))
        .and_then(|n| n.checked_add(publication.receipt.batch_digest.len()))
        .and_then(|n| n.checked_mul(16))
        .and_then(|n| n.checked_add(65536))
        .ok_or(IndexError::ResourceLimited)?;
    let _frame = budget
        .reserve_memory(
            u64::try_from(frame)?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?
        .ok_or(IndexError::ResourceLimited)?;
    expected.validate_v1()?;
    let key = quanta_index_core::IdempotencyKeyV1 {
        kind: quanta_index_contract::IngestOperationKindV1::SearchCorpus,
        repo_id: lexical.repo_id.clone(),
        revision_id: lexical.revision_id.clone(),
        generation: publication.receipt.generation,
        batch_digest: publication.receipt.batch_digest.clone(),
    };
    // This admitted path/frame grant precedes all owned keys and path joins.
    // Current physical Source was separately checked by the signed Original
    // supplier. These on-disk observations never mint that authority.
    let root = authority.root();
    let quiescence = quanta_index_catalog::verify_legacy_catalog_quiescence_v1(
        root,
        authority.catalog(),
        &lexical.repo_id,
        &lexical.revision_id,
        budget,
    )?;
    if quiescence.latest_retained_search_corpus_sequence
        != Some(publication.receipt.durable_sequence)
    {
        return Err(IndexError::DigestConflict.into());
    }
    let repomap = verify_legacy_repomap_v1(
        authority,
        &quiescence,
        &publication.repomap,
        native,
        config,
        locked,
        budget,
    )?;
    let has_history_rows = quiescence.has_history_rows_v1();
    if publication.history.is_some() && !has_history_rows {
        return Err(IndexError::DigestConflict.into());
    }
    drop(quiescence);
    let activation_root = root.join("activations");
    let before = quanta_index_search_plane::readiness::inspect_existing_activation_repository_v1(
        &activation_root,
        &lexical.repo_id,
        &lexical.revision_id,
        authority.objects(),
        authority.metadata(),
        budget,
    )?;
    if before.head_v1() != Some(expected) {
        return Err(IndexError::DigestConflict.into());
    }
    let receipt = quanta_index_catalog::verify_legacy_terminal_batch_receipt(
        root,
        authority.catalog(),
        &key,
        &publication.receipt,
        budget,
    )?;
    quanta_index_search_plane::readiness::inspect_existing_sealed_search_corpus_v1(
        root,
        &lexical.repo_id,
        &lexical.revision_id,
        publication.receipt.generation,
        &lexical.manifest_digest,
        authority.objects(),
        budget,
    )?;
    let lexical_track = root.join("indexes/lexical");
    let semantic_track = root.join("indexes/semantic");
    let semantic_path = quanta_index_core::GenerationStorageKeyV1::for_repo_revision(
        &semantic.repo_id,
        &semantic.revision_id,
    )
    .generation_dir(&semantic_track, semantic.manifest_generation);
    let owner = rustix::process::geteuid().as_raw();
    quanta_index_lexical::legacy::verify_existing_lexical_generation_read_only(
        &lexical_track,
        lexical,
        owner,
        authority.maximum_objects(),
        authority.metadata(),
        authority.objects(),
        budget,
    )?;
    quanta_index_semantic::legacy::verify_existing_semantic_generation_read_only(
        &semantic_path,
        semantic,
        owner,
        authority.metadata(),
        authority.objects(),
        budget,
        |roots, _budget| {
            if roots != &generation.semantic_content {
                return Err(IndexError::DigestConflict.into());
            }
            Ok(())
        },
    )?;
    let after = quanta_index_search_plane::readiness::inspect_existing_activation_repository_v1(
        &activation_root,
        &lexical.repo_id,
        &lexical.revision_id,
        authority.objects(),
        authority.metadata(),
        budget,
    )?;
    if after.head_v1() != Some(expected) {
        return Err(IndexError::DigestConflict.into());
    }
    drop((before, after, receipt));
    authority.require_current_and_retired(budget)?;
    budget.checkpoint()?;
    Ok(VerifiedLegacyPublicationV1 {
        repomap,
        has_history_rows,
    })
}

struct VerifiedLegacyRepoMapV1 {
    files: quanta_index_repomap::VerifiedActiveRepoMapFilesV1,
    separate: Option<legacy_read_factory::LegacyReadAuthorityV1>,
    locator: quanta_index_contract::native_cutover_evidence_v1::NativeCutoverDigest64V1,
}

fn verify_legacy_repomap_v1(
    legacy: &legacy_read_factory::LegacyReadAuthorityV1,
    quiescence: &quanta_index_catalog::LegacyCatalogQuiescenceV1,
    expected: &quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceLegacyRepoMapV1,
    native: &NativeBootAuthority,
    config: &ValidatedConfig,
    locked: &LockedDeploymentSelectionV1,
    budget: &mut WorkBudget,
) -> Result<Option<VerifiedLegacyRepoMapV1>> {
    use quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceLegacyRepoMapV1 as RepoMap;
    let RepoMap::Required {
        state_root,
        locator_digest,
        publish,
        activation,
    } = expected
    else {
        if config.legacy_repomap_selection.is_some()
            || quiescence.active_repomap_candidate_v1().is_some()
        {
            return Err(IndexError::DigestConflict.into());
        }
        return Ok(None);
    };
    let root = std::path::Path::new(state_root);
    let receipts = (publish, activation);
    if root == legacy.root() {
        anyhow::ensure!(
            config.legacy_repomap_selection.is_none(),
            "same-root RepoMap must use the saved Legacy selection"
        );
        legacy.require_repomap_locator_v1(*locator_digest, budget)?;
        return Ok(Some(VerifiedLegacyRepoMapV1 {
            files: legacy.verify_active_repomap_v1(quiescence, receipts, budget)?,
            separate: None,
            locator: *locator_digest,
        }));
    }
    anyhow::ensure!(
        quiescence.active_repomap_candidate_v1().is_none(),
        "Legacy SearchCorpus root has an unexpected active RepoMap"
    );
    for other in [legacy.root(), native.lease.state_root_identity_v1()] {
        anyhow::ensure!(
            !root.starts_with(other) && !other.starts_with(root),
            "separate RepoMap root overlaps a recovery root"
        );
    }
    let selection = config
        .legacy_repomap_selection
        .as_ref()
        .ok_or_else(|| anyhow!("separate RepoMap root has no saved physical policy selection"))?;
    let saved = locked.read_legacy_repomap_selection_v1(
        &selection.path,
        quanta_index_core::index::Digest(selection.sha256),
        root,
        budget,
    )?;
    let (lease, memory) = acquire_recovery_root_v1(root, budget)?;
    let authority = legacy_read_factory::LegacyReadAuthorityV1::acquire(
        &saved,
        native.io.as_ref(),
        lease,
        memory,
        budget,
    )?;
    authority.require_repomap_locator_v1(*locator_digest, budget)?;
    let observed = quanta_index_catalog::verify_legacy_catalog_quiescence_v1(
        authority.root(),
        authority.catalog(),
        &publish.mutation.repo_id,
        &publish.mutation.revision_id,
        budget,
    )?;
    let files = authority.verify_active_repomap_v1(&observed, receipts, budget)?;
    drop(observed);
    authority.retire_catalog(budget)?;
    Ok(Some(VerifiedLegacyRepoMapV1 {
        files,
        separate: Some(authority),
        locator: *locator_digest,
    }))
}

fn acquire_recovery_root_v1(
    root: &std::path::Path,
    budget: &mut WorkBudget,
) -> Result<(
    StateRootLease,
    Arc<crate::publication_file_capacity::RecoveryReadMemoryV1>,
)> {
    let memory = crate::publication_file_capacity::RecoveryReadMemoryV1::new(root, budget)?;
    budget.charge(quanta_index_core::index::WorkUsage {
        reads: u64::try_from(root.components().count())?
            .checked_mul(128)
            .and_then(|n| n.checked_add(128))
            .ok_or(IndexError::ResourceLimited)?,
        ..Default::default()
    })?;
    Ok((StateRootLease::acquire_existing(root)?, memory))
}

pub(crate) fn with_legacy_checkpoint_catalog_v1<T>(
    selection: &crate::deployment_selection::DeploymentSelectionDocumentV1,
    budget: &mut WorkBudget,
    read: impl FnOnce(
        &std::path::Path,
        &dyn quanta_index_catalog::CatalogReadOnlyIoAdmission,
        &mut WorkBudget,
    ) -> Result<T>,
) -> Result<T> {
    let (lease, memory) = acquire_recovery_root_v1(selection.state_root_v1(), budget)?;
    let authority = legacy_read_factory::LegacyReadAuthorityV1::acquire_for_checkpoint_export_v1(
        selection, lease, memory, budget,
    )?;
    let result = read(authority.root(), authority.catalog(), budget)?;
    authority.require_current_root_v1()?;
    Ok(result)
}

/// Lend the same held, offline Legacy root and read-only physical ports to a
/// complete Source export. The callback cannot outlive the root lease.
pub(crate) fn with_legacy_source_export_v1<T>(
    selection: &crate::deployment_selection::DeploymentSelectionDocumentV1,
    budget: &mut WorkBudget,
    read: impl FnOnce(
        &std::path::Path,
        &dyn quanta_index_catalog::CatalogReadOnlyIoAdmission,
        &dyn quanta_index_core::index::ports::ObjectReadIoPort,
        &dyn quanta_index_core::ExistingTreeMetadataIoPortV1,
        usize,
        u64,
        &mut WorkBudget,
    ) -> Result<T>,
) -> Result<T> {
    let (lease, memory) = acquire_recovery_root_v1(selection.state_root_v1(), budget)?;
    let authority = legacy_read_factory::LegacyReadAuthorityV1::acquire_for_checkpoint_export_v1(
        selection, lease, memory, budget,
    )?;
    let result = read(
        authority.root(),
        authority.catalog(),
        authority.objects(),
        authority.metadata(),
        authority.maximum_objects(),
        authority.repomap_candidate_file_bytes_v1(),
        budget,
    )?;
    authority.retire_catalog(budget)?;
    authority.require_current_root_v1()?;
    Ok(result)
}

/// Both recovery directions retain the same physical/Original authorities
/// through the single canonical selection publisher and its final checks.
fn publish_with_held_recovery_roots_v1(
    native: &NativeBootAuthority,
    legacy_for_repomap: Option<&legacy_read_factory::LegacyReadAuthorityV1>,
    previous_current: &mut dyn FnMut(&mut WorkBudget) -> Result<()>,
    repomap: Option<&VerifiedLegacyRepoMapV1>,
    config: &ValidatedConfig,
    locked: &mut LockedDeploymentSelectionV1,
    budget: &mut WorkBudget,
    custody_checkpoint: &mut dyn FnMut(&mut WorkBudget) -> Result<()>,
) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
    let mut checkpoint = |budget: &mut WorkBudget| -> Result<()> {
        require_retired(native)?;
        native
            .lease
            .require_state_root_v1(native.lease.state_root_identity_v1())?;
        custody_checkpoint(budget)?;
        if let Some(repomap) = repomap {
            let root = repomap
                .separate
                .as_ref()
                .or(legacy_for_repomap)
                .ok_or_else(|| anyhow!("RepoMap proof lost its retained Legacy root"))?;
            root.require_repomap_locator_v1(repomap.locator, budget)?;
            repomap
                .files
                .require_current_v1(root.root(), root.objects(), budget)?;
            if let Some(separate) = &repomap.separate {
                let selection = config
                    .legacy_repomap_selection
                    .as_ref()
                    .ok_or_else(|| anyhow!("separate RepoMap selection binding disappeared"))?;
                LockedDeploymentSelectionV1::require_legacy_repomap_selection_v1(
                    &selection.path,
                    quanta_index_core::index::Digest(selection.sha256),
                    budget,
                )?;
                separate.require_current_and_retired(budget)?;
            }
        }
        previous_current(budget)?;
        budget.checkpoint()?;
        Ok(())
    };
    locked.publish_with_custody_v1(budget, &mut checkpoint)
}

fn publish_rollback_with_source_capture_v1(
    command: &NativeStateCommandV1,
    config: &ValidatedConfig,
    registry: &AdapterRegistry,
    source: &RecoverySourceInputs<'_>,
    locked: &mut LockedDeploymentSelectionV1,
    budget: &mut WorkBudget,
    custody_checkpoint: &mut dyn FnMut(&mut WorkBudget) -> Result<()>,
) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
    let verified = with_recovery_boot_authority(
        config,
        budget,
        |authority,
         budget|
         -> Result<
            std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1>,
        > {
            // The saved Legacy root must also be offline. Acquisition is
            // nonblocking and existing-only; no other owner's daemon is stopped.
            let (legacy_lease, legacy_memory) =
                acquire_recovery_root_v1(locked.candidate_v1().state_root_v1(), budget)?;
            custody_checkpoint(budget)?;
            let full = verify_recovery_candidate_with_source_capture_v1(
                &authority,
                command,
                registry,
                source,
                config.resources.physical.inventory.max_entries,
                budget,
            )?;
            legacy_lease.require_state_root_v1(locked.candidate_v1().state_root_v1())?;
            custody_checkpoint(budget)?;
            if source.legacy_publication.is_none() {
                return Err(MissingLegacyPublicationInputV1.into());
            }
            let legacy = legacy_read_factory::LegacyReadAuthorityV1::acquire(
                locked.candidate_v1(),
                authority.io.as_ref(),
                legacy_lease,
                legacy_memory,
                budget,
            )?;
            let verified =
                verify_legacy_publication_v1(&legacy, source, &authority, config, locked, budget)?;
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
            drop(full);
            Ok(published)
        },
    );
    verified.map_err(DeploymentSelectionErrorV1::RefusedBeforeRename)?
}

#[cfg(test)]
#[path = "native_boot/legacy_candidate_tests.rs"]
mod legacy_candidate_tests;
