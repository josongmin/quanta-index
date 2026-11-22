//! Borrowed Original Source associations and Host facts remain data until the
//! receiver joins them with its live Q authority and retained descriptors.

use std::cmp::Ordering;

use anyhow::{Result, anyhow, ensure};
use quanta_index_contract::{
    native_cutover_evidence_v1::{
        NativeCutoverEvidenceFreshSdkV1, NativeCutoverEvidenceHostFactsV1,
        NativeCutoverEvidenceHostRecoveryReceiptV1,
        NativeCutoverEvidenceOrdinalV1, NativeCutoverEvidenceSdkBindingV1,
        NativeCutoverEvidenceSourcePublicationBindingV1,
        NativeCutoverEvidenceSourceFactV1, NativeCutoverEvidenceV1,
    },
    native_cutover_source_join_v1::{
        NativeCutoverJoinedSourceFactsV1, NativeCutoverSourceJoinErrorV1,
        NativeCutoverSourceEventMatchV1, NativeCutoverSourceEventMatchesV1,
        join_native_cutover_source_facts_v1,
    },
};
use quanta_index_core::index::{MemoryLease, MemoryLifetime, WorkBudget, WorkUsage};

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

fn require_sdk_source_binding_v1(
    binding: &NativeCutoverEvidenceSdkBindingV1,
    fresh: &NativeCutoverEvidenceFreshSdkV1,
    budget: &mut WorkBudget,
) -> Result<()> {
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
        || binding.target_source_digest.as_bytes() != fresh.physical_target_source_digest.0
        || binding.transition_digest.as_bytes() != fresh.source_manifest_digest.0
        || binding.completion_pointer_digest.as_bytes() != fresh.source_pointer_digest.0
    {
        return Err(FreshSourceParityRefusalV1::SdkTargetMismatch.into());
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
    if let Some(NativeCutoverEvidenceSourceFactV1::PendingStoreSdkAck { binding, .. }) = sdk_fact {
        require_sdk_source_binding_v1(binding, fresh, budget)?;
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

struct RetainedHostRecoveryReceiptV1<'a> {
    receipt: &'a NativeCutoverEvidenceHostRecoveryReceiptV1,
    verified: bool,
}

impl RetainedHostRecoveryReceiptV1<'_> {
    fn require_event(
        &mut self,
        event_id: u128,
        verify: &mut impl FnMut(
            &NativeCutoverEvidenceHostRecoveryReceiptV1,
            &mut WorkBudget,
        ) -> Result<()>,
        budget: &mut WorkBudget,
    ) -> Result<()> {
        if !self.verified {
            verify(self.receipt, budget)?;
            self.verified = true;
        }
        if find_host_item_v1(&self.receipt.tail_event_ids, |actual| {
            charge_host_v1(budget, std::mem::size_of::<u128>())?;
            Ok(actual.cmp(&event_id))
        })?
        .is_none()
        {
            return Err(RetainedHostRefusalV1::InvalidRecoveryReceipt.into());
        }
        Ok(())
    }
}

fn compare_host_digests_v1(left: &str, right: &str, budget: &mut WorkBudget) -> Result<Ordering> {
    charge_host_v1(budget, left.len().saturating_add(right.len()))?;
    Ok(left.cmp(right))
}

fn find_host_item_v1<T>(
    items: &[T],
    mut compare: impl FnMut(&T) -> Result<Ordering>,
) -> Result<Option<usize>> {
    let mut lower = 0;
    let mut upper = items.len();
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        if compare(&items[middle])? == Ordering::Less {
            lower = middle + 1;
        } else {
            upper = middle;
        }
    }
    if let Some(item) = items.get(lower)
        && compare(item)? == Ordering::Equal
    {
        return Ok(Some(lower));
    }
    Ok(None)
}

fn recovery_receipt_for_digest_v1<'rows, 'evidence>(
    rows: &'rows mut [RetainedHostRecoveryReceiptV1<'evidence>],
    request_digest: &str,
    budget: &mut WorkBudget,
) -> Result<&'rows mut RetainedHostRecoveryReceiptV1<'evidence>> {
    let index = find_host_item_v1(rows, |row| {
        compare_host_digests_v1(&row.receipt.request_identity_digest, request_digest, budget)
    })?
    .ok_or(RetainedHostRefusalV1::MissingRecoveryReceipt)?;
    // Reject a duplicate when its digest is referenced. Unreferenced rows are
    // still rejected by the same orphan check as a lone unreferenced receipt.
    if !rows[index].verified
        && let Some(next) = rows.get(index + 1)
        && compare_host_digests_v1(
            &next.receipt.request_identity_digest,
            request_digest,
            budget,
        )? == Ordering::Equal
    {
        return Err(RetainedHostRefusalV1::DuplicateRecoveryReceipt.into());
    }
    Ok(&mut rows[index])
}

/// This is a comparison of the checkpoint owner's re-read receipt DATA. The
/// S loan must have produced the DTO from its genuine typed receipt while the
/// retained inbox and Source census were held; these scalar checks cannot
/// mint a recovery or Source publication authority.
/// A full recovery covers its exact tail through the current workspace Source
/// proof. Its events need not have an earlier completed Watch request.
fn require_recovery_receipt_v1(
    receipt: &NativeCutoverEvidenceHostRecoveryReceiptV1,
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
    matched_events: &NativeCutoverSourceEventMatchesV1<'a, '_>,
    event_id: u128,
    budget: &mut WorkBudget,
) -> Result<&'a NativeCutoverEvidenceSourcePublicationBindingV1> {
    let binding = matched_events.source_for_event_v1(event_id, &mut |units| {
        budget.charge(WorkUsage { reads: 1, bytes: units, ..WorkUsage::default() })
    })?
        .ok_or(RetainedHostRefusalV1::SourceOnlyOrUnmatched)?;
    // The S Original lender already checked every completed step against the
    // current manifest/pointer ancestry under one retained Source/Output loan.
    // An older Source-only step need not equal the fresh pointer's four axes.
    Ok(binding)
}

fn host_ordinal_needs_recovery_v1(ordinal: &NativeCutoverEvidenceOrdinalV1) -> bool {
    ordinal.recovery_request_digest.is_some()
        || ordinal.terminal.as_deref() == Some("RecoverySubsumed")
}

fn ordinary_host_event_id_v1(ordinal: &NativeCutoverEvidenceOrdinalV1) -> Option<u128> {
    if ordinal.filtered_no_semantic_work || ordinal.reconciliation_event_id.is_some()
        || host_ordinal_needs_recovery_v1(ordinal) {
        return None;
    }
    ordinal.emitted_event_id
}

struct RetainedHostSourceMatchesV1<'a> {
    // Field order releases the backing allocation before its grant on every exit.
    slots: Vec<NativeCutoverSourceEventMatchV1<'a>>,
    _lease: Option<MemoryLease>,
}

fn prepare_host_event_matches_v1<'a>(
    host: &'a NativeCutoverEvidenceHostFactsV1,
    budget: &mut WorkBudget,
) -> Result<RetainedHostSourceMatchesV1<'a>> {
    let ordinals = host.deliveries.iter().flat_map(|delivery| &delivery.ordinals);
    let mut count = 0_usize;
    for ordinal in ordinals.clone() {
        charge_host_v1(budget, std::mem::size_of_val(ordinal))?;
        if ordinary_host_event_id_v1(ordinal).is_some() {
            count = count.checked_add(1)
                .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
        }
    }
    if count == 0 {
        return Ok(RetainedHostSourceMatchesV1 { slots: Vec::new(), _lease: None });
    }
    let bytes = count.checked_mul(std::mem::size_of::<NativeCutoverSourceEventMatchV1<'_>>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
    let lease = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?
        .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
    let mut slots = Vec::new();
    slots.try_reserve_exact(count)?;
    ensure!(slots.capacity() <= count, "Host event match scratch exceeded its memory grant");
    for ordinal in ordinals {
        charge_host_v1(budget, std::mem::size_of_val(ordinal))?;
        if let Some(event_id) = ordinary_host_event_id_v1(ordinal) {
            slots.push(NativeCutoverSourceEventMatchV1::new(event_id));
        }
    }
    sort_host_items_v1(&mut slots, |left, right| {
        charge_host_v1(budget, 2 * std::mem::size_of::<u128>())?;
        Ok(left.event_id_v1().cmp(&right.event_id_v1()))
    })?;
    Ok(RetainedHostSourceMatchesV1 { slots, _lease: Some(lease) })
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

fn sift_host_heap_v1<T>(
    items: &mut [T],
    mut root: usize,
    end: usize,
    compare: &mut impl FnMut(&T, &T) -> Result<Ordering>,
) -> Result<()> {
    loop {
        let left = root.checked_mul(2).and_then(|value| value.checked_add(1))
            .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
        if left >= end { return Ok(()) }
        let right = left + 1;
        let mut largest = root;
        if compare(&items[largest], &items[left])? == Ordering::Less {
            largest = left;
        }
        if right < end && compare(&items[largest], &items[right])? == Ordering::Less {
            largest = right;
        }
        if largest == root { return Ok(()) }
        items.swap(root, largest);
        root = largest;
    }
}

fn sort_host_items_v1<T>(
    items: &mut [T],
    mut compare: impl FnMut(&T, &T) -> Result<Ordering>,
) -> Result<()> {
    let length = items.len();
    for root in (0..length / 2).rev() {
        sift_host_heap_v1(items, root, length, &mut compare)?;
    }
    for end in (1..length).rev() {
        items.swap(0, end);
        sift_host_heap_v1(items, 0, end, &mut compare)?;
    }
    Ok(())
}

fn reject_duplicate_host_event_ids_v1(ids: &mut [u128], budget: &mut WorkBudget) -> Result<()> {
    sort_host_items_v1(ids, |left, right| {
        charge_host_v1(budget, 2 * std::mem::size_of::<u128>())?;
        Ok(left.cmp(right))
    })?;
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
    mut verify_recovery_receipt: impl FnMut(
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
    let receipt_count = host.recovery_receipts.len();
    let receipt_row_bytes = std::mem::size_of::<RetainedHostRecoveryReceiptV1<'_>>();
    let reserved_bytes = receipt_count
        .checked_mul(receipt_row_bytes)
        .and_then(|receipt_bytes| {
            ordinal_count
                .checked_mul(std::mem::size_of::<u128>())
                .and_then(|ordinal_bytes| ordinal_bytes.checked_add(receipt_bytes))
        })
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
    let scratch_lease = if reserved_bytes == 0 { None } else {
        Some(budget.reserve_memory(reserved_bytes, MemoryLifetime::Temporary)?
            .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?)
    };
    let mut emitted_ids = Vec::<u128>::new();
    emitted_ids.try_reserve_exact(ordinal_count)?;
    let mut recovery_receipts = Vec::new();
    recovery_receipts.try_reserve_exact(receipt_count)?;
    ensure!(
        emitted_ids.capacity() <= ordinal_count && recovery_receipts.capacity() <= receipt_count,
        "Host inventory scratch exceeded its memory grant"
    );
    for receipt in &host.recovery_receipts {
        charge_host_v1(budget, receipt_row_bytes)?;
        recovery_receipts.push(RetainedHostRecoveryReceiptV1 {
            receipt,
            verified: false,
        });
    }
    sort_host_items_v1(&mut recovery_receipts, |left, right| {
        compare_host_digests_v1(
            &left.receipt.request_identity_digest,
            &right.receipt.request_identity_digest,
            budget,
        )
    })?;
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
            if host_ordinal_needs_recovery_v1(ordinal) {
                let event_id = match (ordinal.emitted_event_id, ordinal.reconciliation_event_id) {
                    (Some(event_id), None) if !ordinal.filtered_no_semantic_work => event_id,
                    (None, Some(event_id)) if !ordinal.filtered_no_semantic_work => event_id,
                    _ => return Err(RetainedHostRefusalV1::InvalidOrdinal.into()),
                };
                let request_digest = ordinal.recovery_request_digest.as_deref()
                    .ok_or(RetainedHostRefusalV1::MissingRecoveryReceipt)?;
                let row =
                    recovery_receipt_for_digest_v1(&mut recovery_receipts, request_digest, budget)?;
                if ordinal.terminal.is_none() {
                    row.require_event(event_id, &mut verify_recovery_receipt, budget)?;
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
                                    && snapshot > 0
                                    && snapshot <= row.receipt.published_snapshot_id => {}
                            _ => return Err(RetainedHostRefusalV1::ConflictingSourceEcho.into()),
                        }
                    }
                    _ => return Err(RetainedHostRefusalV1::InvalidOrdinal.into()),
                }
                row.require_event(event_id, &mut verify_recovery_receipt, budget)?;
                emitted_ids.push(event_id);
                recovered_in_delivery = true;
                continue;
            }
            if delivery.cursor_receipt_digest.is_none() && !ordinal.filtered_no_semantic_work {
                return Err(RetainedHostRefusalV1::IncompleteDelivery.into());
            }
            let source = ordinary_host_event_id_v1(ordinal)
                .map(|event_id| source_for_event(event_id, budget)).transpose()?;
            if let Some(event_id) = require_settled_host_ordinal_v1(ordinal, source, budget)? {
                emitted_ids.push(event_id);
            }
        }
        if delivery.cursor_receipt_digest.is_none() && !recovered_in_delivery {
            return Err(RetainedHostRefusalV1::IncompleteDelivery.into());
        }
    }
    for row in &recovery_receipts {
        charge_host_v1(budget, std::mem::size_of::<bool>())?;
        if !row.verified {
            return Err(RetainedHostRefusalV1::MissingRecoveryReceipt.into());
        }
    }
    reject_duplicate_host_event_ids_v1(&mut emitted_ids, budget)?;
    drop(recovery_receipts);
    drop(emitted_ids);
    drop(scratch_lease);
    budget.checkpoint()?;
    Ok(())
}

fn require_settled_retained_host_v1<'a>(
    evidence: &'a NativeCutoverEvidenceV1,
    joined: &NativeCutoverJoinedSourceFactsV1<'a>,
    matched_events: &NativeCutoverSourceEventMatchesV1<'a, '_>,
    budget: &mut WorkBudget,
) -> Result<()> {
    require_settled_retained_host_inventory_v1(&evidence.host_facts, budget,
        |event_id, budget| completed_source_for_event_v1(matched_events, event_id, budget),
        |receipt, budget| {
            require_recovery_receipt_v1(
                receipt,
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
    let mut event_matches = prepare_host_event_matches_v1(&evidence.host_facts, budget)?;
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
    let (joined, matched_events) = join_native_cutover_source_facts_v1(
        evidence,
        &mut reservation_rows,
        &mut replay_rows,
        &mut event_matches.slots,
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
    require_settled_retained_host_v1(evidence, &joined, &matched_events, budget)?;
    drop(event_matches);
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
        let verify = |receipt: &NativeCutoverEvidenceHostRecoveryReceiptV1| {
            require_recovery_receipt_v1(receipt, "/workspace", "/workspace", &fresh, &mut budget())
        };
        verify(&receipt)
            .expect("fixed receipt projection shape; real S readback is a separate prerequisite");
        let mut foreign = receipt.clone();
        foreign.applied_operation_id = "foreign-op".into();
        assert!(verify(&foreign).is_err());
        let mut foreign = receipt.clone();
        foreign.tail_event_ids[0] = 17;
        assert!(verify(&foreign).is_err());
        let mut foreign = receipt.clone();
        foreign.observation_cut_digest = Some("3".repeat(64));
        assert!(verify(&foreign).is_err());
        let mut foreign = receipt.clone();
        foreign.checkpoint_generation = 0;
        assert!(verify(&foreign).is_err());
        let mut foreign = receipt.clone();
        foreign.checkpoint_payload_digest = "not-a-digest".into();
        assert!(verify(&foreign).is_err());
        let mut foreign = receipt.clone();
        foreign.applied_source_view_digest = "c".repeat(64);
        assert!(verify(&foreign).is_err());
        let mut foreign_fresh = fresh.clone();
        foreign_fresh.committed_source_snapshot_id = 42;
        assert!(require_recovery_receipt_v1(
            &receipt, "/workspace", "/workspace", &foreign_fresh, &mut budget(),
        ).is_err());
        assert!(require_recovery_receipt_v1(
            &receipt, "/foreign", "/workspace", &fresh, &mut budget(),
        ).is_err());
        assert!(require_recovery_receipt_v1(
            &receipt, "/workspace", "/foreign", &fresh, &mut budget(),
        ).is_err());
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
    fn change_event_sdk_ack_binds_source_target_even_when_committed_head_has_advanced() -> Result<()> {
        let (_, fresh) = recovery_receipt_shape_fixture_v1();
        let binding = NativeCutoverEvidenceSdkBindingV1 {
            source_intent_digest: "3".repeat(64),
            completion_pointer_digest: "f".repeat(64),
            transition_digest: "1".repeat(64),
            target_source_digest: "2".repeat(64),
            target_snapshot: 40,
            ingress_kind: "Watch".into(),
            workspace_identity_digest: "4".repeat(64),
            request_fingerprint_digest: "5".repeat(64),
            client_idempotency_digest: None,
        };
        let mut charged = budget();
        require_sdk_source_binding_v1(&binding, &fresh, &mut charged)?;
        assert_eq!(charged.usage().bytes, 3 * 64);
        for foreign in [
            NativeCutoverEvidenceSdkBindingV1 {
                target_snapshot: fresh.committed_source_snapshot_id,
                ..binding.clone()
            },
            NativeCutoverEvidenceSdkBindingV1 {
                target_source_digest: "d".repeat(64),
                ..binding.clone()
            },
            NativeCutoverEvidenceSdkBindingV1 {
                transition_digest: "3".repeat(64),
                ..binding.clone()
            },
            NativeCutoverEvidenceSdkBindingV1 {
                completion_pointer_digest: "4".repeat(64),
                ..binding
            },
        ] {
            assert!(matches!(
                require_sdk_source_binding_v1(&foreign, &fresh, &mut budget())
                    .unwrap_err()
                    .downcast_ref::<FreshSourceParityRefusalV1>(),
                Some(FreshSourceParityRefusalV1::SdkTargetMismatch)
            ));
        }
        Ok(())
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
        with_host_inventory_limits_v1(
            host,
            WorkLimits {
                reads: 256,
                writes: 0,
                bytes: 32 * 1024,
                mutations: 0,
            },
            check,
        )
    }

    fn with_host_inventory_limits_v1(
        host: &NativeCutoverEvidenceHostFactsV1,
        limits: WorkLimits,
        check: impl FnOnce(&mut WorkBudget) -> Result<()>,
    ) -> Result<()> {
        // Fixed scalar-shape tests retain their own scratch grant. They do
        // not issue a Source, Host, or checkpoint-owner publication proof.
        let ordinal_count = host.deliveries.iter().try_fold(0_usize, |sum, delivery| {
            sum.checked_add(delivery.ordinals.len())
                .ok_or(quanta_index_core::index::IndexError::ResourceLimited)
        })?;
        let scratch_bytes = ordinal_count
            .checked_mul(std::mem::size_of::<u128>())
            .and_then(|bytes| {
                host.recovery_receipts
                    .len()
                    .checked_mul(std::mem::size_of::<RetainedHostRecoveryReceiptV1<'_>>())
                    .and_then(|receipt_bytes| bytes.checked_add(receipt_bytes))
            })
            .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
        let memory_bytes = u64::try_from(scratch_bytes.max(1))?;
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
        let mut charged = WorkBudget::new(limits, Instant::now() + Duration::from_secs(10));
        charged.bind_memory_scope(Some(scope));
        let result = check(&mut charged);
        let refunded = charged
            .reserve_memory(memory_bytes, MemoryLifetime::Temporary)?
            .ok_or_else(|| anyhow!("Host inventory scratch grant leaked"))?;
        drop(refunded);
        drop(charged);
        drop(guard);
        result
    }

    fn check_filtered_host_fixture_v1(host: &NativeCutoverEvidenceHostFactsV1) -> Result<()> {
        with_host_inventory_budget_v1(host, |charged| {
            require_settled_retained_host_inventory_v1(
                host,
                charged,
                |_, _| {
                    Err(anyhow!(
                        "filtered fixture must not request Source membership"
                    ))
                },
                |_, _| {
                    Err(anyhow!(
                        "filtered fixture must not request recovery receipt"
                    ))
                },
            )
        })
    }

    #[test]
    fn retained_host_source_slots_include_only_ordinary_emissions() {
        let mut host = retained_filtered_host_fixture_v1();
        let delivery = &mut host.deliveries[0];
        let ordinary = NativeCutoverEvidenceOrdinalV1 {
            admitted: true,
            terminal: Some("Applied".into()),
            emitted_event_id: Some(42),
            filtered_no_semantic_work: false,
            ..delivery.ordinals[0].clone()
        };
        delivery.ordinals.extend([
            ordinary.clone(),
            NativeCutoverEvidenceOrdinalV1 {
                emitted_event_id: Some(7),
                ..ordinary.clone()
            },
            NativeCutoverEvidenceOrdinalV1 {
                terminal: Some("RecoverySubsumed".into()),
                ..ordinary.clone()
            },
            NativeCutoverEvidenceOrdinalV1 {
                recovery_request_digest: Some("a".repeat(64)),
                ..ordinary.clone()
            },
            NativeCutoverEvidenceOrdinalV1 {
                reconciliation_event_id: Some(99),
                ..ordinary
            },
        ]);
        with_host_inventory_budget_v1(&host, |budget| {
            let matches = prepare_host_event_matches_v1(&host, budget)?;
            let ids: Vec<_> = matches.slots.iter()
                .map(NativeCutoverSourceEventMatchV1::event_id_v1).collect();
            assert_eq!(ids, [7, 42]);
            assert!(matches._lease.is_some());
            Ok(())
        }).expect("ordinary probes fit the granted scratch and refund it");
        with_host_inventory_limits_v1(&host, WorkLimits {
            reads: host.deliveries[0].ordinals.len() as u64 + 1,
            writes: 0,
            bytes: 32 * 1024,
            mutations: 0,
        }, |budget| {
            let result = prepare_host_event_matches_v1(&host, budget);
            assert!(matches!(result, Err(ref error)
                if error.downcast_ref::<quanta_index_core::index::IndexError>()
                    == Some(&quanta_index_core::index::IndexError::ResourceLimited)));
            Ok(())
        }).expect("failure during slot collection refunds the grant");
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
            require_settled_retained_host_inventory_v1(
                host,
                charged,
                |_, _| {
                    Err(anyhow!(
                        "full-only recovery must not require a Watch request"
                    ))
                },
                |receipt, charged| {
                    require_recovery_receipt_v1(receipt, "/workspace", "/workspace", fresh, charged)
                },
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
    fn shared_retained_host_receipt_is_verified_once_with_exact_tail_membership() {
        let (mut host, fresh) = retained_recovery_host_fixture_v1();
        let mut second = host.deliveries[0].ordinals[0].clone();
        second.source_ordinal = 1;
        second.emitted_event_id = Some(29);
        host.deliveries[0].ordinals.push(second);
        host.deliveries[0].input_count = 2;
        host.active_units = 2;
        host.capacity = 2;

        let mut validations = 0;
        with_host_inventory_budget_v1(&host, |charged| {
            require_settled_retained_host_inventory_v1(
                &host,
                charged,
                |_, _| {
                    Err(anyhow!(
                        "full-only recovery must not require a Watch request"
                    ))
                },
                |receipt, charged| {
                    validations += 1;
                    require_recovery_receipt_v1(
                        receipt,
                        "/workspace",
                        "/workspace",
                        &fresh,
                        charged,
                    )
                },
            )
        })
        .expect("both ordinals are covered by the same full receipt");
        assert_eq!(validations, 1, "verify the full receipt once per inventory");

        // A prior ordinal having validated the receipt cannot admit a new event.
        host.deliveries[0].ordinals[1].emitted_event_id = Some(30);
        assert_eq!(
            check_recovery_host_fixture_v1(&host, &fresh)
                .unwrap_err()
                .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::InvalidRecoveryReceipt)
        );

        host.deliveries[0].ordinals[1].terminal = None;
        assert_eq!(
            check_recovery_host_fixture_v1(&host, &fresh)
                .unwrap_err()
                .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::InvalidRecoveryReceipt)
        );
        host.deliveries[0].ordinals[1].emitted_event_id = Some(29);
        assert_eq!(
            check_recovery_host_fixture_v1(&host, &fresh)
                .unwrap_err()
                .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::PendingRecoveryOrdinal)
        );
    }

    #[test]
    fn retained_host_receipt_index_rejects_duplicates_orphans_and_missing_keys() {
        let (mut host, fresh) = retained_recovery_host_fixture_v1();
        let receipt = host.recovery_receipts[0].clone();
        host.recovery_receipts.push(receipt.clone());
        assert_eq!(
            check_recovery_host_fixture_v1(&host, &fresh)
                .unwrap_err()
                .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::DuplicateRecoveryReceipt)
        );

        let mut orphan = receipt;
        orphan.request_identity_digest = "1".repeat(64);
        // Distinct lower-sort key forces lookup of the second row. The
        // unreferenced receipt is rejected even when the real receipt passed.
        host.recovery_receipts[1] = orphan.clone();
        assert_eq!(
            check_recovery_host_fixture_v1(&host, &fresh)
                .unwrap_err()
                .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::MissingRecoveryReceipt)
        );

        // Isolate lookup refusal. An orphan could produce the same error even
        // if a missing key had fallen through to the valid receipt.
        host.recovery_receipts.truncate(1);
        for missing in ["0".repeat(64), "f".repeat(64)] {
            host.deliveries[0].ordinals[0].recovery_request_digest = Some(missing);
            assert_eq!(
                check_recovery_host_fixture_v1(&host, &fresh)
                    .unwrap_err()
                    .downcast_ref::<RetainedHostRefusalV1>(),
                Some(&RetainedHostRefusalV1::MissingRecoveryReceipt)
            );
        }

        // Unreferenced duplicate keys retain the existing orphan refusal.
        let mut filtered = retained_filtered_host_fixture_v1();
        filtered.recovery_receipts.extend([orphan.clone(), orphan]);
        assert_eq!(
            check_filtered_host_fixture_v1(&filtered)
                .unwrap_err()
                .downcast_ref::<RetainedHostRefusalV1>(),
            Some(&RetainedHostRefusalV1::MissingRecoveryReceipt)
        );
    }

    #[test]
    fn shared_retained_host_tail_fits_bounded_work() {
        let (mut host, fresh) = retained_recovery_host_fixture_v1();
        let receipt = &mut host.recovery_receipts[0];
        receipt.tail_event_ids = (1_000..1_128).collect();
        // Fixed goldens from the S V2 request and tail framing for these IDs.
        receipt.tail_custody_digest =
            "d9e13bc4e7866dbbf6cbe54c3601a3138ec6b83ed00b503ec4bd1ee829551de9".into();
        receipt.request_identity_digest =
            "b004baaef6bfa8319cd41b95339929f46e22654a1cf7f6c13da5d08df346c005".into();
        receipt.applied_request_identity_digest = receipt.request_identity_digest.clone();
        let delivery = &mut host.deliveries[0];
        let mut ordinal = delivery.ordinals[0].clone();
        ordinal.recovery_request_digest = Some(receipt.request_identity_digest.clone());
        delivery.ordinals = receipt
            .tail_event_ids
            .iter()
            .enumerate()
            .map(|(index, &event_id)| NativeCutoverEvidenceOrdinalV1 {
                source_ordinal: u64::try_from(index).unwrap(),
                emitted_event_id: Some(event_id),
                ..ordinal.clone()
            })
            .collect();
        delivery.input_count = 128;
        host.active_units = 128;
        host.capacity = 128;
        // The old repeat path charged 256 KiB for tail bytes alone.
        with_host_inventory_limits_v1(
            &host,
            WorkLimits {
                reads: 8_192,
                writes: 0,
                bytes: 192 * 1024,
                mutations: 0,
            },
            |charged| {
                require_settled_retained_host_inventory_v1(
                    &host,
                    charged,
                    |_, _| {
                        Err(anyhow!(
                            "full-only recovery must not require a Watch request"
                        ))
                    },
                    |receipt, charged| {
                        require_recovery_receipt_v1(
                            receipt,
                            "/workspace",
                            "/workspace",
                            &fresh,
                            charged,
                        )
                    },
                )
            },
        )
        .expect("128 covered ordinals fit the metered budget without rehashing the tail");
    }

    #[test]
    fn retained_host_receipt_index_fits_bounded_lookup_work() {
        let (receipt, _) = recovery_receipt_shape_fixture_v1();
        let receipts: Vec<_> = (0..128)
            .rev()
            .map(|key| NativeCutoverEvidenceHostRecoveryReceiptV1 {
                request_identity_digest: format!("{key:064x}"),
                ..receipt.clone()
            })
            .collect();
        let mut rows: Vec<_> = receipts
            .iter()
            .map(|receipt| RetainedHostRecoveryReceiptV1 {
                receipt,
                verified: false,
            })
            .collect();
        let mut charged = WorkBudget::new(
            WorkLimits {
                reads: 4_096,
                writes: 0,
                bytes: 512 * 1024,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(10),
        );
        sort_host_items_v1(&mut rows, |left, right| {
            compare_host_digests_v1(
                &left.receipt.request_identity_digest,
                &right.receipt.request_identity_digest,
                &mut charged,
            )
        })
        .expect("receipt ordering fits the metered budget");
        for receipt in &receipts {
            let actual = recovery_receipt_for_digest_v1(
                &mut rows,
                &receipt.request_identity_digest,
                &mut charged,
            )
            .expect("128 exact lookups fit the metered budget");
            assert!(std::ptr::eq(actual.receipt, receipt));
        }
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
        let no_scope = require_settled_retained_host_inventory_v1(
            &valid,
            &mut budget(),
            |_, _| {
                Err(anyhow!(
                    "filtered fixture must not request Source membership"
                ))
            },
            |_, _| {
                Err(anyhow!(
                    "filtered fixture must not request recovery receipt"
                ))
            },
        )
        .unwrap_err();
        assert!(matches!(
            no_scope.downcast_ref::<quanta_index_core::index::IndexError>(),
            Some(quanta_index_core::index::IndexError::ResourceLimited)
        ));
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
