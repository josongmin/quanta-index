//! Borrowed Original Source associations and Host facts remain data until the
//! receiver joins them with its live Q authority and retained descriptors.

use anyhow::{Result, anyhow, ensure};
use quanta_index_contract::{
    native_cutover_evidence_v1::{
        NativeCutoverEvidenceFreshSdkV1, NativeCutoverEvidenceHostFactsV1,
        NativeCutoverEvidenceHostRecoveryReceiptV1,
        NativeCutoverEvidenceOrdinalV1, NativeCutoverEvidenceSourcePublicationBindingV1,
        NativeCutoverEvidenceSourceFactV1, NativeCutoverEvidenceV1,
    },
    native_cutover_source_join_v1::{
        NativeCutoverJoinedSourceFactsV1, NativeCutoverSourceJoinErrorV1,
        join_native_cutover_source_facts_v1,
    },
};
use quanta_index_core::index::{MemoryLifetime, WorkBudget, WorkUsage};

#[derive(Debug)]
pub(super) enum FreshSourceParityRefusalV1 {
    InvalidDigestEncoding,
    SdkTargetMismatch,
    OrdinaryTargetMismatch,
    CompetingLegacyBindings,
}

impl std::fmt::Display for FreshSourceParityRefusalV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidDigestEncoding => "fresh Source identity is not a canonical digest",
            Self::SdkTargetMismatch => {
                "fresh SDK Source identity differs from the original SDK ACK"
            }
            Self::OrdinaryTargetMismatch => {
                "ordinary initial Source binding differs from the fresh SDK Source"
            }
            Self::CompetingLegacyBindings => {
                "ordinary initial and change-event SDK bindings both claim current Legacy"
            }
        })
    }
}

impl std::error::Error for FreshSourceParityRefusalV1 {}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum RetainedHostRefusalV1 {
    PendingApplyOrOccupancy,
    IncompleteDelivery,
    InvalidDeliveryIdentity,
    InvalidOrdinal,
    PendingRecoveryOrdinal,
    ReconciliationRequired,
    SourceOnlyOrUnmatched,
    ConflictingSourceEcho,
    DuplicateEmittedEvent,
    MissingRecoveryReceipt,
    InvalidRecoveryReceipt,
    DuplicateRecoveryReceipt,
}

impl std::fmt::Display for RetainedHostRefusalV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PendingApplyOrOccupancy => "Host pending apply or retained occupancy differs",
            Self::IncompleteDelivery => "Host delivery lacks terminal cursor custody",
            Self::InvalidDeliveryIdentity => "Host delivery identity or prepared authority differs",
            Self::InvalidOrdinal => "Host ordinal disposition or terminal custody differs",
            Self::PendingRecoveryOrdinal => "Host recovery receipt covers an ordinal without a terminal decision",
            Self::ReconciliationRequired => "Host reconciliation ordinal needs an exact recovery receipt",
            Self::SourceOnlyOrUnmatched => "Host event has no completed Source request association",
            Self::ConflictingSourceEcho => "retained Host echo conflicts with completed Source",
            Self::DuplicateEmittedEvent => "Host emitted event ID occurs in multiple retained ordinals",
            Self::MissingRecoveryReceipt => "Host recovery ordinal has no exact checkpoint-owner receipt",
            Self::InvalidRecoveryReceipt => "Host recovery receipt, tail, checkpoint, or Source identity differs",
            Self::DuplicateRecoveryReceipt => "Host recovery request identity occurs in multiple retained receipts",
        })
    }
}

impl std::error::Error for RetainedHostRefusalV1 {}

fn require_ordinary_source_binding_v1(
    binding: &NativeCutoverEvidenceSourcePublicationBindingV1,
    fresh: &NativeCutoverEvidenceFreshSdkV1,
    budget: &mut WorkBudget,
) -> Result<()> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 3 * 64 + 8,
        ..WorkUsage::default()
    })?;
    if binding.transition_digest != fresh.source_manifest_digest
        || binding.completion_pointer_digest != fresh.source_pointer_digest
        || binding.physical_target_source_digest != fresh.physical_target_source_digest
        || binding.target_snapshot_id != fresh.target_snapshot_id
    {
        return Err(FreshSourceParityRefusalV1::OrdinaryTargetMismatch.into());
    }
    Ok(())
}

/// The canonical Source join binds the required live committed closure seal
/// to the independent fresh physical target and exact original V5/V9 facts.
/// The selected ordinary or change-event Source binding must describe that
/// same target. Neither scalar comparison is a standalone publication permit.
pub(super) fn require_fresh_source_parity_v1(
    joined: &NativeCutoverJoinedSourceFactsV1<'_>,
    budget: &mut WorkBudget,
) -> Result<()> {
    budget.checkpoint()?;
    let fresh = joined.fresh_sdk;
    budget.charge(WorkUsage {
        bytes: 6 * 64,
        ..WorkUsage::default()
    })?;
    for digest in [
        fresh.sdk_generation_digest,
        fresh.source_pointer_digest,
        fresh.source_manifest_digest,
        fresh.physical_target_source_digest,
        fresh.committed_source_snapshot_digest,
        joined.v5_artifact.lexical_revision_digest,
    ] {
        if !digest
            .0
            .iter()
            .copied()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(FreshSourceParityRefusalV1::InvalidDigestEncoding.into());
        }
    }
    if let Some(ordinary) = joined.current_legacy_ordinary_binding {
        if joined.current_legacy_sdk_ack.is_some() {
            return Err(FreshSourceParityRefusalV1::CompetingLegacyBindings.into());
        }
        require_ordinary_source_binding_v1(ordinary, fresh, budget)?;
    }
    let sdk_fact = if joined.current_legacy_ordinary_binding.is_some() {
        None
    } else {
        joined.current_legacy_sdk_ack.or(joined.sdk_acked_binding)
    };
    let selected_sdk_binding = match sdk_fact {
        Some(NativeCutoverEvidenceSourceFactV1::PendingStoreSdkAck { binding, .. }) => {
            Some(binding)
        }
        _ => None,
    };
    if let Some(binding) = selected_sdk_binding {
        budget.charge(WorkUsage {
            bytes: u64::try_from(
                binding
                    .target_source_digest
                    .len()
                    .saturating_add(binding.transition_digest.len())
                    .saturating_add(binding.completion_pointer_digest.len()),
            )?,
            ..WorkUsage::default()
        })?;
        if binding.target_snapshot != fresh.target_snapshot_id
            || binding.target_source_digest.as_bytes()
                != fresh.physical_target_source_digest.0.as_slice()
            || binding.transition_digest.as_bytes() != fresh.source_manifest_digest.0.as_slice()
            || binding.completion_pointer_digest.as_bytes()
                != fresh.source_pointer_digest.0.as_slice()
        {
            return Err(FreshSourceParityRefusalV1::SdkTargetMismatch.into());
        }
    }
    // S's retained Original reader compares its closed physical scan with the
    // durable pointer before reading the manifest/heads and both finishers.
    // The admitted signed peer and retained descriptors remain necessary;
    // these scalar comparisons do not issue a standalone permission token.
    budget.checkpoint()?;
    Ok(())
}

fn charge_host_v1(budget: &mut WorkBudget, bytes: usize) -> Result<()> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: u64::try_from(bytes)?,
        ..WorkUsage::default()
    })?;
    Ok(())
}

fn canonical_host_digest_v1(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| {
        byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
    })
}

fn recovery_receipt_for_digest_v1<'a>(
    host: &'a NativeCutoverEvidenceHostFactsV1,
    request_digest: &str,
    budget: &mut WorkBudget,
) -> Result<&'a NativeCutoverEvidenceHostRecoveryReceiptV1> {
    let mut found = None;
    for receipt in &host.recovery_receipts {
        charge_host_v1(budget, receipt.request_identity_digest.len())?;
        if receipt.request_identity_digest == request_digest {
            if found.replace(receipt).is_some() {
                return Err(RetainedHostRefusalV1::DuplicateRecoveryReceipt.into());
            }
        }
    }
    found.ok_or_else(|| RetainedHostRefusalV1::MissingRecoveryReceipt.into())
}

/// This is a comparison of the checkpoint owner's re-read receipt DATA. The
/// S loan must have produced the DTO from its genuine typed receipt while the
/// retained inbox and Source census were held; these scalar checks cannot
/// mint a recovery or Source publication authority.
/// A full recovery covers its exact tail through the current workspace Source
/// proof. Its events need not have an earlier completed Watch request.
fn require_recovery_receipt_v1(
    receipt: &NativeCutoverEvidenceHostRecoveryReceiptV1,
    request_digest: &str,
    event_id: u128,
    workspace_root: &str,
    host_workspace_root: &str,
    fresh: &NativeCutoverEvidenceFreshSdkV1,
    budget: &mut WorkBudget,
) -> Result<()> {
    let text = [
        receipt.operation_id.as_str(),
        receipt.canonical_workspace_root.as_str(),
        receipt.request_identity_digest.as_str(),
        receipt.reason.as_str(),
        receipt.tail_custody_digest.as_str(),
        receipt.checkpoint_payload_digest.as_str(),
        receipt.applied_operation_id.as_str(),
        receipt.applied_request_identity_digest.as_str(),
        receipt.applied_source_owner_identity.as_str(),
        receipt.applied_source_execution_authority_digest.as_str(),
        receipt.applied_source_view_digest.as_str(),
        receipt.applied_analysis_owner_identity.as_str(),
        receipt.applied_analysis_lineage_digest.as_str(),
        receipt.applied_index_owner_identity.as_str(),
        receipt.applied_index_publication_identity.as_str(),
        receipt.applied_index_publication_digest.as_str(),
    ];
    let text_bytes = text.iter().try_fold(0usize, |sum, value| sum.checked_add(value.len()))
        .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
    let tail_bytes = receipt.tail_event_ids.len()
        .checked_mul(std::mem::size_of::<u128>())
        .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
    charge_host_v1(budget, text_bytes.checked_add(tail_bytes)
        .and_then(|sum| sum.checked_add(workspace_root.len()))
        .and_then(|sum| sum.checked_add(host_workspace_root.len()))
        .and_then(|sum| sum.checked_add(receipt.observation_cut_digest.as_ref().map_or(0, String::len)))
        .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?)?;
    receipt.validate_request_identity_v1()
        .map_err(|_| RetainedHostRefusalV1::InvalidRecoveryReceipt)?;
    if text.iter().any(|value| value.trim().is_empty())
        || receipt.canonical_workspace_root != workspace_root
        || receipt.canonical_workspace_root != host_workspace_root
        || !canonical_host_digest_v1(request_digest)
        || receipt.request_identity_digest != request_digest
        || receipt.operation_id != receipt.applied_operation_id
        || receipt.request_identity_digest != receipt.applied_request_identity_digest
        || !canonical_host_digest_v1(&receipt.tail_custody_digest)
        || !canonical_host_digest_v1(&receipt.checkpoint_payload_digest)
        || !canonical_host_digest_v1(&receipt.applied_source_view_digest)
        || receipt.observation_cut_digest.as_deref()
            .is_some_and(|digest| !canonical_host_digest_v1(digest))
        || receipt.checkpoint_generation == 0
        || receipt.published_snapshot_id == 0
        || receipt.published_snapshot_id != receipt.applied_published_snapshot_id
        || receipt.published_snapshot_id != fresh.committed_source_snapshot_id
        || receipt.published_snapshot_id < receipt.snapshot_before.max(receipt.required_snapshot_floor)
        || receipt.applied_source_view_digest.as_bytes()
            != fresh.committed_source_snapshot_digest.0.as_slice()
        || receipt.tail_event_ids.is_empty()
        || receipt.tail_event_ids.windows(2).any(|pair| pair[0] >= pair[1])
        || receipt.tail_event_ids.binary_search(&event_id).is_err()
    {
        return Err(RetainedHostRefusalV1::InvalidRecoveryReceipt.into());
    }
    budget.checkpoint()?;
    Ok(())
}

/// The Source join has proved the completed request census, including the
/// exact Native Watch V9 association for Some(operation) and the explicit
/// Source-only None operation. Event IDs are globally unique across both.
/// A missing completed request is a refusal, never an inferred publication.
fn completed_source_for_event_v1<'a>(
    joined: &NativeCutoverJoinedSourceFactsV1<'a>,
    event_id: u128,
    budget: &mut WorkBudget,
) -> Result<&'a NativeCutoverEvidenceSourcePublicationBindingV1> {
    let (binding, _, _) = joined.completed_source_request_for_event_v1(event_id, &mut |units| {
        budget.charge(WorkUsage { reads: 1, bytes: units, ..WorkUsage::default() })
    })?
        .ok_or(RetainedHostRefusalV1::SourceOnlyOrUnmatched)?;
    // The S Original lender already checked every completed step against the
    // current manifest/pointer ancestry under one retained Source/Output loan.
    // An older Source-only step need not equal the fresh pointer's four axes.
    Ok(binding)
}

/// Pure ordinal shape. `source` is supplied only after the authenticated
/// completed-Source lookup above; this helper grants no independent authority.
fn require_settled_host_ordinal_v1(
    ordinal: &NativeCutoverEvidenceOrdinalV1,
    source: Option<&NativeCutoverEvidenceSourcePublicationBindingV1>,
    budget: &mut WorkBudget,
) -> Result<Option<u128>> {
    charge_host_v1(budget, std::mem::size_of::<NativeCutoverEvidenceOrdinalV1>()
        .saturating_add(ordinal.terminal.as_ref().map_or(0, String::len))
        .saturating_add(ordinal.source_transition_digest.as_ref().map_or(0, String::len))
        .saturating_add(ordinal.target_source_digest.as_ref().map_or(0, String::len)))?;
    if ordinal.reconciliation_event_id.is_some() {
        return Err(RetainedHostRefusalV1::ReconciliationRequired.into());
    }
    if ordinal.recovery_request_digest.is_some() {
        return Err(RetainedHostRefusalV1::InvalidOrdinal.into());
    }
    if ordinal.filtered_no_semantic_work {
        if ordinal.admitted
            || ordinal.emitted_event_id.is_some()
            || ordinal.terminal.as_deref() != Some("NoSemanticWork")
            || ordinal.source_transition_digest.is_some()
            || ordinal.target_source_digest.is_some()
            || ordinal.target_snapshot_id.is_some()
        {
            return Err(RetainedHostRefusalV1::InvalidOrdinal.into());
        }
        return Ok(None);
    }
    let event_id = ordinal.emitted_event_id
        .ok_or(RetainedHostRefusalV1::InvalidOrdinal)?;
    if !matches!(ordinal.terminal.as_deref(), Some("Applied" | "AlreadyPublished")) {
        return Err(RetainedHostRefusalV1::InvalidOrdinal.into());
    }
    if ordinal.terminal.as_deref() == Some("Applied") && !ordinal.admitted {
        return Err(RetainedHostRefusalV1::InvalidOrdinal.into());
    }
    let source = source.ok_or(RetainedHostRefusalV1::SourceOnlyOrUnmatched)?;
    let echo = (&ordinal.source_transition_digest, &ordinal.target_source_digest,
        ordinal.target_snapshot_id);
    match echo {
        (None, None, None) => {}
        (Some(transition), Some(target), Some(snapshot))
            if ordinal.terminal.as_deref() == Some("AlreadyPublished")
                && transition.as_bytes() == source.transition_digest.0.as_slice()
                && target.as_bytes() == source.physical_target_source_digest.0.as_slice()
                && snapshot == source.target_snapshot_id => {}
        _ => return Err(RetainedHostRefusalV1::ConflictingSourceEcho.into()),
    }
    Ok(Some(event_id))
}

fn sift_host_event_heap_v1(
    ids: &mut [u128],
    mut root: usize,
    end: usize,
    budget: &mut WorkBudget,
) -> Result<()> {
    loop {
        let left = root.checked_mul(2).and_then(|value| value.checked_add(1))
            .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
        if left >= end { return Ok(()) }
        let right = left + 1;
        let mut largest = root;
        charge_host_v1(budget, 2 * std::mem::size_of::<u128>())?;
        if ids[largest] < ids[left] { largest = left; }
        if right < end {
            charge_host_v1(budget, 2 * std::mem::size_of::<u128>())?;
            if ids[largest] < ids[right] { largest = right; }
        }
        if largest == root { return Ok(()) }
        ids.swap(root, largest);
        root = largest;
    }
}

fn reject_duplicate_host_event_ids_v1(ids: &mut [u128], budget: &mut WorkBudget) -> Result<()> {
    let length = ids.len();
    for root in (0..length / 2).rev() {
        sift_host_event_heap_v1(ids, root, length, budget)?;
    }
    for end in (1..length).rev() {
        ids.swap(0, end);
        sift_host_event_heap_v1(ids, 0, end, budget)?;
    }
    for pair in ids.windows(2) {
        charge_host_v1(budget, 2 * std::mem::size_of::<u128>())?;
        if pair[0] == pair[1] {
            return Err(RetainedHostRefusalV1::DuplicateEmittedEvent.into());
        }
    }
    Ok(())
}

fn require_settled_retained_host_inventory_v1<'a>(
    host: &NativeCutoverEvidenceHostFactsV1,
    budget: &mut WorkBudget,
    mut source_for_event: impl FnMut(
        u128,
        &mut WorkBudget,
    ) -> Result<&'a NativeCutoverEvidenceSourcePublicationBindingV1>,
    mut recovery_for_event: impl FnMut(
        u128,
        &NativeCutoverEvidenceHostRecoveryReceiptV1,
        &mut WorkBudget,
    ) -> Result<()>,
) -> Result<()> {
    if host.pending_apply_intent.is_some() {
        return Err(RetainedHostRefusalV1::PendingApplyOrOccupancy.into());
    }
    if host.retained_delivery_count != u64::try_from(host.deliveries.len())? {
        return Err(RetainedHostRefusalV1::PendingApplyOrOccupancy.into());
    }
    let mut ordinal_count = 0_usize;
    let mut occupied_units = 0_u64;
    for delivery in &host.deliveries {
        charge_host_v1(budget, 1)?;
        ordinal_count = ordinal_count.checked_add(delivery.ordinals.len())
            .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
        occupied_units = occupied_units.checked_add(
            u64::try_from(delivery.ordinals.len().max(1))?
        ).ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
    }
    if occupied_units != host.active_units || occupied_units > host.capacity {
        return Err(RetainedHostRefusalV1::PendingApplyOrOccupancy.into());
    }
    let reserved_bytes = ordinal_count.checked_mul(std::mem::size_of::<u128>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
    let scratch_lease = if reserved_bytes == 0 { None } else {
        Some(budget.reserve_memory(reserved_bytes, MemoryLifetime::Temporary)?
            .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?)
    };
    let mut emitted_ids = Vec::<u128>::new();
    emitted_ids.try_reserve_exact(ordinal_count)?;
    ensure!(emitted_ids.capacity().checked_mul(std::mem::size_of::<u128>())
        .is_some_and(|actual| u64::try_from(actual).ok().is_some_and(|actual| actual <= reserved_bytes)),
        "Host emitted event scratch exceeded its memory grant");
    for delivery in &host.deliveries {
        charge_host_v1(budget, delivery.delivery_id.len()
            .saturating_add(delivery.record_digest.len())
            .saturating_add(delivery.cursor.len())
            .saturating_add(delivery.cursor_receipt_digest.as_ref().map_or(0, String::len))
            .saturating_add(delivery.ordinals.len()))?;
        if usize::try_from(delivery.input_count).ok() != Some(delivery.ordinals.len())
            || delivery.workspace_authority != host.workspace_authority
        {
            return Err(RetainedHostRefusalV1::IncompleteDelivery.into());
        }
        if delivery.schema_version != "quanta-change-event-durable-delivery-inbox/v2"
            || !canonical_host_digest_v1(&delivery.record_digest)
            || !canonical_host_digest_v1(&delivery.delivery_id)
            || !canonical_host_digest_v1(&delivery.payload_digest)
            || !canonical_host_digest_v1(&delivery.config_seal)
            || delivery.cursor_receipt_digest.as_deref()
                .is_some_and(|digest| !canonical_host_digest_v1(digest))
            || delivery.backend.is_empty()
            || delivery.workspace_authority.is_empty()
            || delivery.configured_root.is_empty()
            || delivery.watch_root.is_empty()
            || delivery.stream_id.is_empty()
            || delivery.cursor.is_empty()
            || delivery.mapping_schema.is_empty()
            || delivery.prior_cursor.as_deref() == Some("")
        {
            return Err(RetainedHostRefusalV1::InvalidDeliveryIdentity.into());
        }
        let mut authorized = false;
        for authority in &host.prepared_authorities {
            charge_host_v1(budget, authority.backend.len()
                .saturating_add(authority.workspace_authority.len())
                .saturating_add(authority.configured_root.len())
                .saturating_add(authority.watch_root.len())
                .saturating_add(authority.mapping_schema.len())
                .saturating_add(authority.config_seal.len()))?;
            if authority.backend == delivery.backend
                && authority.workspace_authority == delivery.workspace_authority
                && authority.configured_root == delivery.configured_root
                && authority.watch_root == delivery.watch_root
                && authority.mapping_schema == delivery.mapping_schema
                && authority.config_seal == delivery.config_seal {
                authorized = true;
            }
        }
        if !authorized {
            return Err(RetainedHostRefusalV1::InvalidDeliveryIdentity.into());
        }
        let mut recovered_in_delivery = false;
        let mut no_cursor_recovery_digest = None;
        for (index, ordinal) in delivery.ordinals.iter().enumerate() {
            charge_host_v1(budget, std::mem::size_of::<NativeCutoverEvidenceOrdinalV1>()
                .saturating_add(ordinal.recovery_request_digest.as_ref().map_or(0, String::len)))?;
            if ordinal.source_ordinal != u64::try_from(index)? {
                return Err(RetainedHostRefusalV1::InvalidOrdinal.into());
            }
            if delivery.cursor_receipt_digest.is_none() && !ordinal.filtered_no_semantic_work {
                let digest = ordinal.recovery_request_digest.as_deref()
                    .ok_or(RetainedHostRefusalV1::MissingRecoveryReceipt)?;
                if no_cursor_recovery_digest.is_some_and(|prior| prior != digest) {
                    return Err(RetainedHostRefusalV1::InvalidRecoveryReceipt.into());
                }
                no_cursor_recovery_digest = Some(digest);
            }
            if ordinal.recovery_request_digest.is_some()
                || ordinal.terminal.as_deref() == Some("RecoverySubsumed")
            {
                let event_id = match (ordinal.emitted_event_id, ordinal.reconciliation_event_id) {
                    (Some(event_id), None) if !ordinal.filtered_no_semantic_work => event_id,
                    (None, Some(event_id)) if !ordinal.filtered_no_semantic_work => event_id,
                    _ => return Err(RetainedHostRefusalV1::InvalidOrdinal.into()),
                };
                let request_digest = ordinal.recovery_request_digest.as_deref()
                    .ok_or(RetainedHostRefusalV1::MissingRecoveryReceipt)?;
                let receipt = recovery_receipt_for_digest_v1(host, request_digest, budget)?;
                if ordinal.terminal.is_none() {
                    recovery_for_event(event_id, receipt, budget)?;
                    return Err(RetainedHostRefusalV1::PendingRecoveryOrdinal.into());
                }
                // A later genuine full receipt may cover an already completed
                // ordinal. Preserve its original terminal and echo; labels
                // alone never provide this workspace-wide recovery proof.
                let echo = (
                    ordinal.source_transition_digest.as_deref(),
                    ordinal.target_source_digest.as_deref(),
                    ordinal.target_snapshot_id,
                );
                charge_host_v1(budget,
                    ordinal.source_transition_digest.as_ref().map_or(0, String::len)
                        .checked_add(ordinal.target_source_digest.as_ref().map_or(0, String::len))
                        .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?)?;
                match ordinal.terminal.as_deref() {
                    Some("RecoverySubsumed") if echo == (None, None, None) => {}
                    Some("Applied") if ordinal.admitted && ordinal.reconciliation_event_id.is_none()
                        && echo == (None, None, None) => {}
                    Some("AlreadyPublished") if ordinal.reconciliation_event_id.is_none() => {
                        match echo {
                            (None, None, None) => {}
                            (Some(transition), Some(target), Some(snapshot))
                                if canonical_host_digest_v1(transition)
                                    && canonical_host_digest_v1(target)
                                    && snapshot > 0 && snapshot <= receipt.published_snapshot_id => {}
                            _ => return Err(RetainedHostRefusalV1::ConflictingSourceEcho.into()),
                        }
                    }
                    _ => return Err(RetainedHostRefusalV1::InvalidOrdinal.into()),
                }
                recovery_for_event(event_id, receipt, budget)?;
                emitted_ids.push(event_id);
                recovered_in_delivery = true;
                continue;
            }
            if delivery.cursor_receipt_digest.is_none() && !ordinal.filtered_no_semantic_work {
                return Err(RetainedHostRefusalV1::IncompleteDelivery.into());
            }
            let source = match ordinal.emitted_event_id {
                Some(event_id) if !ordinal.filtered_no_semantic_work
                    && ordinal.reconciliation_event_id.is_none() =>
                    Some(source_for_event(event_id, budget)?),
                _ => None,
            };
            if let Some(event_id) = require_settled_host_ordinal_v1(ordinal, source, budget)? {
                emitted_ids.push(event_id);
            }
        }
        if delivery.cursor_receipt_digest.is_none() && !recovered_in_delivery {
            return Err(RetainedHostRefusalV1::IncompleteDelivery.into());
        }
    }
    for receipt in &host.recovery_receipts {
        let mut referenced = false;
        for delivery in &host.deliveries {
            for ordinal in &delivery.ordinals {
                charge_host_v1(budget, receipt.request_identity_digest.len())?;
                if ordinal.recovery_request_digest.as_deref()
                        == Some(receipt.request_identity_digest.as_str())
                {
                    referenced = true;
                }
            }
        }
        if !referenced {
            return Err(RetainedHostRefusalV1::MissingRecoveryReceipt.into());
        }
    }
    reject_duplicate_host_event_ids_v1(&mut emitted_ids, budget)?;
    drop(emitted_ids);
    drop(scratch_lease);
    budget.checkpoint()?;
    Ok(())
}

fn require_settled_retained_host_v1<'a>(
    evidence: &'a NativeCutoverEvidenceV1,
    joined: &NativeCutoverJoinedSourceFactsV1<'a>,
    budget: &mut WorkBudget,
) -> Result<()> {
    require_settled_retained_host_inventory_v1(&evidence.host_facts, budget,
        |event_id, budget| completed_source_for_event_v1(joined, event_id, budget),
        |event_id, receipt, budget| {
            require_recovery_receipt_v1(
                receipt,
                &receipt.request_identity_digest,
                event_id,
                &evidence.header.canonical_workspace_root,
                &evidence.header.host.canonical_workspace_root,
                joined.fresh_sdk,
                budget,
            )
        })
}

pub(super) fn join_source_and_host_v1<'a>(
    evidence: &'a NativeCutoverEvidenceV1,
    budget: &mut WorkBudget,
) -> Result<NativeCutoverJoinedSourceFactsV1<'a>> {
    budget.checkpoint()?;
    evidence.validate_structure_v1()?;
    let row_count = evidence.source_facts.len();
    let bytes = row_count
        .checked_mul(2)
        .and_then(|count| count.checked_mul(std::mem::size_of::<usize>()))
        .and_then(|count| u64::try_from(count).ok())
        .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
    let scratch_lease = budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)?
        .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
    let mut reservation_rows = Vec::new();
    reservation_rows.try_reserve_exact(row_count)?;
    let mut replay_rows = Vec::new();
    replay_rows.try_reserve_exact(row_count)?;
    let joined = join_native_cutover_source_facts_v1(
        evidence,
        &mut reservation_rows,
        &mut replay_rows,
        &mut |units| {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: units,
                ..WorkUsage::default()
            })
        },
    )
    .map_err(|error| match error {
        NativeCutoverSourceJoinErrorV1::Refused(reason) => {
            anyhow!("Source facts refused: {reason}")
        }
        NativeCutoverSourceJoinErrorV1::Work(error) => error.into(),
    })?;
    drop(replay_rows);
    drop(reservation_rows);
    drop(scratch_lease);
    ensure!(
        joined.native_update_request_v1().is_some(),
        "original FULL durable capture request is absent"
    );
    ensure!(
        joined.current_native_update_request_v1().is_some(),
        "current Native durable capture request is absent"
    );
    ensure!(
        joined.baseline.text_admission == evidence.header.selected_source.text_admission,
        "initial FULL text admission differs from selected Source"
    );
    require_settled_retained_host_v1(evidence, &joined, budget)?;
    // Empty Host inventory alone is never interpreted as a high-water mark.
    // Source's independent complete inventory and its original ACK joins above
    // remain necessary; the producer must finish every bounded cursor.
    budget.checkpoint()?;
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::super::{Error, UnixDatagram, send_outcome, wire};
    use super::*;
    use quanta_index_contract::index::Digest;
    use quanta_index_contract::native_cutover_evidence_v1::{
        NativeCutoverDigest64V1, NativeCutoverEvidenceDeliveryV1,
        NativeCutoverEvidencePreparedAuthorityV1,
    };
    use quanta_index_core::index::WorkLimits;
    use quanta_index_core::index::effort::{ReservationGuard, ResourceNeed};
    use crate::native_resources::{NativeResourceClass, NativeResourceLedger, NativeResourcePlan};
    use std::time::{Duration, Instant};

    fn budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 16,
                writes: 0,
                bytes: 4096,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(10),
        )
    }

    fn recovery_receipt_shape_fixture_v1() -> (
        NativeCutoverEvidenceHostRecoveryReceiptV1,
        NativeCutoverEvidenceFreshSdkV1,
    ) {
        // Fixed S request-identity and tail-custody goldens. This tests only
        // the Q scalar consumer, never a fabricated checkpoint readback.
        let request_digest = "86ae0c184e3cf354073df394a7b27939ee6ff2bccc26dfd1f3482d329118e70e";
        let receipt = NativeCutoverEvidenceHostRecoveryReceiptV1 {
            operation_id: "recovery-op".into(),
            canonical_workspace_root: "/workspace".into(),
            request_identity_digest: request_digest.into(),
            reason: "WatcherStartupRecovery".into(),
            snapshot_before: 40,
            required_snapshot_floor: 40,
            failure_revision: None,
            tail_event_ids: vec![19, 29],
            tail_custody_digest: "099ca89d689b16ffd177a87f24bddfcba84436c9123b076a9653aa3ab5492305".into(),
            observation_cut_digest: Some("a".repeat(64)),
            checkpoint_generation: 2,
            checkpoint_payload_digest: "b".repeat(64),
            published_snapshot_id: 41,
            applied_operation_id: "recovery-op".into(),
            applied_request_identity_digest: request_digest.into(),
            applied_source_owner_identity: "source-owner".into(),
            applied_source_execution_authority_digest: "source-authority".into(),
            applied_source_view_digest: "d".repeat(64),
            applied_analysis_owner_identity: "analysis-owner".into(),
            applied_analysis_lineage_digest: "analysis-lineage".into(),
            applied_index_owner_identity: "index-owner".into(),
            applied_index_publication_identity: "index-publication".into(),
            applied_index_publication_digest: "index-digest".into(),
            applied_published_snapshot_id: 41,
        };
        let fresh = NativeCutoverEvidenceFreshSdkV1 {
            target_snapshot_id: 40,
            committed_source_snapshot_id: 41,
            committed_source_snapshot_digest: NativeCutoverDigest64V1([b'd'; 64]),
            sdk_generation_digest: NativeCutoverDigest64V1([b'e'; 64]),
            source_pointer_digest: NativeCutoverDigest64V1([b'f'; 64]),
            source_manifest_digest: NativeCutoverDigest64V1([b'1'; 64]),
            physical_target_source_digest: NativeCutoverDigest64V1([b'2'; 64]),
        };
        (receipt, fresh)
    }

    #[test]
    fn recovery_receipt_consumer_rejects_foreign_request_tail_cut_checkpoint_and_source() {
        let (receipt, fresh) = recovery_receipt_shape_fixture_v1();
        require_recovery_receipt_v1(
            &receipt, &receipt.request_identity_digest, 19, "/workspace", "/workspace",
            &fresh, &mut budget(),
        ).expect("fixed receipt projection shape; real S readback is a separate prerequisite");
        let mut foreign = receipt.clone();
        foreign.applied_operation_id = "foreign-op".into();
        assert!(require_recovery_receipt_v1(&foreign, &receipt.request_identity_digest,
            19, "/workspace", "/workspace", &fresh, &mut budget()).is_err());
        let mut foreign = receipt.clone();
        foreign.tail_event_ids[0] = 17;
        assert!(require_recovery_receipt_v1(&foreign, &receipt.request_identity_digest,
            19, "/workspace", "/workspace", &fresh, &mut budget()).is_err());
        let mut foreign = receipt.clone();
        foreign.observation_cut_digest = Some("3".repeat(64));
        assert!(require_recovery_receipt_v1(&foreign, &receipt.request_identity_digest,
            19, "/workspace", "/workspace", &fresh, &mut budget()).is_err());
        let mut foreign = receipt.clone();
        foreign.checkpoint_generation = 0;
        assert!(require_recovery_receipt_v1(&foreign, &receipt.request_identity_digest,
            19, "/workspace", "/workspace", &fresh, &mut budget()).is_err());
        let mut foreign = receipt.clone();
        foreign.checkpoint_payload_digest = "not-a-digest".into();
        assert!(require_recovery_receipt_v1(&foreign, &receipt.request_identity_digest,
            19, "/workspace", "/workspace", &fresh, &mut budget()).is_err());
        let mut foreign = receipt.clone();
        foreign.applied_source_view_digest = "c".repeat(64);
        assert!(require_recovery_receipt_v1(&foreign, &receipt.request_identity_digest,
            19, "/workspace", "/workspace", &fresh, &mut budget()).is_err());
        let mut foreign_fresh = fresh.clone();
        foreign_fresh.committed_source_snapshot_id = 42;
        assert!(require_recovery_receipt_v1(&receipt, &receipt.request_identity_digest,
            19, "/workspace", "/workspace", &foreign_fresh, &mut budget()).is_err());
        assert!(require_recovery_receipt_v1(&receipt, &receipt.request_identity_digest,
            19, "/foreign", "/workspace", &fresh, &mut budget()).is_err());
        assert!(require_recovery_receipt_v1(&receipt, &receipt.request_identity_digest,
            19, "/workspace", "/foreign", &fresh, &mut budget()).is_err());
        assert!(require_recovery_receipt_v1(&receipt, &"f".repeat(64),
            19, "/workspace", "/workspace", &fresh, &mut budget()).is_err());
        assert!(require_recovery_receipt_v1(&receipt, &receipt.request_identity_digest,
            30, "/workspace", "/workspace", &fresh, &mut budget()).is_err());
    }

    #[test]
    fn ordinary_source_binding_requires_all_four_fresh_source_axes() {
        // Scalar parity only; no signed producer or publication is fabricated.
        let fresh = NativeCutoverEvidenceFreshSdkV1 {
            target_snapshot_id: 41,
            committed_source_snapshot_id: 41,
            committed_source_snapshot_digest: NativeCutoverDigest64V1([b'd'; 64]),
            sdk_generation_digest: NativeCutoverDigest64V1([b'a'; 64]),
            source_pointer_digest: NativeCutoverDigest64V1([b'b'; 64]),
            source_manifest_digest: NativeCutoverDigest64V1([b'c'; 64]),
            physical_target_source_digest: NativeCutoverDigest64V1([b'd'; 64]),
        };
        let binding = NativeCutoverEvidenceSourcePublicationBindingV1 {
            transition_digest: fresh.source_manifest_digest,
            completion_pointer_digest: fresh.source_pointer_digest,
            physical_target_source_digest: fresh.physical_target_source_digest,
            target_snapshot_id: fresh.target_snapshot_id,
        };
        let mut charged = budget();
        require_ordinary_source_binding_v1(&binding, &fresh, &mut charged)
            .expect("all four actual Source axes match");
        assert!(charged.usage().reads > 0 && charged.usage().bytes >= 200);
        let mut foreign = binding;
        foreign.transition_digest = NativeCutoverDigest64V1([b'e'; 64]);
        let mut wrong = budget();
        assert!(matches!(
            require_ordinary_source_binding_v1(&foreign, &fresh, &mut wrong)
                .unwrap_err()
                .downcast_ref::<FreshSourceParityRefusalV1>(),
            Some(FreshSourceParityRefusalV1::OrdinaryTargetMismatch)
        ));
        let mut foreign = binding;
        foreign.completion_pointer_digest = NativeCutoverDigest64V1([b'e'; 64]);
        assert!(require_ordinary_source_binding_v1(&foreign, &fresh, &mut budget()).is_err());
        let mut foreign = binding;
        foreign.physical_target_source_digest = NativeCutoverDigest64V1([b'e'; 64]);
        assert!(require_ordinary_source_binding_v1(&foreign, &fresh, &mut budget()).is_err());
        let mut foreign = binding;
        foreign.target_snapshot_id += 1;
        assert!(require_ordinary_source_binding_v1(&foreign, &fresh, &mut budget()).is_err());
    }

    #[test]
    fn retained_host_ordinal_shape_requires_completed_source_loan() {
        // These are shape checks only. The production caller obtains `source`
        // from the complete signed Source join and physical Host custody.
        let source = NativeCutoverEvidenceSourcePublicationBindingV1 {
            transition_digest: NativeCutoverDigest64V1([b'a'; 64]),
            completion_pointer_digest: NativeCutoverDigest64V1([b'b'; 64]),
            physical_target_source_digest: NativeCutoverDigest64V1([b'c'; 64]),
            target_snapshot_id: 41,
        };
        let emitted = NativeCutoverEvidenceOrdinalV1 {
            source_ordinal: 0,
            admitted: true,
            terminal: Some("Applied".into()),
            emitted_event_id: Some(19),
            filtered_no_semantic_work: false,
            reconciliation_event_id: None,
            source_transition_digest: None,
            target_source_digest: None,
            target_snapshot_id: None,
            recovery_request_digest: None,
        };
        assert_eq!(require_settled_host_ordinal_v1(&emitted, Some(&source), &mut budget())
            .expect("shape with matching Source"), Some(19));
        assert_eq!(require_settled_host_ordinal_v1(&emitted, None, &mut budget())
            .unwrap_err().downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::SourceOnlyOrUnmatched));

        let filtered = NativeCutoverEvidenceOrdinalV1 {
            source_ordinal: 1,
            admitted: false,
            terminal: Some("NoSemanticWork".into()),
            emitted_event_id: None,
            filtered_no_semantic_work: true,
            reconciliation_event_id: None,
            source_transition_digest: None,
            target_source_digest: None,
            target_snapshot_id: None,
            recovery_request_digest: None,
        };
        assert_eq!(require_settled_host_ordinal_v1(&filtered, None, &mut budget())
            .expect("terminal filtered ordinal"), None);
        let mut reconciliation = filtered.clone();
        reconciliation.filtered_no_semantic_work = false;
        reconciliation.reconciliation_event_id = Some(29);
        reconciliation.terminal = Some("RecoverySubsumed".into());
        assert_eq!(require_settled_host_ordinal_v1(&reconciliation, None, &mut budget())
            .unwrap_err().downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::ReconciliationRequired));
        let mut wrong_echo = emitted;
        wrong_echo.terminal = Some("AlreadyPublished".into());
        wrong_echo.source_transition_digest = Some("d".repeat(64));
        wrong_echo.target_source_digest = Some("c".repeat(64));
        wrong_echo.target_snapshot_id = Some(41);
        assert_eq!(require_settled_host_ordinal_v1(&wrong_echo, Some(&source), &mut budget())
            .unwrap_err().downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::ConflictingSourceEcho));
    }

    #[test]
    fn retained_host_duplicate_emitted_ids_and_work_refusal() {
        let mut ids = [19, 17, 19];
        assert_eq!(reject_duplicate_host_event_ids_v1(&mut ids, &mut budget())
            .unwrap_err().downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::DuplicateEmittedEvent));
        let mut unique = [23, 17, 19];
        reject_duplicate_host_event_ids_v1(&mut unique, &mut budget())
            .expect("distinct emitted event IDs");
        assert_eq!(unique, [17, 19, 23]);
    }

    fn retained_filtered_host_fixture_v1() -> NativeCutoverEvidenceHostFactsV1 {
        let authority = NativeCutoverEvidencePreparedAuthorityV1 {
            backend: "watchman".into(),
            workspace_authority: "watchman-workspace:owner".into(),
            configured_root: "/workspace".into(),
            watch_root: "/workspace".into(),
            mapping_schema: "watchman-map/v1".into(),
            config_seal: "a".repeat(64),
        };
        let delivery = NativeCutoverEvidenceDeliveryV1 {
            record_digest: "b".repeat(64),
            schema_version: "quanta-change-event-durable-delivery-inbox/v2".into(),
            sequence: 0,
            delivery_id: "c".repeat(64),
            backend: authority.backend.clone(),
            workspace_authority: authority.workspace_authority.clone(),
            configured_root: authority.configured_root.clone(),
            watch_root: authority.watch_root.clone(),
            stream_id: "watchman-stream".into(),
            prior_cursor: None,
            cursor: "cursor-1".into(),
            payload_digest: "d".repeat(64),
            mapping_schema: authority.mapping_schema.clone(),
            config_seal: authority.config_seal.clone(),
            input_count: 1,
            cursor_receipt_digest: Some("e".repeat(64)),
            ordinals: vec![NativeCutoverEvidenceOrdinalV1 {
                source_ordinal: 0,
                admitted: false,
                terminal: Some("NoSemanticWork".into()),
                emitted_event_id: None,
                filtered_no_semantic_work: true,
                reconciliation_event_id: None,
                source_transition_digest: None,
                target_source_digest: None,
                target_snapshot_id: None,
                recovery_request_digest: None,
            }],
        };
        NativeCutoverEvidenceHostFactsV1 {
            workspace_authority: authority.workspace_authority.clone(),
            capacity: 1,
            retained_delivery_count: 1,
            active_units: 1,
            prepared_authorities: vec![authority],
            deliveries: vec![delivery],
            pending_apply_intent: None,
            recovery_receipts: Vec::new(),
        }
    }

    fn with_host_inventory_budget_v1(
        host: &NativeCutoverEvidenceHostFactsV1,
        check: impl FnOnce(&mut WorkBudget) -> Result<()>,
    ) -> Result<()> {
        // Fixed scalar-shape tests retain their own scratch grant. They do
        // not issue a Source, Host, or checkpoint-owner publication proof.
        let ordinal_count = host.deliveries.iter().try_fold(0_usize, |sum, delivery| {
            sum.checked_add(delivery.ordinals.len())
                .ok_or(quanta_index_core::index::IndexError::ResourceLimited)
        })?;
        let memory_bytes = u64::try_from(ordinal_count.max(1)
            .checked_mul(std::mem::size_of::<u128>())
            .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?)?;
        let need = ResourceNeed {
            cpu_units: 1,
            io_bytes: 1,
            memory_bytes,
            provider_units: 0,
        };
        let ledger = NativeResourceLedger::new(NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 4,
                io_bytes: 4,
                memory_bytes: 4 * memory_bytes,
                provider_units: 0,
            },
            control: need,
            interactive: need,
            core: need,
            enrichment: need,
        })?;
        let guard = ReservationGuard::try_acquire(
            ledger.scheduler(NativeResourceClass::Core), need,
        ).ok_or_else(|| anyhow!("Core fixture reservation unavailable"))?;
        let scope = guard.memory_scope()
            .ok_or_else(|| anyhow!("Core fixture memory scope unavailable"))?;
        let mut charged = WorkBudget::new(
            WorkLimits { reads: 256, writes: 0, bytes: 32 * 1024, mutations: 0 },
            Instant::now() + Duration::from_secs(10),
        );
        charged.bind_memory_scope(Some(scope));
        let result = check(&mut charged);
        drop(charged);
        drop(guard);
        result
    }

    fn check_filtered_host_fixture_v1(host: &NativeCutoverEvidenceHostFactsV1) -> Result<()> {
        with_host_inventory_budget_v1(host, |charged| require_settled_retained_host_inventory_v1(host, charged, |_, _| {
            Err(anyhow!("filtered fixture must not request Source membership"))
        }, |_, _, _| Err(anyhow!("filtered fixture must not request recovery receipt"))))
    }

    fn retained_recovery_host_fixture_v1() -> (
        NativeCutoverEvidenceHostFactsV1,
        NativeCutoverEvidenceFreshSdkV1,
    ) {
        let (receipt, fresh) = recovery_receipt_shape_fixture_v1();
        let mut host = retained_filtered_host_fixture_v1();
        let ordinal = &mut host.deliveries[0].ordinals[0];
        ordinal.filtered_no_semantic_work = false;
        ordinal.emitted_event_id = Some(19);
        ordinal.terminal = Some("RecoverySubsumed".into());
        ordinal.recovery_request_digest = Some(receipt.request_identity_digest.clone());
        host.deliveries[0].cursor_receipt_digest = None;
        host.recovery_receipts.push(receipt);
        (host, fresh)
    }

    fn check_recovery_host_fixture_v1(
        host: &NativeCutoverEvidenceHostFactsV1,
        fresh: &NativeCutoverEvidenceFreshSdkV1,
    ) -> Result<()> {
        with_host_inventory_budget_v1(host, |charged| {
            require_settled_retained_host_inventory_v1(host, charged,
                |_, _| Err(anyhow!("full-only recovery must not require a Watch request")),
                |event_id, receipt, charged| require_recovery_receipt_v1(
                    receipt, &receipt.request_identity_digest, event_id,
                    "/workspace", "/workspace", fresh, charged,
                ),
            )
        })
    }

    #[test]
    fn full_only_retained_recovery_accepts_exact_receipt_without_watch_request() {
        // Fixed Q scalar shape. The shipping S boundary supplies the genuine
        // current checkpoint-owner readback and retained Host descriptor.
        let (host, fresh) = retained_recovery_host_fixture_v1();
        check_recovery_host_fixture_v1(&host, &fresh)
            .expect("full receipt covers a no-cursor event without Watch Source request");
    }

    #[test]
    fn no_cursor_mixed_terminals_require_one_exact_full_receipt() {
        let (mut host, fresh) = retained_recovery_host_fixture_v1();
        let mut already = host.deliveries[0].ordinals[0].clone();
        already.source_ordinal = 1;
        already.emitted_event_id = Some(29);
        already.terminal = Some("AlreadyPublished".into());
        already.source_transition_digest = Some("a".repeat(64));
        already.target_source_digest = Some("b".repeat(64));
        already.target_snapshot_id = Some(40);
        host.deliveries[0].ordinals.push(already);
        host.deliveries[0].input_count = 2;
        host.active_units = 2;
        host.capacity = 2;
        check_recovery_host_fixture_v1(&host, &fresh)
            .expect("original AlreadyPublished label and complete echo survive full coverage");

        let mut missing_reference = host.clone();
        missing_reference.deliveries[0].ordinals[1].recovery_request_digest = None;
        assert_eq!(check_recovery_host_fixture_v1(&missing_reference, &fresh)
            .unwrap_err().downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::MissingRecoveryReceipt));
        let mut conflicting_reference = host.clone();
        conflicting_reference.deliveries[0].ordinals[1].recovery_request_digest =
            Some("f".repeat(64));
        assert_eq!(check_recovery_host_fixture_v1(&conflicting_reference, &fresh)
            .unwrap_err().downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::InvalidRecoveryReceipt));
        let mut partial_echo = host.clone();
        partial_echo.deliveries[0].ordinals[1].target_source_digest = None;
        assert_eq!(check_recovery_host_fixture_v1(&partial_echo, &fresh)
            .unwrap_err().downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::ConflictingSourceEcho));
        let mut stale_fresh = fresh.clone();
        stale_fresh.committed_source_snapshot_id += 1;
        assert_eq!(check_recovery_host_fixture_v1(&host, &stale_fresh)
            .unwrap_err().downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::InvalidRecoveryReceipt));
    }

    #[test]
    fn full_receipt_does_not_authorize_pending_or_reconciliation_applied() {
        let (host, fresh) = retained_recovery_host_fixture_v1();
        let mut missing = host.clone();
        missing.recovery_receipts.clear();
        assert_eq!(check_recovery_host_fixture_v1(&missing, &fresh)
            .unwrap_err().downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::MissingRecoveryReceipt));
        let mut pending = host.clone();
        pending.deliveries[0].ordinals[0].terminal = None;
        let cause = check_recovery_host_fixture_v1(&pending, &fresh).unwrap_err();
        assert_eq!(
            cause.downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::PendingRecoveryOrdinal)
        );
        let begin = wire::NativeCutoverControlBeginV1 {
            format_version: wire::NativeCutoverControlBeginV1::FORMAT_VERSION_V1,
            operation_id: "retained-host-ordinal-refused".into(),
            mode: wire::NativeCutoverRecoveryModeV1::RestoreForward,
            current_selection_sha256: Digest([1; 32]),
            candidate_selection_sha256: Digest([2; 32]),
            deadline_ns: 1,
            evidence_length: 1,
            evidence_sha256: Digest([3; 32]),
            nonce: Digest([4; 32]),
        };
        let (resident, helper) = UnixDatagram::pair().expect("control socket pair");
        resident
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("bound terminal receive time");
        let exit = send_outcome(&helper, &begin, Err(Error::RefusedBeforeRename(cause)))
            .expect("send bound refusal");
        assert_eq!(exit, 2);
        let mut frame = [0; wire::MAX_NATIVE_CUTOVER_CONTROL_BYTES_V1];
        let size = resident.recv(&mut frame).expect("receive refusal frame");
        let terminal = wire::decode_native_cutover_control_terminal_v1(&frame[..size])
            .expect("decode canonical refusal frame");
        assert!(matches!(
            terminal,
            wire::NativeCutoverControlTerminalV1::RefusedBeforeRename {
                operation_id, nonce, code, message,
            } if operation_id == begin.operation_id
                && nonce == begin.nonce
                && code == "PENDING_RETAINED_RECOVERY_ORDINAL"
                && message == "Host recovery receipt covers an ordinal without a terminal decision"
        ));
        let mut pending_missing = pending.clone();
        pending_missing.recovery_receipts.clear();
        assert_eq!(check_recovery_host_fixture_v1(&pending_missing, &fresh)
            .unwrap_err().downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::MissingRecoveryReceipt));
        let mut stale_fresh = fresh.clone();
        stale_fresh.committed_source_snapshot_id += 1;
        assert_eq!(check_recovery_host_fixture_v1(&pending, &stale_fresh)
            .unwrap_err().downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::InvalidRecoveryReceipt));
        let mut reconciliation_applied = host;
        let ordinal = &mut reconciliation_applied.deliveries[0].ordinals[0];
        ordinal.emitted_event_id = None;
        ordinal.reconciliation_event_id = Some(19);
        ordinal.terminal = Some("Applied".into());
        assert_eq!(check_recovery_host_fixture_v1(&reconciliation_applied, &fresh)
            .unwrap_err().downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::InvalidOrdinal));
    }

    #[test]
    fn retained_filtered_host_inventory_checks_occupancy_authority_and_cursor() {
        let valid = retained_filtered_host_fixture_v1();
        let no_scope = require_settled_retained_host_inventory_v1(&valid, &mut budget(), |_, _| {
            Err(anyhow!("filtered fixture must not request Source membership"))
        }, |_, _, _| Err(anyhow!("filtered fixture must not request recovery receipt"))).unwrap_err();
        assert!(matches!(no_scope.downcast_ref::<quanta_index_core::index::IndexError>(),
            Some(quanta_index_core::index::IndexError::ResourceLimited)));
        check_filtered_host_fixture_v1(&valid).expect("retained filtered Host shape");

        let mut wrong_occupancy = valid.clone();
        wrong_occupancy.active_units = 0;
        assert_eq!(check_filtered_host_fixture_v1(&wrong_occupancy).unwrap_err()
            .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::PendingApplyOrOccupancy));
        let mut wrong_capacity = valid.clone();
        wrong_capacity.capacity = 0;
        assert_eq!(check_filtered_host_fixture_v1(&wrong_capacity).unwrap_err()
            .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::PendingApplyOrOccupancy));
        let mut wrong_count = valid.clone();
        wrong_count.deliveries[0].input_count = 2;
        assert_eq!(check_filtered_host_fixture_v1(&wrong_count).unwrap_err()
            .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::IncompleteDelivery));
        let mut wrong_ordinal = valid.clone();
        wrong_ordinal.deliveries[0].ordinals[0].source_ordinal = 1;
        assert_eq!(check_filtered_host_fixture_v1(&wrong_ordinal).unwrap_err()
            .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::InvalidOrdinal));
        let mut missing_receipt = valid.clone();
        missing_receipt.deliveries[0].cursor_receipt_digest = None;
        assert_eq!(check_filtered_host_fixture_v1(&missing_receipt).unwrap_err()
            .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::IncompleteDelivery));
        let mut malformed_receipt = valid.clone();
        malformed_receipt.deliveries[0].cursor_receipt_digest = Some(String::new());
        assert_eq!(check_filtered_host_fixture_v1(&malformed_receipt).unwrap_err()
            .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::InvalidDeliveryIdentity));
        let mut foreign_authority = valid;
        foreign_authority.prepared_authorities[0].config_seal = "f".repeat(64);
        assert_eq!(check_filtered_host_fixture_v1(&foreign_authority).unwrap_err()
            .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::InvalidDeliveryIdentity));
    }
}
