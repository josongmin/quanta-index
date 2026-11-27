//! Observe the actual V7 Source publications under Q's offline root leases.

use anyhow::{Result, ensure};
use quanta_index_catalog::{
    LegacyHistoryProducerScopeV1, NativeBootPublicationReader,
    verify_legacy_active_repomap_receipts_with_siblings_v1,
    verify_legacy_history_source_inventory_all_v2, verify_native_source_root_publications_v1,
    verify_native_source_root_publications_with_routes_v1,
};
use quanta_index_contract::native_cutover_evidence_v1::{
    MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1, NativeCutoverDigest64V1,
    NativeCutoverEvidenceLegacyPublicationV1, NativeCutoverEvidenceLegacyRepoMapV1,
    NativeCutoverEvidenceSourceFactV1 as Fact, NativeCutoverEvidenceSourcePublicationBindingV1,
    NativeCutoverEvidenceTargetV1, NativeCutoverHistoricalRepoMapV6,
    NativeCutoverHistoricalV5ArtifactV6, NativeCutoverPreviousNativeV6,
    NativeCutoverPreviousPublicationV6, NativeCutoverReplayPhaseV1,
    NativeCutoverReservationEvidenceV1, NativeCutoverSourceOnlyEvidenceV6,
    NativeCutoverSourceOnlyPublicationV7, read_native_cutover_before_native_selector_v1,
};
use quanta_index_contract::native_cutover_source_join_v1::{
    NativeCutoverPublicationFactsV1, NativeCutoverSourceJoinErrorV1,
};
use quanta_index_core::index::{
    CatalogView, Digest, ImmutableOperation, IndexError, MemoryLifetime, ObjectRef, OperationKey,
    SourceCut, SourceOperationMode, ViewRef, WorkBudget, WorkUsage,
};
use quanta_index_core::publication_capacity::InodeKey;
use quanta_index_search_plane::native_history::with_native_history_boot_source_v1;
use quanta_index_storage::FileObjectReader;

use super::super::{
    FundedInstalledHistoryV1, RestoreForwardHistoryV1, VerifiedLegacyRepoMapV1,
    legacy_read_factory::LegacyReadAuthorityV1,
};
use super::HeldPreviousNativeV1;
use super::preserved_sources_v1::VerifiedPreservedSourcesV1;
use crate::deployment_selection::{
    DeploymentSelectionDocumentV1, DeploymentSelectionErrorV1, DeploymentSelectionPublishedV1,
    LockedDeploymentSelectionV1, SourceOnlyRecoveryIdentityV2,
};

#[derive(Clone, Copy)]
struct VerifiedSourcePublicationV7 {
    operation: OperationKey,
    request: ObjectRef,
    view: ViewRef,
    source: SourceCut,
}

/// Watch publication observed under one of the retained Catalog leases.
pub(crate) struct CatalogWatchPublicationV7<'a> {
    pub(crate) operation: OperationKey,
    pub(crate) snapshot: u64,
    // HistoricalV5 exposes the settled snapshot. Source-only V7 also carries
    // the complete Source binding from the current or retained writer.
    pub(crate) source: Option<&'a NativeCutoverEvidenceSourcePublicationBindingV1>,
}

enum SourceOnlyPredecessorV7 {
    Native {
        held: HeldPreviousNativeV1,
        repomap: Option<VerifiedLegacyRepoMapV1>,
    },
    Legacy {
        held: LegacyReadAuthorityV1,
        preservation: VerifiedPreservedSourcesV1,
        repomap_locator: Option<NativeCutoverDigest64V1>,
    },
}

/// Retain physical custody of the exact FD6 selection pair through publication.
pub(crate) struct VerifiedSourceOnlyRecoveryV7 {
    before: Digest,
    after: Digest,
    candidate: HeldPreviousNativeV1,
    previous: SourceOnlyPredecessorV7,
    installed: SourceOnlyRecoveryIdentityV2,
    root_inode: InodeKey,
    history: Option<FundedInstalledHistoryV1>,
}

impl VerifiedSourceOnlyRecoveryV7 {
    fn require_current_v7(&self, budget: &mut WorkBudget) -> Result<()> {
        self.candidate.require_current_and_retired_v1(budget)?;
        match &self.previous {
            SourceOnlyPredecessorV7::Native { held, repomap } => {
                if let Some(proof) = repomap {
                    super::require_previous_repomap_current_v1(
                        proof,
                        held.selected_config_v1(),
                        budget,
                    )?;
                }
                held.require_current_and_retired_v1(budget)?;
            }
            SourceOnlyPredecessorV7::Legacy {
                held,
                preservation,
                repomap_locator,
            } => {
                if let Some(locator) = repomap_locator {
                    held.require_repomap_locator_v1(*locator, budget)?;
                }
                preservation.require_current_v1(held, budget)?;
            }
        }
        ensure!(
            self.candidate.current_root_inode_v1()? == self.root_inode,
            "V7 candidate root inode changed during the held transaction"
        );
        Ok(())
    }

    pub(crate) fn publish_v7(
        self,
        locked: &mut LockedDeploymentSelectionV1,
        history: RestoreForwardHistoryV1<'_>,
        budget: &mut WorkBudget,
        checkpoint: &mut impl FnMut(&mut WorkBudget) -> Result<()>,
    ) -> std::result::Result<DeploymentSelectionPublishedV1, DeploymentSelectionErrorV1> {
        let before = DeploymentSelectionErrorV1::RefusedBeforeRename;
        if locked.current_v1().fingerprint_v1() != self.before
            || locked.candidate_v1().fingerprint_v1() != self.after
            || !history.matches_retained_v1(self.history.as_ref().map(|verified| &verified.value))
        {
            return Err(before(IndexError::DigestConflict.into()));
        }
        locked
            .prepare_source_only_recovery_witness_v2(
                self.root_inode,
                self.installed,
                self.history.as_ref().map(|verified| &verified.value),
                budget,
            )
            .map_err(before)?;
        let published = locked.publish_with_custody_v1(budget, &mut |budget| {
            checkpoint(budget)?;
            self.require_current_v7(budget)
        });
        if published.is_ok() {
            *history.selected = self.history;
        }
        published
    }
}

/// Match canonical request identities to actual Catalog rows and lend the
/// Watch census while the old and candidate roots remain held.
pub(crate) fn verify_locked_source_only_catalog_v7(
    locked: &LockedDeploymentSelectionV1,
    evidence: &NativeCutoverSourceOnlyEvidenceV6,
    budget: &mut WorkBudget,
    checkpoint: &mut impl FnMut(&mut WorkBudget) -> Result<()>,
    verify_host: impl FnOnce(&mut [CatalogWatchPublicationV7<'_>], &mut WorkBudget) -> Result<()>,
) -> Result<VerifiedSourceOnlyRecoveryV7> {
    let mut watch_capacity = 0usize;
    for fact in &evidence.source_facts {
        budget.charge(WorkUsage {
            reads: 1,
            ..Default::default()
        })?;
        if matches!(
            fact,
            Fact::Reservation {
                evidence: NativeCutoverReservationEvidenceV1::Watch { .. },
                ..
            }
        ) {
            watch_capacity += 1;
        }
    }
    let watch_bytes = watch_capacity
        .checked_mul(std::mem::size_of::<CatalogWatchPublicationV7<'_>>())
        .and_then(|size| u64::try_from(size).ok())
        .ok_or(IndexError::ResourceLimited)?;
    let _watch_memory = if watch_bytes == 0 {
        None
    } else {
        Some(
            budget
                .reserve_memory(watch_bytes, MemoryLifetime::Temporary)?
                .ok_or(IndexError::ResourceLimited)?,
        )
    };
    let mut watches = Vec::new();
    watches.try_reserve_exact(watch_capacity)?;
    ensure!(
        watches.capacity() == watch_capacity,
        "Watch census capacity differs from its grant"
    );
    let previous = match &evidence.previous_native {
        Some(previous) => {
            let held = match &previous.publication {
                NativeCutoverPreviousPublicationV6::HistoricalV5(_) => {
                    HeldPreviousNativeV1::acquire_retained_source_v1(locked.current_v1(), budget)?
                }
                NativeCutoverPreviousPublicationV6::SourceOnlyV7(_) => {
                    HeldPreviousNativeV1::acquire_v1(locked.current_v1(), budget)?
                }
            };
            budget.charge(WorkUsage {
                reads: 1,
                bytes: u64::try_from(evidence.before_selection.document.0.len())?,
                ..Default::default()
            })?;
            let (_, boot_operation) = read_native_cutover_before_native_selector_v1(
                &evidence.before_selection.document.0,
            )?
            .ok_or(IndexError::SnapshotUnavailable)?;
            let snapshot = &held.observed_v1().snapshot;
            ensure!(
                snapshot.root_nonce == previous.coverage.previous_root_nonce.0
                    && snapshot
                        .selected
                        .as_ref()
                        .is_some_and(|selected| { selected.operation.key == boot_operation }),
                "V7 predecessor BOOT root or operation differs from the retained selection"
            );
            let previous_repomap = match &previous.publication {
                NativeCutoverPreviousPublicationV6::HistoricalV5(artifact) => {
                    let mut count = 0usize;
                    for fact in &evidence.source_facts {
                        budget.charge(WorkUsage {
                            reads: 1,
                            bytes: std::mem::size_of::<quanta_index_core::index::Digest>() as u64,
                            ..Default::default()
                        })?;
                        if matches!(fact, Fact::Reservation {
                            evidence: NativeCutoverReservationEvidenceV1::Watch { .. },
                            candidate_root_nonce: Some(nonce),
                            ..
                        } if *nonce == previous.coverage.previous_root_nonce)
                        {
                            count += 1;
                        }
                    }
                    verify_historical_publication_v7(
                        &held,
                        previous,
                        artifact,
                        evidence,
                        &mut watches,
                        budget,
                    )?;
                    ensure!(
                        watches.len() == count,
                        "historical Watch Catalog count differs"
                    );
                    verify_historical_repomap_v7(locked, &held, artifact, budget)?
                }
                NativeCutoverPreviousPublicationV6::SourceOnlyV7(lineage) => {
                    let _verified_predecessor = verify_held_publications_v7(
                        locked.current_v1(),
                        &held,
                        &lineage.initial_full,
                        &lineage.successors,
                        budget,
                    )?;
                    ensure!(
                        held.selected_config_v1().legacy_repomap_selection.is_none(),
                        "V7 previous Native retains an unclaimed RepoMap selection"
                    );
                    None
                }
            };
            Some((previous_repomap, held))
        }
        None => {
            ensure!(
                locked.current_v1().native_command_v1().is_none(),
                "V7 Native predecessor evidence is absent"
            );
            None
        }
    };
    let candidate = HeldPreviousNativeV1::acquire_v1(locked.candidate_v1(), budget)?;
    ensure!(
        candidate
            .selected_config_v1()
            .legacy_repomap_selection
            .is_none(),
        "V7 candidate config retains a RepoMap selection"
    );
    if let Some((_, previous)) = &previous {
        ensure!(
            previous.observed_v1().snapshot.root_nonce
                != candidate.observed_v1().snapshot.root_nonce,
            "V7 candidate aliases the previous Native root nonce"
        );
    }
    let published = verify_held_publications_v7(
        locked.candidate_v1(),
        &candidate,
        &evidence.initial_full,
        &evidence.successors,
        budget,
    )?;
    let selected = candidate
        .observed_v1()
        .snapshot
        .selected
        .as_ref()
        .ok_or(IndexError::SnapshotUnavailable)?;
    let install_catalog_digest = locked
        .candidate_v1()
        .native_command_v1()
        .and_then(|command| command.install_catalog_digest)
        .ok_or(IndexError::SnapshotUnavailable)?;
    if locked.requires_candidate_install_attestation_v1() {
        ensure!(
            candidate.install_catalog_digest_v1(budget)? == install_catalog_digest,
            "V7 candidate Catalog differs from the installed digest"
        );
    }
    let installed = SourceOnlyRecoveryIdentityV2 {
        boot_root_nonce: Digest(candidate.observed_v1().snapshot.root_nonce),
        boot_operation: selected.operation.key,
        install_catalog_digest,
        source_operation: published.operation,
        source_request: published.request,
        source_view: published.view,
        source_cut: published.source,
    };
    let root_inode = candidate.current_root_inode_v1()?;
    checkpoint(budget)?;
    append_source_only_watches_v7(
        evidence,
        Digest(candidate.observed_v1().snapshot.root_nonce),
        &mut watches,
        budget,
    )?;
    verify_host(&mut watches, budget)?;
    let command = locked
        .candidate_v1()
        .native_command_v1()
        .ok_or(IndexError::SnapshotUnavailable)?;
    let previous = if let Some((repomap, previous)) = previous {
        super::preserved_native_v1::verify_preserved_source_only_native_v7(
            &previous,
            &candidate,
            command,
            evidence,
            repomap.is_some(),
            budget,
        )?;
        super::verify_preserved_native_history_v7(&previous, &candidate, budget)?;
        SourceOnlyPredecessorV7::Native {
            held: previous,
            repomap,
        }
    } else {
        let (lease, memory) =
            super::super::acquire_recovery_root_v1(locked.current_v1().state_root_v1(), budget)?;
        let legacy = super::super::legacy_read_factory::LegacyReadAuthorityV1::acquire(
            locked.current_v1(),
            candidate.native_io_v1(),
            lease,
            memory,
            budget,
        )?;
        let config = candidate.selected_config_v1();
        let registry = super::super::super::compose_boot_registry(config)?;
        let proof = super::preserved_sources_v1::verify_preserved_sources_source_only_v7(
            &legacy,
            candidate.authority_v7(),
            command,
            &registry,
            evidence,
            config.resources.physical.inventory.max_entries,
            budget,
        )?
        .ok_or(IndexError::SnapshotUnavailable)?;
        let publication = evidence
            .legacy_publication
            .as_ref()
            .ok_or(IndexError::SnapshotUnavailable)?;
        let history_count =
            verify_preserved_legacy_history_v7(&legacy, &candidate, evidence, publication, budget)?;
        ensure!(
            proof.has_history_rows == (history_count != 0),
            "Source-only FD6 Legacy History census changed under the held root"
        );
        let repomap_locator = match &publication.repomap {
            NativeCutoverEvidenceLegacyRepoMapV1::Required {
                state_root,
                locator_digest,
                publish,
                activation,
            } => {
                ensure!(
                    std::path::Path::new(state_root) == legacy.root(),
                    "Source-only FD6 separate RepoMap root has no saved retirement policy"
                );
                let scope = &publication.activation.active.generation.lexical;
                if publish.mutation.repo_id != scope.repo_id
                    || publish.mutation.revision_id != scope.revision_id
                {
                    return Err(IndexError::DigestConflict.into());
                }
                legacy.require_repomap_locator_v1(*locator_digest, budget)?;
                // The preservation proof already read the old compiled
                // objects. Rejoin the primary acknowledgements
                // without constraining the old Catalog to a single repo.
                verify_legacy_active_repomap_receipts_with_siblings_v1(
                    legacy.root(),
                    legacy.catalog(),
                    publish,
                    activation,
                    budget,
                )?;
                Some(*locator_digest)
            }
            NativeCutoverEvidenceLegacyRepoMapV1::NotApplicable { .. } => None,
        };
        legacy.retire_catalog(budget)?;
        SourceOnlyPredecessorV7::Legacy {
            held: legacy,
            preservation: proof,
            repomap_locator,
        }
    };
    let history = candidate.measure_boot_installed_history_v7(budget)?;
    checkpoint(budget)?;
    let verified = VerifiedSourceOnlyRecoveryV7 {
        before: locked.current_v1().fingerprint_v1(),
        after: locked.candidate_v1().fingerprint_v1(),
        candidate,
        previous,
        installed,
        root_inode,
        history,
    };
    verified.require_current_v7(budget)?;
    Ok(verified)
}

fn verify_preserved_legacy_history_v7(
    legacy: &super::super::legacy_read_factory::LegacyReadAuthorityV1,
    candidate: &HeldPreviousNativeV1,
    evidence: &NativeCutoverSourceOnlyEvidenceV6,
    publication: &NativeCutoverEvidenceLegacyPublicationV1,
    budget: &mut WorkBudget,
) -> Result<u64> {
    // Collect only funded keys and immutable object references. Each carrier
    // is opened in turn while the previous Catalog lends its held snapshot.
    let installed = super::collect_native_history_v1(
        candidate,
        super::NativeHistoryCensusViewV1::Boot,
        budget,
    )?;
    let producer = LegacyHistoryProducerScopeV1 {
        writer_state_root: &evidence.header.writer_state_root,
        workspace_root: &evidence.header.canonical_workspace_root,
        contract_key: &evidence.header.contract_key,
    };
    let required = publication.history.as_ref();
    let scope = &publication.activation.active.generation.lexical;
    let count = candidate.with_catalog_publication_reader_v1(budget, |reader, _, budget| {
        Ok(verify_legacy_history_source_inventory_all_v2(
            legacy.root(),
            legacy.catalog(),
            (&scope.repo_id, &scope.revision_id),
            producer,
            budget,
            |budget, verify| {
                let mut found_required = required.is_none();
                for member in &installed.entries {
                    budget.checkpoint()?;
                    let cut = &member.value.cut;
                    let reference = member.source_input;
                    let stored = reader.get(reference, budget)?;
                    with_native_history_boot_source_v1(
                        reference,
                        &stored,
                        budget,
                        |origin, budget| {
                            found_required |= super::super::verify_legacy_history_boot_source_v1(
                                cut, &origin, required, verify, budget,
                            )?
                            .unwrap_or(false);
                            Ok(())
                        },
                    )?;
                }
                if !found_required {
                    return Err(IndexError::DigestConflict);
                }
                Ok(())
            },
        )?)
    })?;
    Ok(count)
}

fn verify_historical_repomap_v7(
    locked: &LockedDeploymentSelectionV1,
    held: &HeldPreviousNativeV1,
    artifact: &NativeCutoverHistoricalV5ArtifactV6,
    budget: &mut WorkBudget,
) -> Result<Option<super::super::VerifiedLegacyRepoMapV1>> {
    let config = held.selected_config_v1();
    let NativeCutoverHistoricalRepoMapV6::Required {
        locator_digest,
        publish,
        activation,
    } = &artifact.repomap
    else {
        ensure!(
            config.legacy_repomap_selection.is_none(),
            "V7 historical Native retains an unclaimed RepoMap selection"
        );
        return Ok(None);
    };
    let selection = config
        .legacy_repomap_selection
        .as_ref()
        .ok_or(super::RestoreForwardSourceRefusalV1::NativeRepoMapVerificationUnavailable)?;
    let saved = locked.read_saved_legacy_repomap_selection_v1(
        &selection.path,
        quanta_index_core::index::Digest(selection.sha256),
        budget,
    )?;
    Ok(Some(super::verify_saved_native_separate_repomap_v1(
        &saved,
        locked.current_v1().state_root_v1(),
        locked.candidate_v1().state_root_v1(),
        locator_digest,
        (publish, activation),
        held.native_io_v1(),
        budget,
    )?))
}

fn verify_historical_publication_v7(
    held: &HeldPreviousNativeV1,
    previous: &NativeCutoverPreviousNativeV6,
    artifact: &NativeCutoverHistoricalV5ArtifactV6,
    evidence: &NativeCutoverSourceOnlyEvidenceV6,
    old_watches: &mut Vec<CatalogWatchPublicationV7<'_>>,
    budget: &mut WorkBudget,
) -> Result<()> {
    let rows = held.observe_historical_source_rows_v1(
        &evidence.source_facts,
        budget,
        |operation, reservation, replay, budget| {
            // A Source operation may belong to only one lineage. History-only
            // Catalog rows never reach this visitor.
            let search = 2 + evidence.successors.len().max(1).ilog2() as u64;
            budget.charge(WorkUsage {
                reads: search,
                bytes: search * std::mem::size_of::<OperationKey>() as u64,
                ..Default::default()
            })?;
            if operation == evidence.initial_full.target.operation
                || evidence
                    .successors
                    .binary_search_by_key(&operation, |claim| claim.target.operation)
                    .is_ok()
            {
                return Err(IndexError::DigestConflict);
            }
            if matches!(
                reservation,
                NativeCutoverReservationEvidenceV1::Watch { .. }
            ) {
                let Fact::Replay {
                    format_version: 3 | 4,
                    state_revision,
                    published_snapshot_id: Some(snapshot),
                    ..
                } = replay
                else {
                    return Err(IndexError::DigestConflict);
                };
                if *state_revision == 0
                    || *snapshot == 0
                    || old_watches.len() == old_watches.capacity()
                {
                    return Err(IndexError::DigestConflict);
                }
                old_watches.push(CatalogWatchPublicationV7 {
                    operation,
                    snapshot: *snapshot,
                    source: None,
                });
            }
            Ok(())
        },
    )?;
    ensure!(
        rows.old_head_operation == artifact.identity.operation,
        "V7 historical Native artifact differs from the actual Source head"
    );
    let mut source = None;
    let mut replay_matched = false;
    for fact in &evidence.source_facts {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: (std::mem::size_of::<quanta_index_core::index::OperationKey>() + 5 * 64 + 8)
                as u64,
            ..Default::default()
        })?;
        match fact {
            Fact::NativeInitialSourceBinding {
                operation,
                source: binding,
                ..
            }
            | Fact::NativeSdkSourceBinding {
                operation,
                source: binding,
                ..
            }
            | Fact::CompletedSourceRequest {
                native_operation: Some(operation),
                source: binding,
                ..
            } if *operation == artifact.identity.operation => {
                ensure!(
                    source.replace(binding).is_none(),
                    "V7 historical Native head has competing completed Source facts"
                );
            }
            Fact::Replay {
                native_update_request: Some(request),
                format_version,
                state_revision,
                phase,
                published_snapshot_id,
                prepared_artifact_digest,
                source_bound_aggregate_receipt_digest,
                ..
            } if request.operation == artifact.identity.operation => {
                ensure!(
                    !std::mem::replace(&mut replay_matched, true)
                        && matches!(*format_version, 3 | 4)
                        && *state_revision != 0
                        && matches!(
                            phase,
                            NativeCutoverReplayPhaseV1::Acked | NativeCutoverReplayPhaseV1::Cleaned
                        )
                        && *published_snapshot_id == Some(artifact.identity.published_snapshot_id)
                        && *prepared_artifact_digest
                            == Some(artifact.identity.prepared_artifact_digest)
                        && *source_bound_aggregate_receipt_digest
                            == Some(artifact.identity.aggregate_receipt_digest),
                    "V7 historical Native artifact lacks its exact settled V9 replay"
                );
            }
            _ => {}
        }
    }
    ensure!(replay_matched, "V7 historical Native replay is absent");
    let source = source.ok_or(IndexError::SnapshotUnavailable)?;
    super::require_previous_native_source_coverage_v1(
        Some(&previous.coverage),
        rows.root_nonce,
        rows.old_head_operation,
        source,
        evidence.initial_full.target.operation,
        &evidence.initial_full.source,
    )
}

fn append_source_only_watches_v7<'a>(
    evidence: &'a NativeCutoverSourceOnlyEvidenceV6,
    candidate_nonce: Digest,
    watches: &mut Vec<CatalogWatchPublicationV7<'a>>,
    budget: &mut WorkBudget,
) -> Result<()> {
    let fact_count = evidence.source_facts.len();
    if fact_count == 0 || fact_count > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1 {
        return Err(IndexError::DigestConflict.into());
    }
    let bytes = fact_count * std::mem::size_of::<usize>();
    let _scratch = budget
        .reserve_memory(bytes as u64, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut rows = Vec::new();
    rows.try_reserve_exact(fact_count)?;
    if rows.capacity() != fact_count {
        return Err(IndexError::ResourceLimited.into());
    }
    let facts = NativeCutoverPublicationFactsV1::from_facts_v1(
        &evidence.source_facts,
        &mut rows,
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
    let previous =
        evidence
            .previous_native
            .as_ref()
            .and_then(|previous| match &previous.publication {
                NativeCutoverPreviousPublicationV6::SourceOnlyV7(lineage) => Some((
                    previous.coverage.previous_root_nonce,
                    &lineage.initial_full,
                    &lineage.successors,
                )),
                NativeCutoverPreviousPublicationV6::HistoricalV5(_) => None,
            });
    // Both endpoint lineages have already matched actual Catalog publications.
    // Existing reservations without a Published row do not issue Watch events.
    let candidate = (
        candidate_nonce,
        &evidence.initial_full,
        &evidence.successors,
    );
    for (nonce, initial, successors) in std::iter::once(candidate).chain(previous) {
        let mut prior = None;
        for claim in std::iter::once(initial).chain(successors) {
            let search = 2 * (u64::from(fact_count.ilog2()) + 2);
            budget.charge(WorkUsage {
                reads: search,
                bytes: search * std::mem::size_of::<OperationKey>() as u64,
                ..Default::default()
            })?;
            let matches = facts.for_operation_v1(claim.target.operation);
            if require_source_only_watch_reservation_v7(
                matches,
                &claim.target,
                nonce,
                prior,
                budget,
            )? {
                if watches.len() == watches.capacity() {
                    return Err(IndexError::ResourceLimited.into());
                }
                watches.push(CatalogWatchPublicationV7 {
                    operation: claim.target.operation,
                    snapshot: claim.source.target_snapshot_id,
                    source: Some(&claim.source),
                });
            }
            prior = Some(&claim.target);
        }
    }
    Ok(())
}

fn require_source_only_watch_reservation_v7<'a>(
    facts: impl Iterator<Item = &'a Fact>,
    claimed: &NativeCutoverEvidenceTargetV1,
    nonce: Digest,
    prior: Option<&NativeCutoverEvidenceTargetV1>,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let mut kind = None;
    let mut legacy_replay = false;
    for fact in facts {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: std::mem::size_of_val(claimed) as u64,
            ..Default::default()
        })?;
        match fact {
            Fact::Reservation {
                target,
                evidence,
                candidate_root_nonce,
            } => {
                if target != claimed
                    || *candidate_root_nonce != Some(nonce)
                    || kind.replace(evidence).is_some()
                {
                    return Err(IndexError::DigestConflict);
                }
            }
            Fact::Replay { .. } => legacy_replay = true,
            _ => {}
        }
    }
    use NativeCutoverReservationEvidenceV1::{Initial, Sdk, Watch};
    match (kind, prior) {
        (Some(Initial), None) => Ok(false),
        (
            Some(
                kind @ (Watch {
                    prior_target_digest,
                    ..
                }
                | Sdk {
                    prior_target_digest,
                    ..
                }),
            ),
            Some(prior),
        ) if *prior_target_digest == prior.canonical_target_digest
            && claimed.owner_identity_policy_digest == prior.owner_identity_policy_digest =>
        {
            let watch = matches!(kind, Watch { .. });
            if watch && legacy_replay {
                return Err(IndexError::DigestConflict);
            }
            Ok(watch)
        }
        _ => Err(IndexError::DigestConflict),
    }
}

fn verify_held_publications_v7(
    document: &DeploymentSelectionDocumentV1,
    held: &HeldPreviousNativeV1,
    initial: &NativeCutoverSourceOnlyPublicationV7,
    successors: &[NativeCutoverSourceOnlyPublicationV7],
    budget: &mut WorkBudget,
) -> Result<VerifiedSourcePublicationV7> {
    let count = successors
        .len()
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    if count > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1 {
        return Err(IndexError::ResourceLimited.into());
    }
    let _seen_memory = budget
        .reserve_memory(u64::try_from(count)?, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut seen = Vec::<u8>::new();
    seen.try_reserve_exact(count)?;
    if seen.capacity() > count {
        return Err(IndexError::ResourceLimited.into());
    }
    seen.resize(count, 0);
    let current = successors.last().unwrap_or(initial);
    let expected = held.observed_v1();
    let root = document.state_root_v1();
    let workspace = current.target.operation.workspace;
    let catalog_io = held.catalog_io_v1();
    let operation_key_bytes =
        u64::try_from(std::mem::size_of::<quanta_index_core::index::OperationKey>())?;
    let mut published_initial = None;
    let mut published_current = None;
    let mut visit = |reader: &FileObjectReader,
                     catalog: &NativeBootPublicationReader<'_>,
                     head: &CatalogView,
                     op: &ImmutableOperation,
                     view: &CatalogView,
                     budget: &mut WorkBudget| {
        let matched = if op.key == initial.target.operation {
            Some((0, initial))
        } else {
            let mut low = 0;
            let mut high = successors.len();
            while low < high {
                budget.charge(WorkUsage {
                    reads: 1,
                    bytes: operation_key_bytes,
                    ..WorkUsage::default()
                })?;
                let middle = low + (high - low) / 2;
                if successors[middle].target.operation < op.key {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            successors
                .get(low)
                .filter(|claim| claim.target.operation == op.key)
                .map(|claim| (low + 1, claim))
        };
        let Some((index, claim)) = matched else {
            // The candidate's signed lineage must account for every Published
            // Source operation in this exact stream between its two endpoints.
            let first = initial.target.operation;
            let last = current.target.operation;
            if op.key.workspace == first.workspace
                && op.key.epoch == first.epoch
                && op.key.producer == first.producer
                && op.key.stream == first.stream
                && (op.key.stream_epoch, op.key.sequence) >= (first.stream_epoch, first.sequence)
                && (op.key.stream_epoch, op.key.sequence) <= (last.stream_epoch, last.sequence)
                && !matches!(op.source_mode, SourceOperationMode::Unchanged)
            {
                return Err(IndexError::DigestConflict);
            }
            return Ok(());
        };
        // History-only publications preserve this cut. Any intervening Source
        // write must appear in the signed lineage, even on another stream.
        if std::mem::replace(&mut seen[index], 1) != 0
            || (index + 1 == count && head.source != view.source)
            || successors
                .get(index)
                .is_some_and(|next| view.source != next.target.expected_source)
        {
            return Err(IndexError::DigestConflict);
        }
        if op.request != claim.request_ref
            || view.source.frontiers == claim.target.expected_source.frontiers
        {
            return Err(IndexError::DigestConflict);
        }
        let request = quanta_index_kernel::identify_update_request_with(
            &claim.request,
            budget,
            |payload, children, budget| reader.identify(payload, children, budget),
        )?;
        super::super::verify_recovery_source_publication_v1(
            reader,
            &claim.target,
            &claim.request,
            op,
            view,
            request,
            budget,
            |prior, budget| catalog.published_view(prior, budget),
        )?;
        if index == 0 {
            published_initial = Some((view.identity, view.source));
        }
        if index + 1 == count {
            published_current = Some(VerifiedSourcePublicationV7 {
                operation: op.key,
                request: op.request,
                view: view.identity,
                source: view.source,
            });
        }
        Ok(())
    };
    let observed = held.with_catalog_publication_reader_v1(budget, |reader, routes, budget| {
        let observed = if let Some(routes) = routes {
            verify_native_source_root_publications_with_routes_v1(
                root,
                catalog_io,
                workspace,
                routes,
                budget,
                |_, catalog, head, op, view, budget| visit(reader, catalog, head, op, view, budget),
            )
        } else {
            verify_native_source_root_publications_v1(
                root,
                catalog_io,
                workspace,
                budget,
                |_, catalog, head, op, view, budget| visit(reader, catalog, head, op, view, budget),
            )
        }?;
        Ok(observed)
    })?;
    let before = &expected.snapshot;
    budget.charge(WorkUsage {
        reads: u64::try_from(count)?,
        bytes: u64::try_from(count)?,
        ..WorkUsage::default()
    })?;
    ensure!(
        seen.iter().all(|found| *found == 1)
            && observed.snapshot.root_nonce == before.root_nonce
            && observed.snapshot.selected == before.selected
            && observed.snapshot.namespace == before.namespace
            && observed.current_head == expected.current_head,
        "V7 Source publications or held Catalog identity differ"
    );
    let boot = before
        .selected
        .as_ref()
        .ok_or(IndexError::SnapshotUnavailable)?;
    let (initial_view, initial_source) =
        published_initial.ok_or(IndexError::SnapshotUnavailable)?;
    if boot.operation.key.workspace != initial_view.workspace
        || boot.operation.key.epoch != initial_view.epoch
        || boot.publication.source != initial_source
        || boot.publication.identity.sequence < initial_view.sequence
        || (boot.operation.key != initial.target.operation
            && boot.operation.source_mode != SourceOperationMode::Unchanged)
    {
        return Err(IndexError::DigestConflict.into());
    }
    published_current.ok_or_else(|| IndexError::SnapshotUnavailable.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::{
        CollectionId, MapRoot, ObjectRef, ProducerId, SlotId, SourceCut, SourceTextAdmission,
        StreamId, TenantId, ViewRef, WorkspaceKey, WorktreeId,
    };
    use quanta_index_core::index::WorkLimits;
    use std::time::{Duration, Instant};

    #[test]
    fn source_only_watch_requires_exact_root_role_and_predecessor() {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let nonce = Digest([5; 32]);
        let prior = NativeCutoverEvidenceTargetV1 {
            operation: OperationKey {
                workspace,
                epoch: 1,
                producer: ProducerId([6; 16]),
                stream: StreamId([7; 16]),
                stream_epoch: 1,
                sequence: 1,
            },
            expected_view: ViewRef {
                workspace,
                epoch: 1,
                sequence: 0,
                digest: Digest([8; 32]),
            },
            expected_source: SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            expected_profile: ObjectRef(Digest([9; 32])),
            source_mode: SourceOperationMode::Advance,
            event_predecessor: 0,
            event_sequence: 1,
            owner_identity_policy_digest: Digest([10; 32]),
            text_admission: SourceTextAdmission::Excluded,
            canonical_target_digest: Digest([11; 32]),
        };
        let successor = NativeCutoverEvidenceTargetV1 {
            operation: OperationKey {
                sequence: 2,
                ..prior.operation
            },
            event_predecessor: 1,
            event_sequence: 2,
            canonical_target_digest: Digest([12; 32]),
            ..prior
        };
        let checked = |facts: &[Fact], target: &NativeCutoverEvidenceTargetV1, previous| {
            let mut budget = WorkBudget::new(
                WorkLimits {
                    reads: 16,
                    writes: 0,
                    bytes: 4096,
                    mutations: 0,
                },
                Instant::now() + Duration::from_secs(10),
            );
            require_source_only_watch_reservation_v7(
                facts.iter(),
                target,
                nonce,
                previous,
                &mut budget,
            )
        };
        let initial = Fact::Reservation {
            target: prior,
            evidence: NativeCutoverReservationEvidenceV1::Initial,
            candidate_root_nonce: Some(nonce),
        };
        assert!(!checked(&[initial.clone()], &prior, None).unwrap());
        assert!(checked(&[initial], &prior, Some(&prior)).is_err());
        let watch = Fact::Reservation {
            target: successor,
            evidence: NativeCutoverReservationEvidenceV1::Watch {
                idempotency_key: "watch".into(),
                verified_batch_digest: Digest([13; 32]),
                prior_target_digest: prior.canonical_target_digest,
            },
            candidate_root_nonce: Some(nonce),
        };
        assert!(checked(&[watch.clone()], &successor, Some(&prior)).unwrap());
        assert!(checked(&[], &successor, Some(&prior)).is_err());
        assert!(checked(&[watch.clone(), watch.clone()], &successor, Some(&prior)).is_err());
        let foreign_prior = NativeCutoverEvidenceTargetV1 {
            canonical_target_digest: Digest([14; 32]),
            ..prior
        };
        assert!(checked(&[watch.clone()], &successor, Some(&foreign_prior)).is_err());
        let Fact::Reservation {
            target, evidence, ..
        } = watch
        else {
            unreachable!()
        };
        let foreign_root = Fact::Reservation {
            target,
            evidence,
            candidate_root_nonce: Some(Digest([15; 32])),
        };
        assert!(checked(&[foreign_root], &successor, Some(&prior)).is_err());
    }
}
