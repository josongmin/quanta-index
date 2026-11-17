//! Relational admission of the retained Native Source lineage and its
//! independent Legacy, ACK, outbox, and AST facts.
//!
//! This borrowed projection is not a cutover permit. The receiver must still
//! compare the actual Q publication, root custody, FDs, host and deployment.

use crate::index::{UpdateKind, UpdateRequest};
use crate::native_cutover_evidence_v1::{
    MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1, NativeCutoverAstWorkPhaseV1,
    NativeCutoverAttemptFenceWireV1, NativeCutoverDigest64V1, NativeCutoverEvidenceCheckpointAckV1,
    NativeCutoverEvidenceFreshSdkV1, NativeCutoverEvidenceSdkBindingV1,
    NativeCutoverEvidenceSourcePublicationBindingV1,
    NativeCutoverEvidenceSourceFactV1 as Fact, NativeCutoverEvidenceTargetV1,
    NativeCutoverEvidenceV1, NativeCutoverEvidenceV5ArtifactV1, NativeCutoverReplayPhaseV1,
    NativeCutoverReservationEvidenceV1,
};
use crate::native_protocol::{
    NATIVE_STUB_AST_PRODUCER, NATIVE_STUB_AST_PROJECTION, NATIVE_STUB_AST_SCHEMA,
    native_stub_certificate_policy,
};
use std::ops::Range;
use std::cmp::Ordering;

#[derive(Debug)]
pub enum NativeCutoverSourceJoinErrorV1<E> {
    Refused(&'static str),
    Work(E),
}

/// Every optional reference is an exact member of `evidence.source_facts`.
///
/// The latest Published AST request is borrowed for the receiver's independent
/// comparison with Q's actual published view and `SourceCut`.
pub struct NativeCutoverJoinedSourceFactsV1<'a> {
    pub baseline: &'a NativeCutoverEvidenceTargetV1,
    /// Scope of the header-selected initial FULL; the Q lease owner must compare it.
    pub baseline_candidate_root_nonce: Option<crate::index::Digest>,
    /// The latest Native reservation, distinct from the original FULL target.
    pub current_native_target: &'a NativeCutoverEvidenceTargetV1,
    pub fresh_sdk: &'a NativeCutoverEvidenceFreshSdkV1,
    pub v5_artifact: &'a NativeCutoverEvidenceV5ArtifactV1,
    pub v5_artifacts: &'a [NativeCutoverEvidenceV5ArtifactV1],
    pub initial_reservation: &'a Fact,
    pub replay: Option<&'a Fact>,
    /// The latest Native V9, distinct from the original FULL V9.
    pub current_native_replay: Option<&'a Fact>,
    pub pending_carrier: Option<&'a Fact>,
    pub pending_attempt: Option<&'a Fact>,
    pub custody_head: Option<&'a Fact>,
    pub terminal_receipt: Option<&'a Fact>,
    pub sdk_binding: Option<&'a Fact>,
    pub sdk_acked_binding: Option<&'a Fact>,
    pub checkpoint_ack: Option<&'a Fact>,
    pub ast_frontier: Option<&'a Fact>,
    pub ast_published_work: Option<&'a Fact>,
    pub current_legacy_replay: Option<&'a Fact>,
    /// The ordinary initial V9's signed Source association; no pending ACK
    /// row exists for this producer path.
    pub current_legacy_ordinary_binding: Option<&'a NativeCutoverEvidenceSourcePublicationBindingV1>,
    pub current_legacy_sdk_ack: Option<&'a Fact>,
    /// Prior M selected by the current V5 predecessor pair. An independent
    /// initial FULL has no predecessor pair or typed prior-M selector here.
    pub prior_sdk_acked_binding: Option<&'a Fact>,
    completed_watch_sources: &'a [Fact],
    completed_source_requests: &'a [Fact],
    native_initial_sources: &'a [Fact],
    native_sdk_sources: &'a [Fact],
}

impl<'a> NativeCutoverJoinedSourceFactsV1<'a> {
    #[must_use]
    pub fn v5_artifact_for_operation_v1(
        &self,
        operation: crate::index::OperationKey,
    ) -> Option<&'a NativeCutoverEvidenceV5ArtifactV1> {
        let row = self.v5_artifacts.binary_search_by_key(&operation, |artifact| artifact.operation).ok()?;
        self.v5_artifacts.get(row)
    }
    #[must_use]
    pub fn current_legacy_ordinary_source_binding_v1(
        &self,
    ) -> Option<&'a NativeCutoverEvidenceSourcePublicationBindingV1> {
        self.current_legacy_ordinary_binding
    }

    #[must_use]
    pub fn completed_watch_source_for_operation_v1(
        &self,
        operation: crate::index::OperationKey,
    ) -> Option<(&'a NativeCutoverEvidenceSourcePublicationBindingV1, &'a [u128])> {
        let row = self.completed_watch_sources.binary_search_by(|fact| match fact {
            Fact::CompletedSourceRequest { native_operation: Some(actual), .. } => actual.cmp(&operation),
            _ => Ordering::Less,
        }).ok()?;
        match self.completed_watch_sources.get(row)? {
            Fact::CompletedSourceRequest { source, event_ids, .. } => {
                Some((source, event_ids.as_slice()))
            }
            _ => None,
        }
    }

    pub fn completed_source_request_for_event_v1<E>(
        &self,
        event_id: u128,
        charge: &mut impl FnMut(u64) -> Result<(), E>,
    ) -> Result<Option<(&'a NativeCutoverEvidenceSourcePublicationBindingV1, &'a [u128], Option<crate::index::OperationKey>)>, E> {
        for fact in self.completed_source_requests {
            charge((std::mem::size_of::<crate::index::OperationKey>() + 3 * 64) as u64)?;
            let Fact::CompletedSourceRequest { native_operation, source, event_ids } = fact else {
                continue;
            };
            let mut low = 0;
            let mut high = event_ids.len();
            while low < high {
                charge(std::mem::size_of::<u128>() as u64)?;
                let middle = low + (high - low) / 2;
                if event_ids[middle] < event_id { low = middle + 1; } else { high = middle; }
            }
            if low < event_ids.len() {
                charge(std::mem::size_of::<u128>() as u64)?;
                if event_ids[low] == event_id {
                    return Ok(Some((source, event_ids.as_slice(), *native_operation)));
                }
            }
        }
        Ok(None)
    }

    #[must_use]
    pub fn native_initial_source_for_operation_v1(
        &self,
        operation: crate::index::OperationKey,
    ) -> Option<(&'a NativeCutoverEvidenceSourcePublicationBindingV1, &'a str)> {
        let row = self.native_initial_sources.binary_search_by(|fact| match fact {
            Fact::NativeInitialSourceBinding { operation: actual, .. } => actual.cmp(&operation),
            _ => Ordering::Less,
        }).ok()?;
        match self.native_initial_sources.get(row)? {
            Fact::NativeInitialSourceBinding { source, aggregate_identity_digest, .. } =>
                Some((source, aggregate_identity_digest.as_str())),
            _ => None,
        }
    }

    #[must_use]
    pub fn native_sdk_source_for_operation_v1(
        &self,
        operation: crate::index::OperationKey,
    ) -> Option<(&'a NativeCutoverEvidenceSourcePublicationBindingV1, &'a str)> {
        let row = self.native_sdk_sources.binary_search_by(|fact| match fact {
            Fact::NativeSdkSourceBinding { operation: actual, .. } => actual.cmp(&operation),
            _ => Ordering::Less,
        }).ok()?;
        match self.native_sdk_sources.get(row)? {
            Fact::NativeSdkSourceBinding { source, aggregate_identity_digest, .. } =>
                Some((source, aggregate_identity_digest.as_str())),
            _ => None,
        }
    }

    #[must_use]
    pub fn native_update_request_v1(&self) -> Option<&'a UpdateRequest> {
        match self.replay {
            Some(Fact::Replay {
                native_update_request,
                ..
            }) => native_update_request.as_ref(),
            _ => None,
        }
    }

    #[must_use]
    pub fn current_native_update_request_v1(&self) -> Option<&'a UpdateRequest> {
        match self.current_native_replay {
            Some(Fact::Replay { native_update_request: Some(request), .. }) => Some(request),
            _ => None,
        }
    }

    #[must_use]
    pub fn ast_update_request_v1(&self) -> Option<&'a UpdateRequest> {
        match self.ast_published_work {
            Some(Fact::AstWork { request, .. }) => request.as_ref(),
            _ => None,
        }
    }
}

/// Internal ordered associations, not a second partial public admission.
/// The receiver uses only the complete `join_native_cutover_source_facts_v1`.
struct NativeCutoverSourceLineageV1<'e, 's> {
    facts: &'e [Fact],
    reservation_rows: &'s [usize],
    replay_rows: &'s [usize],
    current_v5: &'e NativeCutoverEvidenceV5ArtifactV1,
    original_index: usize,
    current_index: usize,
}

impl<'e> NativeCutoverSourceLineageV1<'e, '_> {
    #[must_use]
    fn len(&self) -> usize { self.reservation_rows.len() }
    #[must_use]
    fn original_full_v1(&self) -> Option<(&'e Fact, &'e Fact)> { self.at(self.original_index) }
    #[must_use]
    fn current_v1(&self) -> Option<(&'e Fact, &'e Fact)> {
        self.at(self.current_index)
    }
    #[must_use]
    fn at(&self, index: usize) -> Option<(&'e Fact, &'e Fact)> {
        let reservation = self.facts.get(*self.reservation_rows.get(index)?)?;
        let replay = self.facts.get(*self.replay_rows.get(index)?)?;
        Some((reservation, replay))
    }
}

fn sift_lineage_heap<E>(
    rows: &mut [usize],
    mut root: usize,
    end: usize,
    compare: &mut impl FnMut(usize, usize) -> Result<Ordering, NativeCutoverSourceJoinErrorV1<E>>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    loop {
        let child = root.checked_mul(2).and_then(|v| v.checked_add(1))
            .ok_or_else(|| refuse("lineage heap index overflow"))?;
        if child >= end { return Ok(()); }
        let mut largest = root;
        if compare(*rows.get(largest).ok_or_else(|| refuse("lineage heap root absent"))?,
                   *rows.get(child).ok_or_else(|| refuse("lineage heap child absent"))?)? == Ordering::Less {
            largest = child;
        }
        let right = child.checked_add(1).ok_or_else(|| refuse("lineage heap index overflow"))?;
        if right < end && compare(
            *rows.get(largest).ok_or_else(|| refuse("lineage heap child absent"))?,
            *rows.get(right).ok_or_else(|| refuse("lineage heap right absent"))?,
        )? == Ordering::Less { largest = right; }
        if largest == root { return Ok(()); }
        rows.swap(root, largest);
        root = largest;
    }
}

fn sort_lineage_rows<E>(
    rows: &mut [usize],
    compare: &mut impl FnMut(usize, usize) -> Result<Ordering, NativeCutoverSourceJoinErrorV1<E>>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let length = rows.len();
    for root in (0..length/2).rev() {
        sift_lineage_heap(rows, root, length, compare)?;
    }
    for end in (1..rows.len()).rev() {
        rows.swap(0, end);
        sift_lineage_heap(rows, 0, end, compare)?;
    }
    Ok(())
}

/// Associate all Native reservations and V9s without requiring contiguous
/// operation sequence. Both scratch vectors must have caller-funded capacity
/// for the entire bounded Source census before this function is entered.
/// Heap sorting is fallible and charges every comparison, O(N log N).
fn fold_native_source_lineage_v1<'e, 's, E>(
    evidence: &'e NativeCutoverEvidenceV1,
    reservation_rows: &'s mut Vec<usize>,
    replay_rows: &'s mut Vec<usize>,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<NativeCutoverSourceLineageV1<'e, 's>, NativeCutoverSourceJoinErrorV1<E>> {
    if evidence.source_facts.len() > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1
        || evidence.v5_artifacts.is_empty()
        || evidence.v5_artifacts.len() > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1 {
        return Err(refuse("lineage census or V5 count differs"));
    }
    let required = evidence.source_facts.len();
    if reservation_rows.capacity() < required || replay_rows.capacity() < required {
        return Err(refuse("lineage scratch was not preadmitted"));
    }
    reservation_rows.clear();
    replay_rows.clear();
    let mut allocator = None;
    for (index, fact) in evidence.source_facts.iter().enumerate() {
        work(charge, 1)?;
        match fact {
            Fact::Allocator { next_operation_sequence } => {
                if allocator.replace(*next_operation_sequence).is_some() {
                    return Err(refuse("duplicate Source allocator"));
                }
            }
            Fact::Reservation { .. } => reservation_rows.push(index),
            Fact::Replay { native_update_request: Some(_), .. } => replay_rows.push(index),
            _ => {}
        }
    }
    if reservation_rows.is_empty() || reservation_rows.len() != replay_rows.len() {
        return Err(refuse("Native reservation and V9 counts differ"));
    }
    // Global OperationKey identifies a durable reservation; each candidate
    // root independently starts its Source event cursor at zero. The two
    // caller-funded indices are reused for two O(N log N) sorted passes.
    sort_lineage_rows(reservation_rows, &mut |left, right| {
        work(charge, 48)?;
        let key = |index| match evidence.source_facts.get(index) {
            Some(Fact::Reservation { target, candidate_root_nonce, .. }) =>
                Some((candidate_root_nonce.map(|nonce| nonce.0), target.event_sequence)),
            _ => None,
        };
        Ok(key(left).ok_or_else(|| refuse("lineage reservation row absent"))?
            .cmp(&key(right).ok_or_else(|| refuse("lineage reservation row absent"))?))
    })?;
    sort_lineage_rows(replay_rows, &mut |left, right| {
        work(charge, std::mem::size_of::<crate::index::OperationKey>())?;
        let key = |index| match evidence.source_facts.get(index) {
            Some(Fact::Replay { native_update_request: Some(request), .. }) => Some(request.operation),
            _ => None,
        };
        Ok(key(left).ok_or_else(|| refuse("lineage V9 row absent"))?
            .cmp(&key(right).ok_or_else(|| refuse("lineage V9 row absent"))?))
    })?;
    let baseline = &evidence.baseline;
    let full = evidence.header.original_full_operation;
    if baseline.operation != full
        || full.workspace != evidence.header.selected_source.workspace
        || full.epoch != evidence.header.selected_source.workspace_epoch
        || full.producer != evidence.header.selected_source.producer
        || full.stream != evidence.header.selected_source.stream
        || full.stream_epoch != evidence.header.selected_source.stream_epoch
        || baseline.text_admission != evidence.header.selected_source.text_admission {
        return Err(refuse("Native lineage differs from selected Source"));
    }
    let mut previous: Option<(&NativeCutoverEvidenceTargetV1, Option<crate::index::Digest>)> = None;
    let mut selected_root_head: Option<(crate::index::Digest, crate::index::OperationKey)> = None;
    for index in 0..reservation_rows.len() {
        work(charge, 1 + std::mem::size_of::<NativeCutoverEvidenceTargetV1>())?;
        let reservation = evidence.source_facts.get(*reservation_rows.get(index)
            .ok_or_else(|| refuse("lineage reservation index absent"))?)
            .ok_or_else(|| refuse("lineage reservation absent"))?;
        let Fact::Reservation { target, evidence: kind, candidate_root_nonce } = reservation else {
            return Err(refuse("lineage reservation type differs"));
        };
        if candidate_root_nonce.is_some_and(|nonce| nonce.0 == [0; 32])
            || target.operation.workspace != full.workspace
            || target.operation.epoch != full.epoch
            || target.operation.producer != full.producer
            || target.operation.stream != full.stream
            || target.operation.stream_epoch != full.stream_epoch
            || target.owner_identity_policy_digest != baseline.owner_identity_policy_digest
            || target.text_admission != baseline.text_admission
            || target.source_mode != crate::index::SourceOperationMode::Advance {
            return Err(refuse("Native reservation has foreign Source scope"));
        }
        if let Some((prior, prior_nonce)) = previous.filter(|(_, nonce)| nonce == candidate_root_nonce) {
            let predecessor_digest = match kind {
                NativeCutoverReservationEvidenceV1::Watch { prior_target_digest, .. } => prior_target_digest,
                NativeCutoverReservationEvidenceV1::Sdk {
                    identity_kind, attempt_identity_digest,
                    routing_identity_digest, prior_target_digest,
                } => {
                    work(charge, identity_kind.len().saturating_add(128))?;
                    if !matches!(identity_kind.as_str(), "Keyed" | "Keyless")
                        || [attempt_identity_digest, routing_identity_digest].into_iter()
                            .any(|digest| !digest.0.iter().all(|byte|
                                byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))) {
                        return Err(refuse("SDK reservation identity is not canonical"));
                    }
                    prior_target_digest
                }
                NativeCutoverReservationEvidenceV1::Initial => return Err(refuse("second initial FULL in one candidate root")),
            };
            if *predecessor_digest != prior.canonical_target_digest
                || target.event_predecessor != prior.event_sequence
                || target.event_sequence != prior.event_sequence.checked_add(1)
                    .ok_or_else(|| refuse("Native event sequence exhausted"))?
                || target.operation.sequence <= prior.operation.sequence {
                return Err(refuse("Native Source predecessor or root event differs"));
            }
            previous = Some((target, prior_nonce));
        } else {
            if !matches!(kind, NativeCutoverReservationEvidenceV1::Initial)
                || target.event_predecessor != 0 || target.event_sequence != 1 {
                return Err(refuse("candidate root has no exact initial FULL"));
            }
            previous = Some((target, *candidate_root_nonce));
        }
        // Only this root advances the selected FULL's Source head. Global
        // operations in other roots can be newer without changing that head.
        if let Some(nonce) = *candidate_root_nonce
            && (target.operation == full
                || selected_root_head.is_some_and(|(root, _)| root == nonce))
        {
            selected_root_head = Some((nonce, target.operation));
        }
        // Every reservation has a unique matching terminal V9. Root sorting
        // does not change the global operation lookup index.
        let mut low = 0;
        let mut high = replay_rows.len();
        while low < high {
            work(charge, std::mem::size_of::<crate::index::OperationKey>())?;
            let mid = low + (high - low) / 2;
            let replay = evidence.source_facts.get(*replay_rows.get(mid)
                .ok_or_else(|| refuse("lineage V9 index absent"))?)
                .ok_or_else(|| refuse("lineage V9 absent"))?;
            let Fact::Replay { native_update_request: Some(request), .. } = replay else {
                return Err(refuse("lineage V9 request absent"));
            };
            if request.operation < target.operation { low = mid + 1; } else { high = mid; }
        }
        let replay = evidence.source_facts.get(*replay_rows.get(low)
            .ok_or_else(|| refuse("Native V9 for reservation absent"))?)
            .ok_or_else(|| refuse("Native V9 for reservation absent"))?;
        let Fact::Replay { native_update_request: Some(request), phase, .. } = replay else {
            return Err(refuse("Native V9 request absent"));
        };
        if request.operation != target.operation
            || !matches!(phase, NativeCutoverReplayPhaseV1::Acked | NativeCutoverReplayPhaseV1::Cleaned)
            || request.expected_view != target.expected_view
            || !matches!(&request.kind, UpdateKind::SourceAdvance(transition)
                if transition.expected_source == target.expected_source) {
            return Err(refuse("Native V9 is not the settled reservation request"));
        }
    }
    // The global counter never resets when a new candidate root starts event1.
    // Sorting again lets AST work find its Source operation in logarithmic time.
    sort_lineage_rows(reservation_rows, &mut |left, right| {
        work(charge, std::mem::size_of::<crate::index::OperationKey>())?;
        let key = |index| match evidence.source_facts.get(index) {
            Some(Fact::Reservation { target, .. }) => Some(target.operation),
            _ => None,
        };
        Ok(key(left).ok_or_else(|| refuse("lineage reservation row absent"))?
            .cmp(&key(right).ok_or_else(|| refuse("lineage reservation row absent"))?))
    })?;
    let selected_head_operation = selected_root_head
        .map(|(_, operation)| operation)
        .ok_or_else(|| refuse("selected root Source head absent"))?;
    let current_v5 = evidence.v5_artifacts
        .binary_search_by_key(&selected_head_operation, |artifact| artifact.operation)
        .ok()
        .and_then(|row| evidence.v5_artifacts.get(row))
        .ok_or_else(|| refuse("current V5 absent"))?;
    let mut previous_v5_operation = None;
    for artifact in &evidence.v5_artifacts {
        work(charge, std::mem::size_of::<NativeCutoverEvidenceV5ArtifactV1>())?;
        if previous_v5_operation.is_some_and(|prior| prior >= artifact.operation) {
            return Err(refuse("V5 artifact operation is duplicated or unsorted"));
        }
        previous_v5_operation = Some(artifact.operation);
        let replay_row = replay_rows.binary_search_by(|row| match evidence.source_facts.get(*row) {
            Some(Fact::Replay { native_update_request: Some(request), .. }) =>
                request.operation.cmp(&artifact.operation),
            _ => Ordering::Less,
        }).map_err(|_| refuse("V5 artifact has no Native V9"))?;
        let Some(Fact::Replay {
            native_update_request: Some(request),
            phase: NativeCutoverReplayPhaseV1::Acked | NativeCutoverReplayPhaseV1::Cleaned,
            published_snapshot_id: Some(snapshot),
            prepared_artifact_digest: Some(prepared),
            source_bound_aggregate_receipt_digest: Some(aggregate),
            ..
        }) = replay_rows.get(replay_row).and_then(|row| evidence.source_facts.get(*row)) else {
            return Err(refuse("V5 artifact has no settled Native V9"));
        };
        if request.operation != artifact.operation
            || *snapshot != artifact.published_snapshot_id
            || *prepared != artifact.prepared_artifact_digest
            || *aggregate != artifact.aggregate_receipt_digest {
            return Err(refuse("V5 artifact differs from its Native V9"));
        }
    }
    let mut original_index = None;
    let mut current_index = None;
    let mut previous_operation = None;
    for (index, row) in reservation_rows.iter().enumerate() {
        work(charge, std::mem::size_of::<crate::index::OperationKey>())?;
        let Some(Fact::Reservation { target, evidence: kind, candidate_root_nonce }) =
            evidence.source_facts.get(*row) else { return Err(refuse("reservation operation row absent")); };
        if previous_operation.is_some_and(|prior| prior >= target.operation)
            || allocator.is_none_or(|next| next <= target.operation.sequence) {
            return Err(refuse("Native operation is duplicated or outside global allocator"));
        }
        previous_operation = Some(target.operation);
        if target.operation == full {
            if *target != *baseline || !matches!(kind, NativeCutoverReservationEvidenceV1::Initial)
                || candidate_root_nonce.is_none() || original_index.replace(index).is_some() {
                return Err(refuse("header-selected FULL has no observed candidate root"));
            }
        }
        if target.operation == current_v5.operation {
            if current_index.replace(index).is_some() {
                return Err(refuse("current V5 operation is duplicated"));
            }
        }
    }
    let original_index = original_index.ok_or_else(|| refuse("original FULL association absent"))?;
    let current_index = current_index.ok_or_else(|| refuse("current V5 reservation absent"))?;
    let current_reservation = evidence.source_facts.get(reservation_rows[current_index])
        .ok_or_else(|| refuse("current reservation absent"))?;
    let Fact::Reservation { target: current, evidence: current_kind, candidate_root_nonce } =
        current_reservation else { return Err(refuse("current reservation type differs")); };
    let baseline_nonce = match evidence.source_facts.get(reservation_rows[original_index]) {
        Some(Fact::Reservation { candidate_root_nonce: Some(nonce), .. }) => *nonce,
        _ => return Err(refuse("baseline candidate root absent")),
    };
    if *candidate_root_nonce != Some(baseline_nonce) {
        return Err(refuse("current V5 belongs to another candidate root"));
    }
    if selected_root_head != Some((baseline_nonce, current.operation)) {
        return Err(refuse("current V5 precedes the selected root's Source head"));
    }
    let predecessor = (current_v5.previous_transition_digest.as_ref(),
                       current_v5.base_source_digest.as_ref());
    let current_is_initial = matches!(current_kind, NativeCutoverReservationEvidenceV1::Initial);
    let expected_native_snapshot = if current_is_initial {
        evidence.fresh_sdk.committed_source_snapshot_id
    } else {
        evidence.fresh_sdk.target_snapshot_id
    };
    if (current_is_initial && predecessor != (None, None))
        || (!current_is_initial
            && !matches!(predecessor, (Some(_), Some(_))))
        || current_v5.published_snapshot_id != expected_native_snapshot
        || current_v5.lexical_revision_digest != evidence.fresh_sdk.sdk_generation_digest
        || allocator.is_none_or(|next| next <= current.operation.sequence) {
        return Err(refuse("current Native V5 or allocator differs from selected root"));
    }
    work(charge, 128)?;
    if evidence.fresh_sdk.target_snapshot_id == 0
        || (!current_is_initial && evidence.fresh_sdk.committed_source_snapshot_id
            != evidence.fresh_sdk.target_snapshot_id)
        || evidence.fresh_sdk.committed_source_snapshot_digest
            != evidence.fresh_sdk.physical_target_source_digest {
        return Err(refuse("Fresh committed Source differs from physical target"));
    }
    // The two indices have the same distinct OperationKeys in sorted order.
    let final_replay_row = replay_rows.get(current_index)
        .ok_or_else(|| refuse("current Native V9 absent"))?;
    let final_replay = evidence.source_facts.get(*final_replay_row)
        .ok_or_else(|| refuse("current Native V9 absent"))?;
    let Fact::Replay { published_snapshot_id, prepared_artifact_digest,
                       source_bound_aggregate_receipt_digest, .. } = final_replay else {
        return Err(refuse("current Native V9 type differs"));
    };
    work(charge, 128)?;
    if *published_snapshot_id != Some(current_v5.published_snapshot_id)
        || prepared_artifact_digest.as_ref() != Some(&current_v5.prepared_artifact_digest)
        || source_bound_aggregate_receipt_digest.as_ref()
            != Some(&current_v5.aggregate_receipt_digest) {
        return Err(refuse("current Native V9/V5 binding differs"));
    }
    Ok(NativeCutoverSourceLineageV1 {
        facts: &evidence.source_facts,
        reservation_rows,
        replay_rows,
        current_v5,
        original_index,
        current_index,
    })
}

/// Reuse the two caller-funded Source scratch arrays before Source fold
/// overwrites them. AST OperationKey is global, while Published event cursors
/// restart for each candidate root. Sorting each index costs O(N log N) work
/// and performs no allocation or physical reread.
fn validate_ast_inventory_v1<E>(
    evidence: &NativeCutoverEvidenceV1,
    operation_rows: &mut Vec<usize>,
    published_rows: &mut Vec<usize>,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let facts = &evidence.source_facts;
    if facts.len() > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1
        || operation_rows.capacity() < facts.len()
        || published_rows.capacity() < facts.len() {
        return Err(refuse("AST inventory scratch was not preadmitted"));
    }
    operation_rows.clear();
    published_rows.clear();
    let mut has_ast = false;
    for (index, fact) in facts.iter().enumerate() {
        work(charge, 1)?;
        match fact {
            Fact::AstFrontier { .. } => has_ast = true,
            Fact::AstWork { ast_operation, phase, .. } => {
                has_ast = true;
                if ast_operation.is_some() { operation_rows.push(index); }
                if *phase == NativeCutoverAstWorkPhaseV1::Published {
                    published_rows.push(index);
                }
            }
            _ => {}
        }
    }
    if !has_ast { return Ok(()); }
    sort_lineage_rows(operation_rows, &mut |left, right| {
        work(charge, std::mem::size_of::<crate::index::OperationKey>())?;
        let operation = |index| match facts.get(index) {
            Some(Fact::AstWork { ast_operation: Some(operation), .. }) => Some(*operation),
            _ => None,
        };
        Ok(operation(left).ok_or_else(|| refuse("AST operation index absent"))?
            .cmp(&operation(right).ok_or_else(|| refuse("AST operation index absent"))?))
    })?;
    let mut previous_operation = None;
    for row in operation_rows.iter() {
        work(charge, std::mem::size_of::<crate::index::OperationKey>())?;
        let Some(Fact::AstWork { ast_operation: Some(operation), .. }) = facts.get(*row) else {
            return Err(refuse("AST operation row absent"));
        };
        if previous_operation.is_some_and(|previous| previous >= *operation) {
            return Err(refuse("AST global operation is duplicated"));
        }
        previous_operation = Some(*operation);
    }
    sort_lineage_rows(published_rows, &mut |left, right| {
        work(charge, 48)?;
        let key = |index| match facts.get(index) {
            Some(Fact::AstWork { candidate_root_nonce, ast_event_sequence: Some(event), .. }) =>
                Some((candidate_root_nonce.map(|nonce| nonce.0), *event)),
            _ => None,
        };
        Ok(key(left).ok_or_else(|| refuse("Published AST event is absent"))?
            .cmp(&key(right).ok_or_else(|| refuse("Published AST event is absent"))?))
    })?;
    let ranges = mixed_ranges(facts, charge)?;
    let frontiers = mixed_slice(facts, &ranges, 10)?;
    let mut cursor = 0;
    let mut previous_scope: Option<Option<[u8; 32]>> = None;
    for frontier in frontiers {
        work(charge, 32)?;
        let Fact::AstFrontier { candidate_root_nonce, committed_event_sequence, .. } = frontier else {
            return Err(refuse("AST frontier row absent"));
        };
        let scope = candidate_root_nonce.map(|nonce| nonce.0);
        if previous_scope.is_some_and(|previous| previous >= scope) {
            return Err(refuse("AST frontier scope is duplicated or unsorted"));
        }
        previous_scope = Some(scope);
        let mut next_event = 1_u64;
        let mut previous_view_sequence = None;
        while cursor < published_rows.len() {
            work(charge, 48)?;
            let Some(Fact::AstWork {
                candidate_root_nonce: row_scope,
                ast_event_sequence: Some(event),
                published_view,
                ..
            }) = facts.get(published_rows[cursor]) else {
                return Err(refuse("Published AST event row absent"));
            };
            let row_scope = row_scope.map(|nonce| nonce.0);
            if row_scope != scope { break; }
            if *event != next_event {
                return Err(refuse("Published AST root event has a gap or duplicate"));
            }
            let view = published_view.ok_or_else(|| refuse("Published AST has no terminal view"))?;
            // Other publications can advance the view between two AST events.
            if previous_view_sequence.is_some_and(|prior| prior >= view.sequence) {
                return Err(refuse("Published AST view does not advance in root event order"));
            }
            previous_view_sequence = Some(view.sequence);
            next_event = next_event.checked_add(1)
                .ok_or_else(|| refuse("Published AST event cursor exhausted"))?;
            cursor += 1;
        }
        if next_event.checked_sub(1) != Some(*committed_event_sequence) {
            return Err(refuse("AST frontier does not equal Published event census"));
        }
    }
    if cursor != published_rows.len() {
        return Err(refuse("Published AST has no root frontier"));
    }
    Ok(())
}

/// Check every completed Watch event set without allocating an event-ID copy.
/// The two caller-funded row/cursor arrays form a min heap and are reused by
/// the AST census and Native lineage fold after this check.
fn validate_completed_watch_inventory_v1<E>(
    evidence: &NativeCutoverEvidenceV1,
    rows: &mut Vec<usize>,
    cursors: &mut Vec<usize>,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let facts = &evidence.source_facts;
    if facts.len() > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1
        || rows.capacity() < facts.len()
        || cursors.capacity() < facts.len()
    {
        return Err(refuse("completed Watch scratch was not preadmitted"));
    }
    rows.clear();
    cursors.clear();
    for (index, fact) in facts.iter().enumerate() {
        work(charge, 1)?;
        if let Fact::CompletedSourceRequest { event_ids, .. } = fact {
            work(charge, event_ids.len().saturating_mul(std::mem::size_of::<u128>())
                .saturating_add(3 * 64 + std::mem::size_of::<u64>()))?;
            fact.validate_source_publication_v1(&evidence.header.selected_source)
                .map_err(|_| refuse("completed Watch Source shape differs"))?;
            rows.push(index);
        }
    }
    sort_lineage_rows(rows, &mut |left, right| {
        work(charge, std::mem::size_of::<crate::index::OperationKey>())?;
        let operation = |index| match facts.get(index) {
            Some(Fact::CompletedSourceRequest { native_operation, source, .. }) =>
                Some((*native_operation, source.transition_digest)),
            _ => None,
        };
        let (left_operation, left_source) = operation(left)
            .ok_or_else(|| refuse("completed Source request absent"))?;
        let (right_operation, right_source) = operation(right)
            .ok_or_else(|| refuse("completed Source request absent"))?;
        Ok(match (left_operation, right_operation) {
            (None, None) => left_source.0.cmp(&right_source.0),
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(left), Some(right)) => left.cmp(&right),
        })
    })?;
    let mut prior_operation = None;
    let mut prior_source_transition = None;
    let mut seen_watch = false;
    for row in rows.iter() {
        work(charge, std::mem::size_of::<crate::index::OperationKey>())?;
        let Some(Fact::CompletedSourceRequest { native_operation, source, .. }) = facts.get(*row) else {
            return Err(refuse("completed Source row absent"));
        };
        match native_operation {
            None => {
                if seen_watch || prior_source_transition.is_some_and(|prior| prior >= source.transition_digest.0) {
                    return Err(refuse("Source-only request transition is duplicated or unsorted"));
                }
                prior_source_transition = Some(source.transition_digest.0);
            }
            Some(operation) => {
                seen_watch = true;
                if prior_operation.is_some_and(|prior| prior >= *operation) {
                    return Err(refuse("completed Watch operation is duplicated"));
                }
                prior_operation = Some(*operation);
            }
        }
        cursors.push(0);
    }
    for root in (0..rows.len() / 2).rev() {
        sift_completed_watch_heap_v1(facts, rows, cursors, root, charge)?;
    }
    let mut prior_event_id = None;
    while !rows.is_empty() {
        let event_id = completed_watch_heap_event_v1(facts, rows, cursors, 0)?;
        work(charge, std::mem::size_of::<u128>())?;
        if prior_event_id.is_some_and(|prior| prior >= event_id) {
            return Err(refuse("completed Watch event ID is duplicated across operations"));
        }
        prior_event_id = Some(event_id);
        let row = *rows.first().ok_or_else(|| refuse("completed Watch heap is empty"))?;
        let fact = facts.get(row).ok_or_else(|| refuse("completed Watch heap row absent"))?;
        let Fact::CompletedSourceRequest { event_ids, .. } = fact else {
            return Err(refuse("completed Watch heap category differs"));
        };
        let cursor = cursors.first_mut().ok_or_else(|| refuse("completed Watch cursor absent"))?;
        *cursor += 1;
        if *cursor == event_ids.len() {
            let last = rows.len() - 1;
            rows.swap(0, last);
            cursors.swap(0, last);
            let _ = rows.pop();
            let _ = cursors.pop();
        }
        if !rows.is_empty() {
            sift_completed_watch_heap_v1(facts, rows, cursors, 0, charge)?;
        }
    }
    Ok(())
}

fn completed_watch_heap_event_v1<E>(
    facts: &[Fact],
    rows: &[usize],
    cursors: &[usize],
    node: usize,
) -> Result<u128, NativeCutoverSourceJoinErrorV1<E>> {
    let row = *rows.get(node).ok_or_else(|| refuse("completed Watch heap node absent"))?;
    let cursor = *cursors.get(node).ok_or_else(|| refuse("completed Watch heap cursor absent"))?;
    match facts.get(row) {
        Some(Fact::CompletedSourceRequest { event_ids, .. }) => event_ids.get(cursor)
            .copied().ok_or_else(|| refuse("completed Watch event cursor out of bounds")),
        _ => Err(refuse("completed Watch heap row differs")),
    }
}

fn sift_completed_watch_heap_v1<E>(
    facts: &[Fact],
    rows: &mut Vec<usize>,
    cursors: &mut Vec<usize>,
    mut root: usize,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let length = rows.len();
    loop {
        let left = root * 2 + 1;
        if left >= length { break; }
        let right = left + 1;
        let mut smallest = left;
        if right < length {
            work(charge, std::mem::size_of::<u128>())?;
            if completed_watch_heap_event_v1::<E>(facts, rows, cursors, right)?
                < completed_watch_heap_event_v1::<E>(facts, rows, cursors, left)? {
                smallest = right;
            }
        }
        work(charge, std::mem::size_of::<u128>())?;
        if completed_watch_heap_event_v1::<E>(facts, rows, cursors, smallest)?
            >= completed_watch_heap_event_v1::<E>(facts, rows, cursors, root)? {
            break;
        }
        rows.swap(root, smallest);
        cursors.swap(root, smallest);
        root = smallest;
    }
    Ok(())
}

/// Complete, ordered bijection between Watch reservations and signed
/// completed-Source rows. The earlier lineage fold already checks that every
/// reservation has one terminal Native V9 and that its request is exact.
fn join_completed_watch_sources_v1<E>(
    lineage: &NativeCutoverSourceLineageV1<'_, '_>,
    completed: &[Fact],
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let mut next_completed = 0;
    let mut previous_completed = None;
    for fact in completed {
        work(charge, std::mem::size_of::<crate::index::OperationKey>())?;
        let Fact::CompletedSourceRequest { native_operation: Some(operation), .. } = fact else {
            return Err(refuse("completed Watch category differs"));
        };
        if previous_completed.is_some_and(|prior| prior >= *operation) {
            return Err(refuse("completed Watch source order or identity differs"));
        }
        previous_completed = Some(*operation);
    }
    for index in 0..lineage.len() {
        work(charge, std::mem::size_of::<crate::index::OperationKey>() + 16)?;
        let (reservation, replay) = lineage.at(index)
            .ok_or_else(|| refuse("completed Watch lineage association absent"))?;
        let Fact::Reservation { target, evidence: kind, .. } = reservation else {
            return Err(refuse("completed Watch reservation absent"));
        };
        let next = completed.get(next_completed);
        match kind {
            NativeCutoverReservationEvidenceV1::Watch { .. } => {
                let Some(Fact::CompletedSourceRequest { native_operation: Some(operation), source, .. }) = next else {
                    return Err(refuse("completed Watch Source absent"));
                };
                if *operation != target.operation {
                    return Err(refuse("completed Watch Source operation differs"));
                }
                let Fact::Replay {
                    native_update_request: Some(request),
                    phase: NativeCutoverReplayPhaseV1::Acked | NativeCutoverReplayPhaseV1::Cleaned,
                    published_snapshot_id: Some(snapshot),
                    ..
                } = replay else {
                    return Err(refuse("completed Watch V9 is not settled"));
                };
                if request.operation != *operation || *snapshot != source.target_snapshot_id {
                    return Err(refuse("completed Watch V9/Source publication differs"));
                }
                next_completed += 1;
            }
            _ => {
                if matches!(next, Some(Fact::CompletedSourceRequest { native_operation: Some(operation), .. })
                    if *operation <= target.operation)
                {
                    return Err(refuse("completed Watch Source has no Watch reservation"));
                }
            }
        }
    }
    if next_completed != completed.len() {
        return Err(refuse("completed Watch Source has no Watch reservation"));
    }
    Ok(())
}

fn split_completed_source_requests_v1<'a, E>(
    completed: &'a [Fact],
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(&'a [Fact], &'a [Fact]), NativeCutoverSourceJoinErrorV1<E>> {
    let mut source_only_count = 0;
    let mut prior_transition = None;
    let mut saw_watch = false;
    for fact in completed {
        work(charge, 64 + std::mem::size_of::<crate::index::OperationKey>())?;
        match fact {
            Fact::CompletedSourceRequest { native_operation: None, source, .. } => {
                if saw_watch || prior_transition.is_some_and(|prior| prior >= source.transition_digest.0) {
                    return Err(refuse("Source-only request order or transition differs"));
                }
                prior_transition = Some(source.transition_digest.0);
                source_only_count += 1;
            }
            Fact::CompletedSourceRequest { native_operation: Some(_), .. } => saw_watch = true,
            _ => return Err(refuse("completed Source request category differs")),
        }
    }
    Ok(completed.split_at(source_only_count))
}

fn join_one_native_bound_source_v1<E>(
    evidence: &NativeCutoverEvidenceV1,
    fact: &Fact,
    reservation: &Fact,
    replay: &Fact,
    expect_sdk: bool,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let (operation, aggregate_identity_digest) = match (expect_sdk, fact) {
        (false, Fact::NativeInitialSourceBinding { operation, aggregate_identity_digest, .. })
        | (true, Fact::NativeSdkSourceBinding { operation, aggregate_identity_digest, .. }) =>
            (operation, aggregate_identity_digest),
        _ => return Err(refuse(if expect_sdk { "Native SDK Source category differs" }
            else { "Native initial Source category differs" })),
    };
    work(charge, aggregate_identity_digest.len().saturating_add(3 * 64 + 8))?;
    fact.validate_source_publication_v1(&evidence.header.selected_source)
        .map_err(|_| refuse(if expect_sdk { "Native SDK Source shape differs" }
            else { "Native initial Source shape differs" }))?;
    let Fact::Reservation {
        target,
        evidence: reservation_kind,
        ..
    } = reservation else {
        return Err(refuse(if expect_sdk { "Native SDK Source reservation absent" }
            else { "Native initial Source has no Initial reservation" }));
    };
    if !matches!((expect_sdk, reservation_kind),
        (false, NativeCutoverReservationEvidenceV1::Initial)
            | (true, NativeCutoverReservationEvidenceV1::Sdk { .. })) {
        return Err(refuse(if expect_sdk { "Native SDK Source has no SDK reservation" }
            else { "Native initial Source has no Initial reservation" }));
    }
    let Fact::Replay {
        aggregate_identity_digest: replay_aggregate,
        native_update_request: Some(request),
        phase: NativeCutoverReplayPhaseV1::Acked | NativeCutoverReplayPhaseV1::Cleaned,
        ..
    } = replay else {
        return Err(refuse(if expect_sdk { "Native SDK Source has no settled V9" }
            else { "Native initial Source has no settled V9" }));
    };
    work(charge, replay_aggregate.len().saturating_add(
        std::mem::size_of::<crate::index::OperationKey>()))?;
    if target.operation != *operation
        || request.operation != *operation
        || replay_aggregate != aggregate_identity_digest
    {
        return Err(refuse(if expect_sdk { "Native SDK Source operation or V9 aggregate differs" }
            else { "Native initial Source operation or V9 aggregate differs" }));
    }
    Ok(())
}

/// A missing historical initial Source binding remains unknown, not a failure
/// of the old Native lineage. Every present binding must have exactly one
/// actual Initial reservation and settled V9 association.
fn join_native_bound_sources_v1<E>(
    evidence: &NativeCutoverEvidenceV1,
    lineage: &NativeCutoverSourceLineageV1<'_, '_>,
    sources: &[Fact],
    expect_sdk: bool,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let mut next_reservation = 0;
    let mut previous_operation = None;
    for fact in sources {
        work(charge, std::mem::size_of::<crate::index::OperationKey>())?;
        let operation = match (expect_sdk, fact) {
            (false, Fact::NativeInitialSourceBinding { operation, .. })
            | (true, Fact::NativeSdkSourceBinding { operation, .. }) => operation,
            _ => return Err(refuse("Native bound Source category differs")),
        };
        if previous_operation.is_some_and(|prior| prior >= *operation) {
            return Err(refuse("Native initial Source operation duplicated or unsorted"));
        }
        previous_operation = Some(*operation);
        while next_reservation < lineage.len() {
            work(charge, std::mem::size_of::<crate::index::OperationKey>())?;
            let (reservation, _) = lineage.at(next_reservation)
                .ok_or_else(|| refuse("Native initial Source lineage row absent"))?;
            let Fact::Reservation { target, .. } = reservation else {
                return Err(refuse("Native initial Source reservation absent"));
            };
            if target.operation >= *operation { break; }
            next_reservation += 1;
        }
        let (reservation, replay) = lineage.at(next_reservation)
            .ok_or_else(|| refuse("Native initial Source has no reservation"))?;
        join_one_native_bound_source_v1(evidence, fact, reservation, replay, expect_sdk, charge)?;
        next_reservation += 1;
    }
    Ok(())
}

fn require_selected_native_initial_source_parity_v1<E>(
    joined: &NativeCutoverJoinedSourceFactsV1<'_>,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    if joined.current_legacy_replay.is_some() { return Ok(()); }
    let Some((source, _)) = joined.native_initial_source_for_operation_v1(
        joined.current_native_target.operation,
    ) else { return Ok(()); };
    // M's completed pointer snapshot is not the frozen Q V9/V5 snapshot N.
    // S authenticates Source4 against M; fresh SDK selects that same M and
    // three digest axes, while committed_source_snapshot_id selects N.
    work(charge, 3 * 64 + 8)?;
    let fresh = joined.fresh_sdk;
    if source.transition_digest != fresh.source_manifest_digest
        || source.completion_pointer_digest != fresh.source_pointer_digest
        || source.physical_target_source_digest != fresh.physical_target_source_digest
        || source.target_snapshot_id != fresh.target_snapshot_id
    {
        return Err(refuse("selected Native initial Source pointer or digest differs from fresh SDK"));
    }
    Ok(())
}

/// Validate the settled signed census for a Native successor chain. The
/// scratch is caller-funded and never escapes; returned facts borrow only the
/// evidence. Q publication, Host and physical owner checks remain separate.
pub fn join_native_cutover_source_facts_v1<'a, E>(
    evidence: &'a NativeCutoverEvidenceV1,
    reservation_rows: &mut Vec<usize>,
    replay_rows: &mut Vec<usize>,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<NativeCutoverJoinedSourceFactsV1<'a>, NativeCutoverSourceJoinErrorV1<E>> {
    validate_completed_watch_inventory_v1(evidence, reservation_rows, replay_rows, charge)?;
    validate_ast_inventory_v1(evidence, reservation_rows, replay_rows, charge)?;
    let lineage = fold_native_source_lineage_v1(evidence, reservation_rows, replay_rows, charge)?;
    if lineage.len() == 1 {
        return join_initial_only_source_facts_v1(evidence, charge);
    }
    let facts = &evidence.source_facts;
    let ranges = &mixed_ranges(facts, charge)?;
    let completed_source_requests = mixed_slice(facts, ranges, 12)?;
    let (_, completed_watch_sources) = split_completed_source_requests_v1(completed_source_requests, charge)?;
    join_completed_watch_sources_v1(&lineage, completed_watch_sources, charge)?;
    let native_initial_sources = mixed_slice(facts, ranges, 13)?;
    join_native_bound_sources_v1(evidence, &lineage, native_initial_sources, false, charge)?;
    let native_sdk_sources = mixed_slice(facts, ranges, 14)?;
    join_native_bound_sources_v1(evidence, &lineage, native_sdk_sources, true, charge)?;
    let (initial_reservation, initial_replay) = lineage.original_full_v1()
        .ok_or_else(|| refuse("original FULL association absent"))?;
    let (current_reservation, current_replay) = lineage.current_v1()
        .ok_or_else(|| refuse("current Native association absent"))?;
    let Fact::Reservation { target: current_target, .. } = current_reservation else {
        return Err(refuse("current Native reservation absent"));
    };
    let Fact::Replay {
        workspace_root, contract_key, aggregate_identity_digest,
        closeout_id, attempt_id, format_version, state_revision,
        phase, published_snapshot_id,
        terminal_aggregate_receipt_digest, terminal_projection_receipt_digest,
        native_update_request: Some(_), ..
    } = initial_replay else { return Err(refuse("initial Native V9 absent")); };
    let revision = closeout_id.strip_prefix("initial-native-full:")
        .ok_or_else(|| refuse("initial FULL marker is not canonical"))?;
    let target_hex = match initial_reservation {
        Fact::Reservation { target, .. } => sa_payload_to_hex(&target.canonical_target_digest),
        _ => return Err(refuse("initial FULL target absent")),
    };
    work(charge, revision.len().saturating_add(attempt_id.len()).saturating_add(64))?;
    if !canonical_hex_digest(revision)
        || attempt_id.strip_prefix("source-target:").map(str::as_bytes)
            != Some(target_hex.as_slice())
        || !text_eq(workspace_root, &evidence.header.canonical_workspace_root, charge)?
        || !text_eq(contract_key, &evidence.header.contract_key, charge)?
        || !matches!(*format_version, 3 | 4)
        || *state_revision == 0
        || !matches!(phase, NativeCutoverReplayPhaseV1::Acked | NativeCutoverReplayPhaseV1::Cleaned)
        || published_snapshot_id.is_none_or(|snapshot| snapshot == 0)
        || terminal_aggregate_receipt_digest.as_deref().is_none_or(|v| !canonical_hex_digest(v))
        || terminal_projection_receipt_digest.as_deref().is_none_or(|v| !canonical_hex_digest(v)) {
        return Err(refuse("initial FULL producer identity or terminal V9 differs"));
    }
    let checkpoint = mixed_find(facts, mixed_range(ranges, 9)?,
        (aggregate_identity_digest, 0, 0), charge)?
        .ok_or_else(|| refuse("initial FULL global checkpoint absent"))?;
    let baseline_nonce = match initial_reservation {
        Fact::Reservation { candidate_root_nonce: Some(nonce), .. } => Some(*nonce),
        _ => return Err(refuse("selected candidate root nonce absent")),
    };
    let mut joined = NativeCutoverJoinedSourceFactsV1 {
        baseline: &evidence.baseline,
        baseline_candidate_root_nonce: baseline_nonce,
        current_native_target: current_target,
        fresh_sdk: &evidence.fresh_sdk,
        v5_artifact: lineage.current_v5,
        v5_artifacts: &evidence.v5_artifacts,
        initial_reservation,
        replay: Some(initial_replay),
        current_native_replay: Some(current_replay),
        pending_carrier: None,
        pending_attempt: None,
        custody_head: None,
        terminal_receipt: None,
        sdk_binding: None,
        sdk_acked_binding: None,
        checkpoint_ack: Some(checkpoint),
        ast_frontier: None,
        ast_published_work: None,
        current_legacy_replay: None,
        current_legacy_ordinary_binding: None,
        current_legacy_sdk_ack: None,
        prior_sdk_acked_binding: None,
        completed_watch_sources,
        completed_source_requests,
        native_initial_sources,
        native_sdk_sources,
    };
    if current_target.operation == evidence.baseline.operation {
        join_initial_full_history(&joined, None, charge)?;
    } else {
        join_checkpoint_ack(&joined, charge)?;
    }
    // Every Native successor V9 must have its own exact pending and global
    // checkpoint group. No historical V5 is compared to the current V5.
    let current_is_initial = matches!(current_reservation,
        Fact::Reservation { evidence: NativeCutoverReservationEvidenceV1::Initial, .. });
    // The fresh SDK ACK selects one publication. A selected Native
    // successor must own it.
    if !current_is_initial && evidence.legacy_publication.is_some() {
        return Err(refuse("current Legacy publication conflicts with selected Native successor"));
    }
    let mut fresh_sdk_ack = None;
    for index in 0..lineage.len() {
        if index == lineage.original_index { continue; }
        let (reservation, replay) = lineage.at(index)
            .ok_or_else(|| refuse("Native successor association absent"))?;
        let Fact::Reservation { evidence: reservation_kind, target, .. } = reservation else {
            return Err(refuse("Native successor target absent"));
        };
        if matches!(reservation_kind, NativeCutoverReservationEvidenceV1::Initial) {
            let Fact::Replay { workspace_root, contract_key, aggregate_identity_digest,
                closeout_id, attempt_id, format_version, state_revision, phase,
                published_snapshot_id: Some(snapshot),
                terminal_aggregate_receipt_digest: Some(terminal_aggregate),
                terminal_projection_receipt_digest: Some(terminal_projection),
                .. } = replay else { return Err(refuse("historical initial FULL V9 incomplete")); };
            let Some(revision) = closeout_id.strip_prefix("initial-native-full:") else {
                return Err(refuse("historical initial FULL marker differs"));
            };
            work(charge, workspace_root.len().saturating_add(contract_key.len())
                .saturating_add(attempt_id.len()).saturating_add(128))?;
            if !text_eq(workspace_root, &evidence.header.canonical_workspace_root, charge)?
                || !text_eq(contract_key, &evidence.header.contract_key, charge)?
                || !canonical_hex_digest(aggregate_identity_digest)
                || !canonical_hex_digest(revision)
                || attempt_id.strip_prefix("source-target:").map(str::as_bytes)
                    != Some(sa_payload_to_hex(&target.canonical_target_digest).as_slice())
                || !matches!(*format_version, 3 | 4) || *state_revision == 0 || *snapshot == 0
                || !matches!(phase, NativeCutoverReplayPhaseV1::Acked | NativeCutoverReplayPhaseV1::Cleaned)
                || !canonical_hex_digest(terminal_aggregate)
                || !canonical_hex_digest(terminal_projection) {
                return Err(refuse("historical initial FULL V9 foreign or unsettled"));
            }
            let checkpoint = mixed_find(facts, mixed_range(ranges, 9)?,
                (aggregate_identity_digest, 0, 0), charge)?
                .ok_or_else(|| refuse("historical initial FULL checkpoint absent"))?;
            let prior_replay = joined.replay;
            let prior_checkpoint = joined.checkpoint_ack;
            joined.replay = Some(replay);
            joined.checkpoint_ack = Some(checkpoint);
            join_checkpoint_ack(&joined, charge)?;
            joined.replay = prior_replay;
            joined.checkpoint_ack = prior_checkpoint;
            continue;
        }
        let Fact::Replay {
            workspace_root: replay_root, contract_key: replay_contract,
            aggregate_identity_digest: aggregate, closeout_id: closeout,
            attempt_id: replay_attempt, format_version: replay_format,
            state_revision: replay_revision, phase: replay_phase,
            published_snapshot_id: Some(snapshot),
            prepared_artifact_digest: Some(prepared_digest),
            source_bound_aggregate_receipt_digest: Some(aggregate_digest),
            terminal_aggregate_receipt_digest: Some(terminal_digest),
            terminal_projection_receipt_digest: Some(projection_digest),
            ..
        } = replay else { return Err(refuse("Native successor V9 is incomplete")); };
        work(charge, 1_usize.saturating_add(closeout.len())
            .saturating_add(replay_attempt.len()).saturating_add(256))?;
        if !text_eq(replay_root, &evidence.header.canonical_workspace_root, charge)?
            || !text_eq(replay_contract, &evidence.header.contract_key, charge)?
            || !matches!(replay_phase, NativeCutoverReplayPhaseV1::Acked | NativeCutoverReplayPhaseV1::Cleaned)
            || !matches!(*replay_format, 3 | 4)
            || *replay_revision == 0
            || *snapshot == 0
            || !canonical_hex_digest(aggregate)
            || !canonical_hex_digest(terminal_digest)
            || !canonical_hex_digest(projection_digest)
            || !closeout.starts_with("change-event:") {
            return Err(refuse("Native successor V9 is foreign or unsettled"));
        }
        let Some(plan) = closeout.strip_prefix("change-event:") else {
            return Err(refuse("Native successor closeout is not change-event"));
        };
        let Some((attempt_plan, suffix)) = replay_attempt.strip_prefix("change-event:")
            .and_then(|value| value.rsplit_once(":attempt-")) else {
            return Err(refuse("Native successor attempt is not change-event"));
        };
        let number = canonical_attempt(suffix)
            .filter(|number| *number != 0)
            .ok_or_else(|| refuse("Native successor attempt number differs"))?;
        if !text_eq(plan, attempt_plan, charge)? {
            return Err(refuse("Native successor plan differs"));
        }
        let carrier = mixed_find_pending(facts, mixed_range(ranges, 3)?, plan, number, charge)?;
        let Fact::PendingReplay { aggregate_identity_digest: pending_aggregate, .. } = carrier else {
            return Err(refuse("Native successor pending carrier absent"));
        };
        if !text_eq(aggregate, pending_aggregate, charge)? {
            return Err(refuse("Native successor carrier aggregate differs"));
        }
        let sdk = join_mixed_pending_group(facts, ranges, replay, carrier, charge)?;
        match (reservation_kind, sdk) {
            (NativeCutoverReservationEvidenceV1::Sdk { identity_kind, .. },
                Some(Fact::PendingStoreSdkAck { binding, .. })) => {
                work(charge, binding.ingress_kind.len()
                    .saturating_add(binding.client_idempotency_digest.as_ref().map_or(0, String::len)))?;
                if binding.ingress_kind != "sdk"
                    || (identity_kind == "Keyed") != binding.client_idempotency_digest.is_some() {
                    return Err(refuse("SDK reservation and M ingress identity differ"));
                }
            }
            (NativeCutoverReservationEvidenceV1::Watch { .. }, None) => {}
            _ => return Err(refuse("Native successor SDK/Watch completion differs")),
        }
        if let Some((source, signed_aggregate)) =
            joined.native_sdk_source_for_operation_v1(target.operation)
        {
            let Some(Fact::PendingStoreSdkAck { binding, .. }) = sdk else {
                return Err(refuse("Native SDK Source has no exact SDK ACK"));
            };
            work(charge, signed_aggregate.len().saturating_add(4 * 64 + 8))?;
            if signed_aggregate != aggregate
                || !ascii_digest_eq(&binding.transition_digest, &source.transition_digest, charge)?
                || !ascii_digest_eq(&binding.completion_pointer_digest, &source.completion_pointer_digest, charge)?
                || !ascii_digest_eq(&binding.target_source_digest, &source.physical_target_source_digest, charge)?
                || binding.target_snapshot != source.target_snapshot_id
            {
                return Err(refuse("Native SDK Source differs from exact SDK ACK/V9"));
            }
        }
        let checkpoint = mixed_find(facts, mixed_range(ranges, 9)?, (aggregate, 0, 0), charge)?
            .ok_or_else(|| refuse("Native successor global checkpoint absent"))?;
        let previous_replay = joined.replay;
        let previous_checkpoint = joined.checkpoint_ack;
        joined.replay = Some(replay);
        joined.checkpoint_ack = Some(checkpoint);
        join_checkpoint_ack(&joined, charge)?;
        joined.replay = previous_replay;
        joined.checkpoint_ack = previous_checkpoint;
        if target.operation == current_target.operation {
            work(charge, 128)?;
            if *snapshot != lineage.current_v5.published_snapshot_id
                || prepared_digest != &lineage.current_v5.prepared_artifact_digest
                || aggregate_digest != &lineage.current_v5.aggregate_receipt_digest {
                return Err(refuse("current Native V9/V5 differs"));
            }
        }
        if let Some(ack @ Fact::PendingStoreSdkAck { binding, .. }) = sdk {
            if (!current_is_initial || evidence.legacy_publication.is_some())
                && ascii_digest_eq(
                    &binding.transition_digest,
                    &evidence.fresh_sdk.source_manifest_digest,
                    charge,
                )?
            {
                // A fresh transition already selected as Legacy cannot also
                // be claimed by a historical Native ACK.
                if current_is_initial {
                    return Err(refuse("current Legacy SDK M conflicts with Native history ACK"));
                }
                if target.operation != current_target.operation
                    || !ascii_digest_eq(&binding.target_source_digest,
                        &evidence.fresh_sdk.physical_target_source_digest, charge)?
                    || !ascii_digest_eq(&binding.completion_pointer_digest,
                        &evidence.fresh_sdk.source_pointer_digest, charge)?
                    || binding.target_snapshot != evidence.fresh_sdk.target_snapshot_id
                    || fresh_sdk_ack.replace(ack).is_some() {
                    return Err(refuse("Fresh Native SDK M is foreign or ambiguous"));
                }
            }
        }
    }
    // An independent initial FULL has no pending SDK ACK. Historical M rows
    // were settled above, but the existing evidence has no typed prior-M
    // physical selector; do not promote one to N or guess which M is prior.
    if !current_is_initial {
        joined.sdk_acked_binding = Some(fresh_sdk_ack
            .ok_or_else(|| refuse("selected Native SDK M ACK absent"))?);
    }
    if let Some(selection) = join_mixed_legacy(evidence, &joined, charge)? {
        match selection {
            LegacyCurrentSelectionV1::ChangeEvent { replay, sdk_ack } => {
                joined.current_legacy_replay = Some(replay);
                joined.current_legacy_sdk_ack = Some(sdk_ack);
            }
            LegacyCurrentSelectionV1::Ordinary { replay, binding } => {
                joined.current_legacy_replay = Some(replay);
                joined.current_legacy_ordinary_binding = Some(binding);
            }
        }
    }
    if let (Some(previous_transition), Some(base_source)) = (
        lineage.current_v5.previous_transition_digest.as_ref(),
        lineage.current_v5.base_source_digest.as_ref(),
    ) {
        let mut prior_sdk_ack = None;
        for fact in mixed_slice(facts, ranges, 8)? {
            work(charge, 1)?;
            let Fact::PendingStoreSdkAck { binding, .. } = fact else {
                return Err(refuse("unacknowledged SDK mutation remains"));
            };
            if ascii_digest_eq(&binding.transition_digest, previous_transition, charge)? {
                if !ascii_digest_eq(&binding.target_source_digest, base_source, charge)?
                    || std::ptr::eq(fact, joined.sdk_acked_binding
                        .ok_or_else(|| refuse("current SDK ACK absent"))?)
                    || prior_sdk_ack.replace(fact).is_some() {
                    return Err(refuse("previous M ACK is foreign or ambiguous"));
                }
            }
            if !std::ptr::eq(fact, joined.sdk_acked_binding
                .ok_or_else(|| refuse("current SDK ACK absent"))?)
                && ascii_digest_eq(&binding.transition_digest,
                    &evidence.fresh_sdk.source_manifest_digest, charge)? {
                return Err(refuse("current M transition has another SDK ACK"));
            }
        }
        joined.prior_sdk_acked_binding = Some(prior_sdk_ack
            .ok_or_else(|| refuse("previous committed M ACK absent"))?);
    } else if lineage.current_v5.previous_transition_digest.is_some()
        || lineage.current_v5.base_source_digest.is_some() {
        return Err(refuse("current V5 predecessor pair is partial"));
    }
    join_ast_lineage(evidence, &lineage, ranges, &mut joined, charge)?;
    require_selected_native_initial_source_parity_v1(&joined, charge)?;
    // Reverse sweeps reject Native carriers that have no exact Native V9.
    // The Legacy sweep already rejects all attempt/custody/terminal/SDK rows
    // without a carrier and all checkpoints without a V9.
    for carrier in mixed_slice(facts, ranges, 3)? {
        let Fact::PendingReplay { aggregate_identity_digest, plan_id, attempt_id, .. } = carrier else {
            return Err(refuse("pending carrier category differs"));
        };
        let replay = mixed_find(facts, mixed_range(ranges, 2)?,
            (aggregate_identity_digest.as_str(), 0, 0), charge)?
            .ok_or_else(|| refuse("orphan Native pending carrier"))?;
        if let Fact::Replay { native_update_request: Some(_), closeout_id,
            attempt_id: replay_attempt, .. } = replay
            && !change_event_identity(closeout_id, replay_attempt, plan_id, *attempt_id, charge)? {
            return Err(refuse("Native carrier and V9 owner key differ"));
        }
    }
    Ok(joined)
}

fn refuse<E>(reason: &'static str) -> NativeCutoverSourceJoinErrorV1<E> {
    NativeCutoverSourceJoinErrorV1::Refused(reason)
}

fn work<E>(
    charge: &mut impl FnMut(u64) -> Result<(), E>,
    units: usize,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let units = u64::try_from(units).map_err(|_overflow| refuse("source join work overflow"))?;
    charge(units).map_err(NativeCutoverSourceJoinErrorV1::Work)
}

fn text_eq<E>(
    left: &str,
    right: &str,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<bool, NativeCutoverSourceJoinErrorV1<E>> {
    work(charge, left.len().saturating_add(right.len()))?;
    Ok(left == right)
}

fn ascii_digest_eq<E>(
    left: &str,
    right: &NativeCutoverDigest64V1,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<bool, NativeCutoverSourceJoinErrorV1<E>> {
    work(charge, left.len().saturating_add(64))?;
    Ok(left.as_bytes() == right.0)
}

fn canonical_attempt(value: &str) -> Option<u32> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.bytes().try_fold(0_u32, |number, byte| {
        number
            .checked_mul(10)?
            .checked_add(u32::from(byte.checked_sub(b'0')?))
    })
}

fn change_event_identity<E>(
    closeout: &str,
    attempt: &str,
    plan: &str,
    number: u32,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<bool, NativeCutoverSourceJoinErrorV1<E>> {
    work(
        charge,
        closeout
            .len()
            .saturating_add(attempt.len())
            .saturating_add(plan.len()),
    )?;
    let Some(closeout_plan) = closeout.strip_prefix("change-event:") else {
        return Ok(false);
    };
    let Some((attempt_plan, suffix)) = attempt
        .strip_prefix("change-event:")
        .and_then(|s| s.rsplit_once(":attempt-"))
    else {
        return Ok(false);
    };
    let attempt_number_matches = canonical_attempt(suffix) == Some(number);
    Ok(closeout_plan == plan && attempt_plan == plan && attempt_number_matches)
}

/// Match the actual SDK first-FULL producer identity without allocating a
/// rendered marker. The revision comes from the same committed SDK Source
/// that supplied the signed fresh snapshot; the target digest is the signed
/// reservation, independently compared with the Q BOOT operation.
fn initial_full_identity<E>(
    closeout: &str,
    attempt: &str,
    revision: &NativeCutoverDigest64V1,
    target: &crate::index::Digest,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<bool, NativeCutoverSourceJoinErrorV1<E>> {
    work(
        charge,
        closeout
            .len()
            .saturating_add(attempt.len())
            .saturating_add(128),
    )?;
    let Some(closeout_revision) = closeout.strip_prefix("initial-native-full:") else {
        return Ok(false);
    };
    let Some(attempt_target) = attempt.strip_prefix("source-target:") else {
        return Ok(false);
    };
    Ok(closeout_revision.as_bytes() == revision.0.as_slice()
        && attempt_target.as_bytes() == sa_payload_to_hex(target).as_slice())
}

/// Costs one work unit per scanned row and charges every dynamic text match.
/// It is intentionally strict: unrelated history cannot be silently skipped.
fn join_initial_only_source_facts_v1<'a, E>(
    evidence: &'a NativeCutoverEvidenceV1,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<NativeCutoverJoinedSourceFactsV1<'a>, NativeCutoverSourceJoinErrorV1<E>> {
    if evidence.source_facts.len() > MAX_NATIVE_CUTOVER_EVIDENCE_SEQUENCE_ITEMS_V1 {
        return Err(refuse("source fact count exceeds bound"));
    }
    if evidence.v5_artifacts.len() != 1 {
        return Err(refuse("initial FULL requires exactly one V5 artifact"));
    }
    let baseline = &evidence.baseline;
    let full = evidence.header.original_full_operation;
    if baseline.operation != full
        || evidence.header.selected_source.workspace != full.workspace
        || evidence.header.selected_source.workspace_epoch != full.epoch
        || evidence.header.selected_source.producer != full.producer
        || evidence.header.selected_source.stream != full.stream
        || evidence.header.selected_source.stream_epoch != full.stream_epoch
    {
        return Err(refuse("initial FULL target or selected Source differs"));
    }
    let v5_artifact = evidence
        .v5_artifacts
        .first()
        .ok_or_else(|| refuse("initial FULL V5 artifact absent"))?;
    let fresh_sdk = &evidence.fresh_sdk;
    work(charge, 1 + 64 * 2)?;
    if v5_artifact.operation != full
        || v5_artifact.published_snapshot_id == 0
        || fresh_sdk.committed_source_snapshot_id != v5_artifact.published_snapshot_id
        || fresh_sdk.target_snapshot_id == 0
        || fresh_sdk.committed_source_snapshot_digest != fresh_sdk.physical_target_source_digest
        || fresh_sdk.sdk_generation_digest != v5_artifact.lexical_revision_digest
    {
        return Err(refuse(
            "fresh SDK and exact initial FULL V5 artifact differ",
        ));
    }
    let minimum_next = full
        .sequence
        .checked_add(1)
        .ok_or_else(|| refuse("initial FULL sequence exhausted"))?;
    let mut allocator = None;
    let mut reservation = None;
    let mut replay = None;
    let mut pending = None;
    let mut attempt = None;
    let mut first_custody: Option<&Fact> = None;
    let mut custody_head: Option<&Fact> = None;
    let mut terminal = None;
    let mut sdk = None;
    let mut sdk_ack = None;
    let mut checkpoint = None;
    let mut ast_frontier = None;
    let mut ast_published_work = None;
    let mut native_initial_source = None;
    let mixed_legacy = evidence.legacy_publication.is_some();
    for fact in &evidence.source_facts {
        work(charge, 1)?;
        match fact {
            Fact::Allocator {
                next_operation_sequence,
            } => {
                if allocator.replace(*next_operation_sequence).is_some() {
                    return Err(refuse("duplicate Source allocator"));
                }
            }
            Fact::Reservation {
                target,
                evidence: NativeCutoverReservationEvidenceV1::Initial,
                candidate_root_nonce: Some(nonce),
            } => {
                if nonce.0 == [0; 32] { return Err(refuse("selected candidate nonce is zero")); }
                if reservation.replace(fact).is_some() || target != baseline {
                    return Err(refuse(
                        "initial reservation missing exact target or duplicated",
                    ));
                }
            }
            Fact::Reservation { .. } => return Err(refuse("post-FULL reservation remains")),
            Fact::Replay {
                native_update_request,
                ..
            } => {
                if mixed_legacy && native_update_request.is_none() {
                    continue;
                }
                if replay.replace(fact).is_some() {
                    return Err(refuse("multiple Native V9 replays remain"));
                }
            }
            Fact::PendingReplay { .. } => {
                if mixed_legacy {
                    continue;
                }
                if pending.replace(fact).is_some() {
                    return Err(refuse("multiple pending carriers remain"));
                }
            }
            Fact::PendingStoreAttempt { .. } => {
                if mixed_legacy {
                    continue;
                }
                if attempt.replace(fact).is_some() {
                    return Err(refuse("multiple pending attempts remain"));
                }
            }
            Fact::PendingStoreCustody {
                custody_revision,
                predecessor_owner_token,
                predecessor_workspace_identity_digest,
                predecessor_runtime_generation,
                ..
            } => {
                if mixed_legacy {
                    continue;
                }
                if let Some(Fact::PendingStoreCustody {
                    custody_revision: prior_revision,
                    owner_token: prior_owner,
                    owner_workspace_identity_digest: prior_identity,
                    owner_runtime_generation: prior_generation,
                    ..
                }) = custody_head
                {
                    if prior_revision.checked_add(1) != Some(*custody_revision)
                        || predecessor_owner_token != prior_owner
                        || predecessor_workspace_identity_digest != prior_identity
                        || predecessor_runtime_generation != prior_generation
                    {
                        return Err(refuse("pending custody chain differs"));
                    }
                } else if *custody_revision != 1 {
                    return Err(refuse("pending custody revision does not start at one"));
                } else {
                    first_custody = Some(fact);
                }
                custody_head = Some(fact);
            }
            Fact::PendingStoreTerminal { .. } => {
                if mixed_legacy {
                    continue;
                }
                if terminal.replace(fact).is_some() {
                    return Err(refuse("multiple terminal receipts remain"));
                }
            }
            Fact::PendingStoreSdk { .. } => {
                if mixed_legacy {
                    continue;
                }
                if sdk.replace(fact).is_some() {
                    return Err(refuse("multiple SDK bindings remain"));
                }
            }
            Fact::PendingStoreSdkAck { .. } => {
                if mixed_legacy {
                    continue;
                }
                if sdk_ack.replace(fact).is_some() {
                    return Err(refuse("multiple SDK ACKs remain"));
                }
            }
            Fact::PendingStoreCheckpoint {
                aggregate_identity_digest,
                ..
            } => {
                if mixed_legacy {
                    let Some(Fact::Replay {
                        aggregate_identity_digest: native_aggregate,
                        ..
                    }) = replay
                    else {
                        return Err(refuse("Native V9 absent before checkpoint census"));
                    };
                    if aggregate_identity_digest != native_aggregate {
                        continue;
                    }
                }
                if checkpoint.replace(fact).is_some() {
                    return Err(refuse("multiple checkpoint ACKs remain"));
                }
            }
            Fact::AstFrontier {
                workspace,
                workspace_epoch,
                producer,
                stream,
                stream_epoch,
                next_operation_sequence,
                committed_event_sequence,
                ..
            } => {
                if *workspace != full.workspace
                    || *workspace_epoch != full.epoch
                    || *producer != NATIVE_STUB_AST_PRODUCER
                    || *stream != crate::index::StreamId(*b"smca.ast.strm.v1")
                    || *stream_epoch != 1
                    || *next_operation_sequence == 0
                    || *committed_event_sequence >= *next_operation_sequence
                {
                    return Err(refuse("AST frontier differs from selected Source"));
                }
                if ast_frontier.replace(fact).is_some() {
                    return Err(refuse("multiple AST frontiers remain"));
                }
            }
            Fact::AstWork {
                source_operation,
                phase,
                ..
            } => {
                if *source_operation != full {
                    return Err(refuse("AST work names another Source"));
                }
                match phase {
                    NativeCutoverAstWorkPhaseV1::Queued
                    | NativeCutoverAstWorkPhaseV1::Claimed
                    | NativeCutoverAstWorkPhaseV1::Bound => {
                        return Err(refuse("active AST work remains"));
                    }
                    NativeCutoverAstWorkPhaseV1::Published => {
                        if ast_published_work.replace(fact).is_some() {
                            return Err(refuse("multiple Published AST work rows remain"));
                        }
                    }
                    NativeCutoverAstWorkPhaseV1::Refused
                    | NativeCutoverAstWorkPhaseV1::Superseded => {
                        return Err(refuse(
                            "AST refusal or supersession needs separate resolution",
                        ));
                    }
                }
            }
            Fact::Outbox { .. } => {
                if !mixed_legacy {
                    join_settled_outbox(fact, &evidence.header.repo_id, charge)?;
                }
            },
            Fact::CompletedSourceRequest { native_operation: Some(_), .. } => {
                return Err(refuse("initial FULL has a completed Watch Source without reservation"));
            }
            Fact::CompletedSourceRequest { native_operation: None, .. } => {}
            Fact::NativeInitialSourceBinding { .. } => {
                if native_initial_source.replace(fact).is_some() {
                    return Err(refuse("Native initial Source operation duplicated"));
                }
            }
            Fact::NativeSdkSourceBinding { .. } => {
                return Err(refuse("initial FULL has SDK Source without SDK reservation"));
            }
        }
    }
    let Some(initial_reservation) = reservation else {
        return Err(refuse("initial Source allocator or reservation absent"));
    };
    if allocator.is_none_or(|next| next < minimum_next) {
        return Err(refuse("initial Source allocator or reservation absent"));
    }
    let baseline_nonce = match initial_reservation {
        Fact::Reservation { candidate_root_nonce: Some(nonce), .. } => Some(*nonce),
        _ => return Err(refuse("selected candidate root nonce absent")),
    };
    if let Some(initial_source) = native_initial_source {
        join_one_native_bound_source_v1(evidence, initial_source,
            initial_reservation, replay.ok_or_else(|| refuse("initial Native V9 absent"))?,
            false, charge)?;
    }
    let native_initial_sources: &'a [Fact] = match native_initial_source {
        Some(fact) => std::slice::from_ref(fact),
        None => &[],
    };
    let completed_ranges = mixed_ranges(&evidence.source_facts, charge)?;
    let completed_source_requests = mixed_slice(&evidence.source_facts, &completed_ranges, 12)?;
    let (_, completed_watch_sources) = split_completed_source_requests_v1(completed_source_requests, charge)?;
    if !completed_watch_sources.is_empty() {
        return Err(refuse("initial FULL has a completed Watch Source without reservation"));
    }
    let mut joined = NativeCutoverJoinedSourceFactsV1 {
        baseline,
        baseline_candidate_root_nonce: baseline_nonce,
        current_native_target: baseline,
        fresh_sdk,
        v5_artifact,
        v5_artifacts: &evidence.v5_artifacts,
        initial_reservation,
        replay,
        current_native_replay: replay,
        pending_carrier: pending,
        pending_attempt: attempt,
        custody_head,
        terminal_receipt: terminal,
        sdk_binding: sdk,
        sdk_acked_binding: sdk_ack,
        checkpoint_ack: checkpoint,
        ast_frontier,
        ast_published_work,
        current_legacy_replay: None,
        current_legacy_ordinary_binding: None,
        current_legacy_sdk_ack: None,
        prior_sdk_acked_binding: None,
        completed_watch_sources,
        completed_source_requests,
        native_initial_sources,
        native_sdk_sources: &[],
    };
    join_history(&joined, first_custody, evidence, charge)?;
    if mixed_legacy {
        let selection = join_mixed_legacy(evidence, &joined, charge)?
            .ok_or_else(|| refuse("current Legacy SDK M ACK is absent"))?;
        match selection {
            LegacyCurrentSelectionV1::ChangeEvent { replay, sdk_ack } => {
                joined.current_legacy_replay = Some(replay);
                joined.current_legacy_sdk_ack = Some(sdk_ack);
            }
            LegacyCurrentSelectionV1::Ordinary { replay, binding } => {
                joined.current_legacy_replay = Some(replay);
                joined.current_legacy_ordinary_binding = Some(binding);
            }
        }
    }
    join_published_ast(&joined, charge)?;
    require_selected_native_initial_source_parity_v1(&joined, charge)?;
    Ok(joined)
}

/// Reduced historical outbox facts are admitted only because the signed S
/// Original already validated the frozen dispatch identity and locator. This
/// checks the same settled receipt axes without reopening GC-able payloads;
/// the destination Q head is a separate authority check.
fn join_settled_outbox<E>(
    fact: &Fact,
    expected_repo: &str,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let Fact::Outbox {
        dispatch_id,
        track,
        repo_id,
        revision_id,
        state,
        attempt_count: _,
        publish_binding,
        delivery_receipt,
        op_count_hint,
        semantic_replace_count_hint,
        semantic_tombstone_count_hint,
    } = fact else {
        return Err(refuse("outbox category differs"));
    };
    for value in [dispatch_id, track, repo_id, revision_id, state] {
        work(charge, value.len())?;
    }
    work(charge, 8)?;
    if dispatch_id.trim().is_empty()
        || repo_id.trim().is_empty()
        || revision_id.trim().is_empty()
        || state != "Delivered"
        || !text_eq(repo_id, expected_repo, charge)?
    {
        return Err(refuse("outbox is not a settled dispatch for this repo"));
    }
    let lexical = match track.as_str() {
        "Lexical" => true,
        "Structural" | "Semantic" | "Dirty" => false,
        _ => return Err(refuse("outbox track is unknown")),
    };
    if !lexical && (*semantic_replace_count_hint != 0 || *semantic_tombstone_count_hint != 0) {
        return Err(refuse("non-lexical outbox carries semantic count hints"));
    }
    let (Some(binding), Some(receipt)) = (publish_binding.as_ref(), delivery_receipt.as_ref()) else {
        return Err(refuse("Delivered outbox binding or receipt absent"));
    };
    work(charge, binding.manifest_digest_hex.len())?;
    work(charge, receipt.manifest_digest_hex.len())?;
    work(charge, 8)?;
    if binding.generation == 0
        || binding.manifest_digest_hex.trim().is_empty()
        || receipt.generation != binding.generation
        || !text_eq(
            &receipt.manifest_digest_hex,
            &binding.manifest_digest_hex,
            charge,
        )?
        || (track == "Dirty" && receipt.sealed)
        || (track != "Dirty" && !receipt.sealed)
    {
        return Err(refuse("Delivered outbox receipt differs from binding or seal policy"));
    }
    let accepted_count = u64::from(receipt.accepted_replace_scopes)
        .checked_add(u64::from(receipt.accepted_tombstone_scopes))
        .ok_or_else(|| refuse("outbox accepted operation count overflow"))?;
    if accepted_count != *op_count_hint
        || (lexical
            && (receipt.accepted_semantic_replace_scopes != *semantic_replace_count_hint
                || receipt.accepted_semantic_tombstone_scopes != *semantic_tombstone_count_hint))
        || (!lexical
            && (receipt.accepted_semantic_replace_scopes != 0
                || receipt.accepted_semantic_tombstone_scopes != 0))
    {
        return Err(refuse("Delivered outbox accepted counts differ from plan hints"));
    }
    if let Some(metrics) = receipt.semantic_admission_metrics.as_ref() {
        work(charge, 10)?;
        if !lexical
            || (metrics.raw_fallback_scope_count == 0 && metrics.raw_fallback_token_count > 0)
            || (metrics.raw_fallback_scope_count > 0 && metrics.raw_fallback_token_count == 0)
            || (metrics.raw_fallback_token_count > 0 && metrics.total_code_token_count == 0)
            || metrics.raw_fallback_token_count > metrics.total_code_token_count
        {
            return Err(refuse("Delivered outbox semantic admission metrics differ"));
        }
        let ratio = if metrics.total_code_token_count == 0 {
            0
        } else {
            u16::try_from(
                (u128::from(metrics.raw_fallback_token_count) * 10_000)
                    / u128::from(metrics.total_code_token_count),
            )
            .map_err(|_overflow| refuse("outbox fallback ratio exceeds u16"))?
        };
        if ratio != metrics.raw_fallback_ratio_bps || ratio > 2_000 {
            return Err(refuse("outbox fallback ratio differs or exceeds quota"));
        }
    }
    Ok(())
}

fn join_history<E>(
    joined: &NativeCutoverJoinedSourceFactsV1<'_>,
    first_custody: Option<&Fact>,
    evidence: &NativeCutoverEvidenceV1,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let Some(Fact::Replay {
        workspace_root,
        contract_key,
        aggregate_identity_digest,
        closeout_id,
        attempt_id,
        format_version,
        state_revision,
        phase,
        native_update_request,
        published_snapshot_id,
        prepared_artifact_digest,
        source_bound_aggregate_receipt_digest,
        terminal_projection_receipt_digest,
        ..
    }) = joined.replay
    else {
        return Err(refuse("initial FULL lacks exact V9 replay"));
    };
    let h = &evidence.header;
    if !text_eq(workspace_root, &h.canonical_workspace_root, charge)?
        || !text_eq(contract_key, &h.contract_key, charge)?
        || !matches!(*format_version, 3 | 4)
        || *state_revision == 0
        || !matches!(
            phase,
            NativeCutoverReplayPhaseV1::Acked | NativeCutoverReplayPhaseV1::Cleaned
        )
    {
        return Err(refuse("V9 is foreign or not terminal ACKed"));
    }
    let Some(request) = native_update_request else {
        return Err(refuse("ACKed V9 lacks exact Native request"));
    };
    if request.operation != h.original_full_operation {
        return Err(refuse("V9 native request names another Source"));
    }
    work(charge, 64 * 2)?;
    if *published_snapshot_id != Some(joined.v5_artifact.published_snapshot_id)
        || prepared_artifact_digest.as_ref() != Some(&joined.v5_artifact.prepared_artifact_digest)
        || source_bound_aggregate_receipt_digest.as_ref()
            != Some(&joined.v5_artifact.aggregate_receipt_digest)
    {
        return Err(refuse("V9 exact V5 snapshot or artifact binding differs"));
    }
    if closeout_id.starts_with("initial-native-full:") {
        return join_initial_full_history(joined, first_custody, charge);
    }
    let Some(Fact::PendingReplay {
        workspace_root: p_root,
        contract_key: p_contract,
        aggregate_identity_digest: p_aggregate,
        plan_id,
        attempt_id: p_attempt,
        owner_token,
        owner_fence_digest,
        custody_revision,
        workspace_identity_digest,
        runtime_generation,
        ..
    }) = joined.pending_carrier
    else {
        return Err(refuse("ACKed V9 lacks pending carrier"));
    };
    if !text_eq(p_root, workspace_root, charge)?
        || !text_eq(p_contract, contract_key, charge)?
        || !text_eq(p_aggregate, aggregate_identity_digest, charge)?
        || !change_event_identity(closeout_id, attempt_id, plan_id, *p_attempt, charge)?
        || *p_attempt == 0
        || *owner_token == 0
        || *runtime_generation == 0
        || owner_fence_digest.is_empty()
    {
        return Err(refuse("V9 and pending carrier identity differs"));
    }
    let Some(Fact::PendingStoreAttempt {
        plan_id: a_plan,
        attempt_id: a_attempt,
        state,
        execution_owner_token,
        fence,
        terminal_at_epoch_ms,
        ..
    }) = joined.pending_attempt
    else {
        return Err(refuse("ACKed V9 lacks pending attempt"));
    };
    if !text_eq(a_plan, plan_id, charge)?
        || a_attempt != p_attempt
        || state != "TerminalCommitted"
        || execution_owner_token.is_some()
        || terminal_at_epoch_ms.is_none()
    {
        return Err(refuse("pending attempt is not exact terminal"));
    }
    match fence {
        NativeCutoverAttemptFenceWireV1::BoundV2 {
            immutable_owner_token,
            workspace_identity_digest: a_identity,
            runtime_generation: a_generation,
        } if immutable_owner_token == owner_token
            && a_identity == workspace_identity_digest
            && a_generation == runtime_generation => {}
        NativeCutoverAttemptFenceWireV1::LegacyUnfenced
        | NativeCutoverAttemptFenceWireV1::BoundV2 { .. } => {
            return Err(refuse("pending attempt lacks exact immutable owner fence"));
        }
    }
    let mut final_owner = *owner_token;
    if let Some(Fact::PendingStoreCustody {
        predecessor_owner_token,
        predecessor_workspace_identity_digest,
        predecessor_runtime_generation,
        ..
    }) = first_custody
    {
        work(charge, 64)?;
        if predecessor_owner_token != owner_token
            || predecessor_workspace_identity_digest != workspace_identity_digest
            || predecessor_runtime_generation != runtime_generation
        {
            return Err(refuse("initial custody predecessor differs from carrier"));
        }
    }
    for fact in &evidence.source_facts {
        work(charge, 1)?;
        if let Fact::PendingStoreCustody {
            plan_id: c_plan,
            attempt_id: c_attempt,
            aggregate_identity_digest: c_aggregate,
            ..
        } = fact
            && (!text_eq(c_plan, plan_id, charge)?
                || c_attempt != p_attempt
                || !ascii_digest_eq(aggregate_identity_digest, c_aggregate, charge)?)
        {
            return Err(refuse(
                "intermediate custody row belongs to another carrier",
            ));
        }
    }
    if let Some(Fact::PendingStoreCustody {
        plan_id: c_plan,
        attempt_id: c_attempt,
        custody_revision: c_revision,
        aggregate_identity_digest: c_aggregate,
        owner_token: c_owner,
        ..
    }) = joined.custody_head
    {
        if !text_eq(c_plan, plan_id, charge)?
            || c_attempt != p_attempt
            || c_revision != custody_revision
            || !ascii_digest_eq(aggregate_identity_digest, c_aggregate, charge)?
        {
            return Err(refuse("pending custody head differs from carrier"));
        }
        final_owner = *c_owner;
    } else if *custody_revision != 0 {
        return Err(refuse("pending carrier names absent custody revision"));
    }
    let Some(Fact::PendingStoreTerminal {
        plan_id: t_plan,
        attempt_id: t_attempt,
        commit_owner_token,
        canonical_receipt_digest,
        published_snapshot_id: t_snapshot,
        checkpoint_generation,
        checkpoint_payload_digest,
        checkpoint_acked_at_epoch_ms,
        ..
    }) = joined.terminal_receipt
    else {
        return Err(refuse("ACKed V9 lacks terminal receipt"));
    };
    if !text_eq(t_plan, plan_id, charge)?
        || t_attempt != p_attempt
        || *commit_owner_token != final_owner
        || *t_snapshot != joined.v5_artifact.published_snapshot_id
        || checkpoint_generation.is_none()
        || checkpoint_payload_digest.is_none()
        || checkpoint_acked_at_epoch_ms.is_none()
    {
        return Err(refuse("terminal receipt does not join V9 and owner"));
    }
    let projection_digest = terminal_projection_receipt_digest
        .as_deref()
        .ok_or_else(|| refuse("V9 lacks terminal projection receipt digest"))?;
    if !text_eq(projection_digest, canonical_receipt_digest, charge)? {
        return Err(refuse("terminal projection receipt digest differs"));
    }
    join_checkpoint_ack(joined, charge)?;
    join_sdk_ack_pair(
        plan_id,
        *p_attempt,
        final_owner,
        joined
            .terminal_receipt
            .ok_or_else(|| refuse("terminal receipt absent"))?,
        joined.sdk_binding,
        joined.sdk_acked_binding,
        charge,
    )?;
    Ok(())
}

/// A completed SDK table row is emitted as one SdkAck fact, not an Sdk plus
/// SdkAck pair. An Sdk-only row remains an unacknowledged mutation.
fn join_sdk_ack_pair<E>(
    plan: &str,
    attempt: u32,
    owner: u64,
    terminal: &Fact,
    plain: Option<&Fact>,
    completed: Option<&Fact>,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let Fact::PendingStoreTerminal {
        canonical_receipt_digest,
        checkpoint_generation,
        checkpoint_payload_digest,
        checkpoint_acked_at_epoch_ms,
        ..
    } = terminal
    else {
        return Err(refuse("SDK ACK has no terminal receipt"));
    };
    match (plain, completed) {
        (None, None) => Ok(()),
        (
            None,
            Some(Fact::PendingStoreSdkAck {
                plan_id,
                attempt_id,
                commit_owner_token,
                terminal_receipt_digest,
                binding,
                canonical_receipt_digest: sdk_receipt,
                acked_binding,
                checkpoint_generation: sdk_generation,
                checkpoint_payload_digest: sdk_payload,
                checkpoint_acked_at_epoch_ms: sdk_time,
                ..
            }),
        ) => {
            work(
                charge,
                sdk_binding_text_bytes(binding)
                    .saturating_add(acked_binding.as_ref().map_or(0, sdk_binding_text_bytes))
                    .saturating_add(64),
            )?;
            let payload = sa_payload_to_hex(sdk_payload);
            if !text_eq(plan_id, plan, charge)?
                || *attempt_id != attempt
                || *commit_owner_token != owner
                || terminal_receipt_digest.0.as_slice() != canonical_receipt_digest.as_bytes()
                || !text_eq(sdk_receipt, canonical_receipt_digest, charge)?
                || acked_binding.as_ref() != Some(binding)
                || Some(*sdk_generation) != *checkpoint_generation
                || Some(*sdk_time) != *checkpoint_acked_at_epoch_ms
                || checkpoint_payload_digest.as_ref().map(|d| d.0.as_slice())
                    != Some(payload.as_slice())
            {
                return Err(refuse("SDK ACK and terminal receipt differ"));
            }
            Ok(())
        }
        (Some(_), _) | (None, Some(_)) => {
            Err(refuse("unacknowledged or duplicate SDK mutation remains"))
        }
    }
}

/// The actual first FULL has a V9 and a global checkpoint ACK, but no
/// change-event pending carrier, attempt, custody, terminal or SDK binding.
/// The latter rows belong to the separate DELTA owner and cannot be invented
/// to make an initial FULL look complete.
fn join_initial_full_history<E>(
    joined: &NativeCutoverJoinedSourceFactsV1<'_>,
    first_custody: Option<&Fact>,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    if joined.pending_carrier.is_some()
        || joined.pending_attempt.is_some()
        || first_custody.is_some()
        || joined.custody_head.is_some()
        || joined.terminal_receipt.is_some()
        || joined.sdk_binding.is_some()
        || joined.sdk_acked_binding.is_some()
    {
        return Err(refuse("initial FULL contains change-event pending state"));
    }
    let Some(Fact::Replay {
        closeout_id,
        attempt_id,
        terminal_aggregate_receipt_digest,
        terminal_projection_receipt_digest,
        ..
    }) = joined.replay
    else {
        return Err(refuse("initial FULL lacks exact V9 replay"));
    };
    if !initial_full_identity(
        closeout_id,
        attempt_id,
        &joined.fresh_sdk.sdk_generation_digest,
        &joined.baseline.canonical_target_digest,
        charge,
    )? {
        return Err(refuse(
            "initial FULL producer identity differs from SDK Source or target",
        ));
    }
    let Some(terminal_digest) = terminal_aggregate_receipt_digest.as_deref() else {
        return Err(refuse("initial FULL V9 lacks terminal aggregate digest"));
    };
    let Some(projection_digest) = terminal_projection_receipt_digest.as_deref() else {
        return Err(refuse("initial FULL V9 lacks terminal projection digest"));
    };
    work(
        charge,
        terminal_digest
            .len()
            .saturating_add(projection_digest.len()),
    )?;
    if !canonical_hex_digest(terminal_digest) || !canonical_hex_digest(projection_digest) {
        return Err(refuse("initial FULL terminal digest is not canonical"));
    }
    join_checkpoint_ack(joined, charge)
}

/// Both canonical producer protocols store this same V9 checkpoint ACK.
/// Keep its signed row/embedded receipt/revision comparison at one owner.
fn join_checkpoint_ack<E>(
    joined: &NativeCutoverJoinedSourceFactsV1<'_>,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let Some(Fact::Replay {
        workspace_root,
        contract_key,
        aggregate_identity_digest,
        state_revision,
        phase,
        terminal_aggregate_receipt_digest,
        checkpoint_ack,
        ..
    }) = joined.replay
    else {
        return Err(refuse("checkpoint lacks exact V9 replay"));
    };
    let Some(Fact::PendingStoreCheckpoint {
        workspace_root: k_root,
        contract_key: k_contract,
        aggregate_identity_digest: k_aggregate,
        terminal_state_revision,
        terminal_aggregate_receipt_digest: k_terminal,
        ack,
        ..
    }) = joined.checkpoint_ack
    else {
        return Err(refuse("ACKed V9 lacks durable checkpoint ACK"));
    };
    if !text_eq(k_root, workspace_root, charge)?
        || !text_eq(k_contract, contract_key, charge)?
        || !text_eq(k_aggregate, aggregate_identity_digest, charge)?
        || *terminal_state_revision >= *state_revision
        || !text_eq(&ack.terminal_aggregate_receipt_digest, k_terminal, charge)?
    {
        return Err(refuse("durable checkpoint ACK differs from V9"));
    }
    if matches!(phase, NativeCutoverReplayPhaseV1::Acked)
        && terminal_state_revision.checked_add(1) != Some(*state_revision)
    {
        return Err(refuse(
            "ACKed V9 revision differs from Terminal predecessor",
        ));
    }
    let embedded_ack = checkpoint_ack
        .as_ref()
        .ok_or_else(|| refuse("V9 lacks embedded checkpoint ACK"))?;
    work(
        charge,
        checkpoint_ack_text_bytes(embedded_ack).saturating_add(checkpoint_ack_text_bytes(ack)),
    )?;
    if embedded_ack != ack {
        return Err(refuse("embedded checkpoint ACK differs"));
    }
    let terminal_digest = terminal_aggregate_receipt_digest
        .as_deref()
        .ok_or_else(|| refuse("V9 lacks terminal aggregate receipt digest"))?;
    if !text_eq(terminal_digest, k_terminal, charge)? {
        return Err(refuse("terminal aggregate receipt digest differs"));
    }
    Ok(())
}

fn canonical_hex_digest(value: &str) -> bool {
    canonical_hex_digest_bytes(value.as_bytes())
}

fn canonical_hex_digest_bytes(value: &[u8]) -> bool {
    value.len() == 64
        && value
            .iter()
            .copied()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The signed AST census is sorted by aggregate digest. The pre-fold AST
/// inventory independently checks global OperationKey uniqueness and each
/// root's contiguous Published event cursor using the caller-funded scratch.
/// This join then checks row-level Source/root/request bindings and terminal
/// supersession.
/// Every Superseded edge resolves to a later Source view, so a finite settled
/// graph cannot cycle or end without a Published terminal.
fn join_ast_lineage<'a, E>(
    evidence: &'a NativeCutoverEvidenceV1,
    lineage: &NativeCutoverSourceLineageV1<'a, '_>,
    ranges: &[Range<usize>; 15],
    joined: &mut NativeCutoverJoinedSourceFactsV1<'a>,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let facts = &evidence.source_facts;
    let frontier_rows = mixed_slice(facts, ranges, 10)?;
    let work_rows = mixed_slice(facts, ranges, 11)?;
    if work_rows.is_empty() {
        if !frontier_rows.is_empty() {
            return Err(refuse("orphan AST frontier"));
        }
        return Ok(());
    }
    if frontier_rows.len() > 4096 {
        return Err(refuse("AST frontier census exceeds bound"));
    }
    let full = evidence.header.original_full_operation;
    let mut prior_scope: Option<Option<[u8; 32]>> = None;
    let mut global_next = None;
    for frontier in frontier_rows {
        work(charge, 64 + std::mem::size_of::<crate::index::OperationKey>())?;
        let Fact::AstFrontier { candidate_root_nonce, workspace, workspace_epoch,
            producer, stream, stream_epoch, next_operation_sequence,
            committed_event_sequence } = frontier else {
            return Err(refuse("AST frontier row differs"));
        };
        let key = candidate_root_nonce.map(|nonce| nonce.0);
        if key == Some([0; 32])
            || prior_scope.is_some_and(|prior| prior >= key)
            || *workspace != full.workspace || *workspace_epoch != full.epoch
            || *producer != NATIVE_STUB_AST_PRODUCER
            || *stream != crate::index::StreamId(*b"smca.ast.strm.v1")
            || *stream_epoch != 1 || *next_operation_sequence == 0
            || *committed_event_sequence >= *next_operation_sequence
            || global_next.is_some_and(|next| next != *next_operation_sequence) {
            return Err(refuse("AST root frontier or global allocator differs"));
        }
        global_next = Some(*next_operation_sequence);
        prior_scope = Some(key);
    }
    // At most 4096 frontier rows: one fixed 512-byte stack bitset records
    // which authentic root cursor was actually used by a work row.
    let mut seen_frontier = [0_u64; 64];
    let mut latest: Option<&'a Fact> = None;
    let mut latest_frontier: Option<&'a Fact> = None;
    let mut latest_view: Option<crate::index::ViewRef> = None;
    for fact in work_rows {
        work(charge, 1)?;
        let Fact::AstWork {
            aggregate_digest, source_operation, lexical_artifact_digest,
            candidate_root_nonce, source_view, revision, phase, ast_operation,
            ast_event_sequence, parser_registration, claim_owner_epoch,
            claim_fence, lease_expires_epoch_ms, refusal, published_view,
            superseded_by_aggregate_digest, request, ..
        } = fact else {
            return Err(refuse("AST work row differs"));
        };
        work(charge, aggregate_digest.len().saturating_add(128))?;
        if !canonical_hex_digest(aggregate_digest)
            || *revision == 0
            || lexical_artifact_digest.0 == [0; 32]
            || source_view.workspace != full.workspace
            || source_view.epoch != full.epoch
            || source_view.digest.0 == [0; 32]
            || *source_operation == full && source_view.sequence == 0
        {
            return Err(refuse("AST work identity or Source view differs"));
        }
        let mut low = 0;
        let mut high = frontier_rows.len();
        let wanted = candidate_root_nonce.map(|nonce| nonce.0);
        while low < high {
            work(charge, 32)?;
            let mid = low + (high - low) / 2;
            let Some(Fact::AstFrontier { candidate_root_nonce: candidate, .. }) = frontier_rows.get(mid)
            else { return Err(refuse("AST root frontier index absent")); };
            if candidate.map(|nonce| nonce.0) < wanted { low = mid + 1; } else { high = mid; }
        }
        let frontier = frontier_rows.get(low).ok_or_else(|| refuse("AST work root frontier absent"))?;
        let Fact::AstFrontier { candidate_root_nonce: frontier_scope,
            next_operation_sequence, producer, stream,
            stream_epoch, .. } = frontier else { return Err(refuse("AST frontier type differs")); };
        if frontier_scope != candidate_root_nonce {
            return Err(refuse("AST work root differs from its event frontier"));
        }
        seen_frontier[low / 64] |= 1_u64 << (low % 64);
        let replay = mixed_find(
            facts, mixed_range(ranges, 2)?, (aggregate_digest, 0, 0), charge,
        )?.ok_or_else(|| refuse("AST source V9 absent"))?;
        let Fact::Replay {
            native_update_request: Some(source_request),
            aggregate_identity_digest: replay_aggregate, ..
        } = replay else {
            return Err(refuse("AST source is not a Native V9"));
        };
        if !text_eq(aggregate_digest, replay_aggregate, charge)?
            || *source_operation != source_request.operation
        {
            return Err(refuse("AST source operation differs from Native V9"));
        }
        // Native reservations are ordered by event and strictly increasing
        // operation sequence. Search that already funded index, not the whole
        // signed Source census for each AST row.
        let mut low = 0;
        let mut high = lineage.len();
        while low < high {
            work(charge, std::mem::size_of::<crate::index::OperationKey>())?;
            let mid = low + (high - low) / 2;
            let (reservation, _) = lineage.at(mid)
                .ok_or_else(|| refuse("AST Source index absent"))?;
            let Fact::Reservation { target, .. } = reservation else {
                return Err(refuse("AST Source reservation absent"));
            };
            if target.operation < *source_operation { low = mid + 1; } else { high = mid; }
        }
        let (reservation, source_replay) = lineage.at(low)
            .ok_or_else(|| refuse("AST Source reservation absent"))?;
        let Fact::Reservation { target, candidate_root_nonce: source_scope, .. } = reservation else {
            return Err(refuse("AST Source reservation type differs"));
        };
        if source_scope != candidate_root_nonce
            || target.operation != *source_operation
            || !std::ptr::eq(source_replay, replay)
            || source_view.sequence <= target.expected_view.sequence
        {
            return Err(refuse("AST source does not join Native lineage"));
        }
        // AST lexical_artifact_digest names the Required Lexical member.
        // V9 prepared_artifact_digest names the aggregate artifact; these are
        // distinct domains. The signed S AST owner validated its member ref.
        match phase {
            NativeCutoverAstWorkPhaseV1::Published => {
                join_published_ast_for_source(target, *candidate_root_nonce,
                    replay, frontier, fact, false, charge)?;
                let view = published_view.ok_or_else(|| refuse("Published AST view absent"))?;
                if *candidate_root_nonce == joined.baseline_candidate_root_nonce
                    && latest_view.is_none_or(|prior| view.sequence > prior.sequence)
                {
                    latest_view = Some(view);
                    latest = Some(fact);
                    latest_frontier = Some(frontier);
                }
            }
            NativeCutoverAstWorkPhaseV1::Superseded => {
                if request.is_some() || published_view.is_some() || refusal.is_some()
                    || parser_registration.is_some() && ast_operation.is_none()
                    || ast_operation.is_some() != ast_event_sequence.is_some()
                    || ast_operation.is_some() != claim_owner_epoch.is_some()
                    || ast_operation.is_some() != claim_fence.is_some()
                    || ast_operation.is_some() != lease_expires_epoch_ms.is_some()
                {
                    return Err(refuse("Superseded AST phase columns differ"));
                }
                if let Some(operation) = ast_operation {
                    if operation.workspace != full.workspace
                        || operation.epoch != full.epoch
                        || operation.producer != *producer
                        || operation.stream != *stream
                        || operation.stream_epoch != *stream_epoch
                        || operation.sequence == 0
                        || operation.sequence >= *next_operation_sequence
                        || ast_event_sequence.is_none_or(|event| event == 0)
                        || claim_owner_epoch.is_none_or(|value| value == 0)
                        || claim_fence.is_none_or(|value| value == 0)
                    {
                        return Err(refuse("Superseded AST reservation differs"));
                    }
                }
                let successor_key = superseded_by_aggregate_digest.as_deref()
                    .ok_or_else(|| refuse("Superseded AST successor absent"))?;
                work(charge, successor_key.len().saturating_add(aggregate_digest.len()))?;
                if !canonical_hex_digest(successor_key) || successor_key == aggregate_digest {
                    return Err(refuse("Superseded AST successor key differs"));
                }
                let successor = mixed_find(
                    facts, mixed_range(ranges, 11)?, (successor_key, 0, 0), charge,
                )?.ok_or_else(|| refuse("Superseded AST successor row absent"))?;
                let Fact::AstWork {
                    source_operation: next_operation, source_view: next_view,
                    candidate_root_nonce: next_scope, ..
                } = successor else {
                    return Err(refuse("Superseded AST successor type differs"));
                };
                if next_scope != candidate_root_nonce
                    || next_operation.workspace != source_operation.workspace
                    || next_operation.epoch != source_operation.epoch
                    || next_operation.producer != source_operation.producer
                    || next_view.sequence <= source_view.sequence
                {
                    return Err(refuse("Superseded AST successor Source is not later"));
                }
            }
            NativeCutoverAstWorkPhaseV1::Queued
            | NativeCutoverAstWorkPhaseV1::Claimed
            | NativeCutoverAstWorkPhaseV1::Bound
            | NativeCutoverAstWorkPhaseV1::Refused => {
                return Err(refuse("active or refused AST work remains"));
            }
        }
    }
    for index in 0..frontier_rows.len() {
        work(charge, 1)?;
        if seen_frontier[index / 64] & (1_u64 << (index % 64)) == 0 {
            return Err(refuse("AST root frontier has no work"));
        }
    }
    // Historical roots can have settled AST work before the selected root
    // publishes any AST. Its physical BOOT is checked against its Source view
    // when no selected-root AST is present.
    let Some(latest) = latest else {
        return Ok(());
    };
    let Fact::AstWork { source_operation: latest_source, .. } = latest else {
        return Err(refuse("latest AST Published row type differs"));
    };
    if *latest_source != joined.current_native_target.operation {
        return Err(refuse("latest AST does not belong to current Native Source"));
    }
    joined.ast_frontier = latest_frontier;
    joined.ast_published_work = Some(latest);
    Ok(())
}

fn join_published_ast<E>(
    joined: &NativeCutoverJoinedSourceFactsV1<'_>,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    match (joined.ast_frontier, joined.ast_published_work) {
        (None, None) => Ok(()),
        (Some(frontier), Some(work_fact)) => join_published_ast_for_source(
            joined.current_native_target,
            joined.baseline_candidate_root_nonce,
            joined.current_native_replay.ok_or_else(|| refuse("Published AST parent V9 absent"))?,
            frontier, work_fact, true, charge,
        ),
        _ => Err(refuse("orphan AST frontier or Published work")),
    }
}

fn join_published_ast_for_source<E>(
    source_target: &NativeCutoverEvidenceTargetV1,
    source_scope: Option<crate::index::Digest>,
    source_replay: &Fact,
    frontier: &Fact,
    work_fact: &Fact,
    strict_frontier: bool,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let (frontier, work_fact) = match (frontier, work_fact) {
        (
            Fact::AstFrontier {
                candidate_root_nonce: frontier_scope,
                workspace,
                workspace_epoch,
                producer,
                stream,
                stream_epoch,
                next_operation_sequence,
                committed_event_sequence,
                ..
            },
            Fact::AstWork {
                candidate_root_nonce: work_scope,
                aggregate_digest,
                source_operation,
                lexical_artifact_digest,
                source_view,
                source_cut,
                revision,
                phase,
                parser_registration,
                ast_operation,
                ast_event_sequence,
                claim_owner_epoch,
                claim_fence,
                lease_expires_epoch_ms,
                refusal,
                published_view,
                superseded_by_aggregate_digest,
                request,
                ..
            },
        ) => (
            (
                frontier_scope,
                workspace,
                workspace_epoch,
                producer,
                stream,
                stream_epoch,
                next_operation_sequence,
                committed_event_sequence,
            ),
            (
                work_scope,
                aggregate_digest,
                source_operation,
                lexical_artifact_digest,
                source_view,
                source_cut,
                revision,
                phase,
                parser_registration,
                ast_operation,
                ast_event_sequence,
                claim_owner_epoch,
                claim_fence,
                lease_expires_epoch_ms,
                refusal,
                published_view,
                superseded_by_aggregate_digest,
                request,
            ),
        ),
        _ => return Err(refuse("orphan AST frontier or Published work")),
    };
    let (
        frontier_scope,
        workspace,
        workspace_epoch,
        producer,
        stream,
        stream_epoch,
        next_operation_sequence,
        committed_event_sequence,
    ) = frontier;
    let (
        work_scope,
        aggregate_digest,
        source_operation,
        lexical_artifact_digest,
        source_view,
        source_cut,
        revision,
        phase,
        parser_registration,
        ast_operation,
        ast_event_sequence,
        claim_owner_epoch,
        claim_fence,
        lease_expires_epoch_ms,
        refusal,
        published_view,
        superseded_by_aggregate_digest,
        request,
    ) = work_fact;
    let Fact::Replay {
        aggregate_identity_digest,
        ..
    } = source_replay
    else {
        return Err(refuse("Published AST lacks V9 parent"));
    };
    if !text_eq(aggregate_digest, aggregate_identity_digest, charge)? {
        return Err(refuse("AST aggregate differs from V9"));
    }
    work(charge, 256)?;
    let full = source_target.operation;
    let Some(operation) = ast_operation else {
        return Err(refuse("Published AST has no reserved operation"));
    };
    let Some(event_sequence) = ast_event_sequence else {
        return Err(refuse("Published AST has no event sequence"));
    };
    let Some(parser) = parser_registration else {
        return Err(refuse("Published AST has no parser registration"));
    };
    let Some(view) = published_view else {
        return Err(refuse("Published AST has no terminal view"));
    };
    let Some(request) = request else {
        return Err(refuse("Published AST has no exact bound request"));
    };
    if *frontier_scope != source_scope || *work_scope != source_scope
        || *source_operation != full
        || *phase != NativeCutoverAstWorkPhaseV1::Published
        || *workspace != full.workspace
        || *workspace_epoch != full.epoch
        || *producer != NATIVE_STUB_AST_PRODUCER
        || *stream != crate::index::StreamId(*b"smca.ast.strm.v1")
        || *stream_epoch != 1
        || *revision == 0
        || lexical_artifact_digest.0 == [0; 32]
        || parser.0 == [0; 32]
        || source_view.workspace != full.workspace
        || source_view.epoch != full.epoch
        || source_view.sequence <= source_target.expected_view.sequence
        || source_view.digest.0 == [0; 32]
        || operation.workspace != full.workspace
        || operation.epoch != full.epoch
        || operation.producer != *producer
        || operation.stream != *stream
        || operation.stream_epoch != *stream_epoch
        || operation.sequence == 0
        || operation.sequence >= *next_operation_sequence
        || *event_sequence == 0
        || *event_sequence > *committed_event_sequence
        || (strict_frontier && *committed_event_sequence != *event_sequence)
        || claim_owner_epoch.is_none_or(|value| value == 0)
        || claim_fence.is_none_or(|value| value == 0)
        || lease_expires_epoch_ms.is_none()
        || refusal.is_some()
        || superseded_by_aggregate_digest.is_some()
        || request.operation != *operation
        || request.expected_view.workspace != full.workspace
        || request.expected_view.epoch != full.epoch
        || request.expected_view.digest.0 == [0; 32]
        || request.expected_view.sequence < source_view.sequence
        || (request.expected_view.sequence == source_view.sequence
            && request.expected_view.digest != source_view.digest)
        || view.workspace != full.workspace
        || view.epoch != full.epoch
        || view.digest.0 == [0; 32]
        || view.sequence <= request.expected_view.sequence
    {
        return Err(refuse(
            "Published AST does not join exact Source and frontier",
        ));
    }
    let UpdateKind::ProjectionAdvance(transition) = &request.kind else {
        return Err(refuse("Published AST request is not projection advance"));
    };
    if transition.projection != NATIVE_STUB_AST_PROJECTION
        || transition.producer != NATIVE_STUB_AST_PRODUCER
        || transition.schema != NATIVE_STUB_AST_SCHEMA
        || transition.producer_policy != native_stub_certificate_policy()
        || transition.dependency_context != *parser
        || transition.target_source != *source_cut
        || transition.events.len() != 1
    {
        return Err(refuse("Published AST projection contract differs"));
    }
    let event = transition
        .events
        .first()
        .ok_or_else(|| refuse("Published AST event absent"))?;
    if event.producer != operation.producer
        || event.stream != operation.stream
        || event.stream_epoch != operation.stream_epoch
        || event.sequence != *event_sequence
        || event.predecessor.checked_add(1) != Some(event.sequence)
    {
        return Err(refuse("Published AST reserved event differs"));
    }
    let payload = transition
        .payload_digest_with_work_v3(&mut |bytes| work(charge, bytes))
        .map_err(|cause| match cause {
            crate::index::ProjectionPayloadDigestErrorV3::Work(cause) => cause,
            crate::index::ProjectionPayloadDigestErrorV3::Arithmetic
            | crate::index::ProjectionPayloadDigestErrorV3::Encode(_) => {
                refuse("AST canonical payload digest could not be encoded")
            }
        })?;
    if event.payload != payload {
        return Err(refuse("AST event payload digest differs"));
    }
    Ok(())
}

fn sa_payload_to_hex(payload: &crate::index::Digest) -> [u8; 64] {
    fn nibble_hex(nibble: u8) -> u8 {
        if nibble < 10 {
            b'0'.wrapping_add(nibble)
        } else {
            b'a'.wrapping_add(nibble.wrapping_sub(10))
        }
    }

    let mut output = [0_u8; 64];
    for (byte, pair) in payload.0.iter().zip(output.chunks_exact_mut(2)) {
        if let [high, low] = pair {
            *high = nibble_hex(*byte >> 4);
            *low = nibble_hex(*byte & 15);
        }
    }
    output
}

fn sdk_binding_text_bytes(binding: &NativeCutoverEvidenceSdkBindingV1) -> usize {
    [
        &binding.source_intent_digest,
        &binding.completion_pointer_digest,
        &binding.transition_digest,
        &binding.target_source_digest,
        &binding.ingress_kind,
        &binding.workspace_identity_digest,
        &binding.request_fingerprint_digest,
    ]
    .iter()
    .fold(
        binding
            .client_idempotency_digest
            .as_ref()
            .map_or(0, String::len),
        |total, value| total.saturating_add(value.len()),
    )
}

fn checkpoint_ack_text_bytes(ack: &NativeCutoverEvidenceCheckpointAckV1) -> usize {
    ack.checkpoint_id
        .len()
        .saturating_add(ack.terminal_aggregate_receipt_digest.len())
        .saturating_add(ack.checkpoint_receipt_digest.len())
        .saturating_add(ack.canonical_checkpoint_ack_digest.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{
        CollectionId, Digest, MapRoot, ObjectRef, OwnerMutationSet, ProducerCompleteness,
        ProducerId, ProjectionTransition, SlotId, SourceCut, SourceEvent, SourceOperationMode,
        SourceTextAdmission, SourceTransition, StreamId, TenantId, UpdateKind, ViewRef,
        WorkspaceKey, WorktreeId,
    };
    use crate::native_cutover_evidence_v1::NativeCutoverEvidenceLegacyPublicationV1;
    use crate::native_cutover_evidence_v1::{
        NativeCutoverByteStringV1, NativeCutoverEvidenceBeforeSelectionV1,
        NativeCutoverEvidenceCheckpointAckV1, NativeCutoverEvidenceFileIdentityV1,
        NativeCutoverEvidenceHeaderV1, NativeCutoverEvidenceHostFactsV1,
        NativeCutoverEvidenceHostSealV1, NativeCutoverEvidenceOutboxDeliveryReceiptV1,
        NativeCutoverEvidenceOutboxPublishBindingV1,
        NativeCutoverEvidenceOutboxSemanticAdmissionMetricsV1,
        NativeCutoverEvidenceSelectedSourceV1,
        NativeCutoverHostCustodyModeV1, NativeCutoverHostWatchModeV1,
    };
    use crate::{
        BatchPublishReceipt, GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
        SearchCorpusActivationTokenV1, SearchCorpusActiveHeadV1, SearchCorpusGenerationIdentityV1,
        SearchPlaneSearchCorpusActivationCasAck, SearchPlaneTrackKind, SemanticContentRootsV1,
    };
    use sha2::Digest as _;
    use std::num::NonZeroU64;


    fn join_test<'a, E>(
        evidence: &'a NativeCutoverEvidenceV1,
        charge: &mut impl FnMut(u64) -> Result<(), E>,
    ) -> Result<NativeCutoverJoinedSourceFactsV1<'a>, NativeCutoverSourceJoinErrorV1<E>> {
        let mut reservations = Vec::with_capacity(evidence.source_facts.len());
        let mut replays = Vec::with_capacity(evidence.source_facts.len());
        join_native_cutover_source_facts_v1(evidence, &mut reservations, &mut replays, charge)
    }

    fn fixed_evidence() -> NativeCutoverEvidenceV1 {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let operation = crate::index::OperationKey {
            workspace,
            epoch: 1,
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            sequence: 7,
        };
        let view = ViewRef {
            workspace,
            epoch: 1,
            sequence: 1,
            digest: Digest([7; 32]),
        };
        let source = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let baseline = NativeCutoverEvidenceTargetV1 {
            operation,
            expected_view: view,
            expected_source: source,
            expected_profile: ObjectRef(Digest([8; 32])),
            source_mode: SourceOperationMode::Advance,
            event_predecessor: 0,
            event_sequence: 1,
            owner_identity_policy_digest: Digest([9; 32]),
            text_admission: SourceTextAdmission::Excluded,
            canonical_target_digest: Digest([10; 32]),
        };
        let file = NativeCutoverEvidenceFileIdentityV1 {
            device: 1,
            inode: 2,
        };
        let ack = NativeCutoverEvidenceCheckpointAckV1 {
            format_version: 2,
            checkpoint_id: "checkpoint".into(),
            checkpoint_sequence: 1,
            terminal_aggregate_receipt_digest: "4".repeat(64),
            checkpoint_receipt_digest: "5".repeat(64),
            canonical_checkpoint_ack_digest: "6".repeat(64),
        };
        let digest = NativeCutoverDigest64V1([b'1'; 64]);
        let request = UpdateRequest {
            operation,
            expected_view: view,
            kind: UpdateKind::SourceAdvance(SourceTransition {
                expected_source: source,
                owners: OwnerMutationSet {
                    root: MapRoot(None),
                    count: 0,
                    digest: Digest([11; 32]),
                },
                events: Vec::new(),
            }),
        };
        NativeCutoverEvidenceV1 {
            header: NativeCutoverEvidenceHeaderV1 {
                format_version: NativeCutoverEvidenceV1::FORMAT_VERSION_V1,
                canonical_workspace_root: "/repo".into(),
                repo_id: "repo".into(),
                contract_key: "contract".into(),
                writer_state_root: "/state".into(),
                canonical_snapshot_namespace: "/snapshots".into(),
                sdk_lock_path: "/lock".into(),
                config_path: "/config".into(),
                config_sha256: Digest([12; 32]),
                original_full_operation: operation,
                selected_source: NativeCutoverEvidenceSelectedSourceV1 {
                    workspace,
                    workspace_epoch: 1,
                    producer: operation.producer,
                    stream: operation.stream,
                    stream_epoch: 1,
                    text_admission: SourceTextAdmission::Excluded,
                },
                host: NativeCutoverEvidenceHostSealV1 {
                    canonical_workspace_root: "/repo".into(),
                    host_instance: 1,
                    host_generation: 1,
                    watch_mode: NativeCutoverHostWatchModeV1::Polling,
                    custody_mode: NativeCutoverHostCustodyModeV1::DurableInbox,
                    namespace_root: "/namespace".into(),
                    namespace_file: file,
                },
                sdk_lock_file: file,
                current_selection_sha256: Digest([13; 32]),
                candidate_selection_sha256: Digest([14; 32]),
            },
            baseline,
            previous_native_source_coverage: None,
            before_selection: NativeCutoverEvidenceBeforeSelectionV1 {
                document: NativeCutoverByteStringV1(b"{}".to_vec()),
                raw_sha256: Digest(sha2::Sha256::digest(b"{}").into()),
                file_version: Some(Digest([15; 32])),
            },
            fresh_sdk: NativeCutoverEvidenceFreshSdkV1 {
                target_snapshot_id: 17,
                committed_source_snapshot_id: 17,
                committed_source_snapshot_digest: NativeCutoverDigest64V1([b'f'; 64]),
                sdk_generation_digest: NativeCutoverDigest64V1([b'c'; 64]),
                source_pointer_digest: NativeCutoverDigest64V1([b'd'; 64]),
                source_manifest_digest: NativeCutoverDigest64V1([b'e'; 64]),
                physical_target_source_digest: NativeCutoverDigest64V1([b'f'; 64]),
            },
            v5_artifacts: vec![NativeCutoverEvidenceV5ArtifactV1 {
                operation,
                published_snapshot_id: 17,
                lexical_revision_digest: NativeCutoverDigest64V1([b'c'; 64]),
                prepared_artifact_digest: NativeCutoverDigest64V1([b'a'; 64]),
                aggregate_receipt_digest: NativeCutoverDigest64V1([b'b'; 64]),
                previous_transition_digest: None,
                base_source_digest: None,
                repomap: crate::native_cutover_evidence_v1::NativeCutoverEvidenceLegacyRepoMapV1::NotApplicable {
                    reason_code: "test-no-repomap".to_string(),
                    policy_digest: NativeCutoverDigest64V1([b'a'; 64]),
                    product_scope_digest: NativeCutoverDigest64V1([b'b'; 64]),
                },
            }],
            legacy_publication: None,
            retained_terminal: None,
            source_facts: vec![
                Fact::Allocator {
                    next_operation_sequence: 8,
                },
                Fact::Reservation {
                    target: baseline,
                    evidence: NativeCutoverReservationEvidenceV1::Initial,
                    candidate_root_nonce: Some(Digest([22; 32])),
                },
                Fact::Replay {
                    workspace_root: "/repo".into(),
                    contract_key: "contract".into(),
                    aggregate_identity_digest: "1".repeat(64),
                    closeout_id: "change-event:plan".into(),
                    attempt_id: "change-event:plan:attempt-1".into(),
                    format_version: 3,
                    state_revision: 5,
                    phase: NativeCutoverReplayPhaseV1::Acked,
                    native_update_request: Some(request),
                    published_snapshot_id: Some(17),
                    prepared_artifact_digest: Some(NativeCutoverDigest64V1([b'a'; 64])),
                    source_bound_aggregate_receipt_digest: Some(NativeCutoverDigest64V1(
                        [b'b'; 64],
                    )),
                    terminal_aggregate_receipt_digest: Some("4".repeat(64)),
                    terminal_projection_receipt_digest: Some("2".repeat(64)),
                    checkpoint_ack: Some(ack.clone()),
                    created_at_epoch_ms: 1,
                    updated_at_epoch_ms: 5,
                    ordinary_source_binding: None,
                },
                Fact::PendingReplay {
                    workspace_root: "/repo".into(),
                    contract_key: "contract".into(),
                    aggregate_identity_digest: "1".repeat(64),
                    plan_id: "plan".into(),
                    attempt_id: 1,
                    owner_token: 10,
                    workspace_identity_digest: digest,
                    runtime_generation: 1,
                    owner_fence_digest: "7".repeat(64),
                    custody_revision: 1,
                },
                Fact::PendingStoreAttempt {
                    plan_id: "plan".into(),
                    attempt_id: 1,
                    state: "TerminalCommitted".into(),
                    execution_owner_token: None,
                    fence: NativeCutoverAttemptFenceWireV1::BoundV2 {
                        immutable_owner_token: 10,
                        workspace_identity_digest: digest,
                        runtime_generation: 1,
                    },
                    started_at_epoch_ms: 1,
                    terminal_at_epoch_ms: Some(4),
                    updated_at_epoch_ms: 4,
                },
                Fact::PendingStoreCustody {
                    plan_id: "plan".into(),
                    attempt_id: 1,
                    custody_revision: 1,
                    aggregate_identity_digest: digest,
                    predecessor_owner_token: 10,
                    predecessor_workspace_identity_digest: digest,
                    predecessor_runtime_generation: 1,
                    owner_token: 12,
                    owner_workspace_identity_digest: NativeCutoverDigest64V1([b'8'; 64]),
                    owner_runtime_generation: 2,
                    claimed_at_epoch_ms: 3,
                },
                Fact::PendingStoreTerminal {
                    plan_id: "plan".into(),
                    attempt_id: 1,
                    commit_owner_token: 12,
                    canonical_receipt_digest: "2".repeat(64),
                    publish_store_generation: 2,
                    published_snapshot_id: 17,
                    checkpoint_generation: Some(9),
                    checkpoint_payload_digest: Some(NativeCutoverDigest64V1([b'3'; 64])),
                    checkpoint_acked_at_epoch_ms: Some(5),
                    cleanup_completed_at_epoch_ms: Some(6),
                    created_at_epoch_ms: 4,
                },
                Fact::PendingStoreCheckpoint {
                    workspace_root: "/repo".into(),
                    contract_key: "contract".into(),
                    aggregate_identity_digest: "1".repeat(64),
                    terminal_state_revision: 4,
                    terminal_aggregate_receipt_digest: "4".repeat(64),
                    ack,
                    created_at_epoch_ms: 5,
                },
            ],
            host_facts: NativeCutoverEvidenceHostFactsV1 {
                workspace_authority: "/repo".into(),
                capacity: 1,
                retained_delivery_count: 0,
                active_units: 0,
                prepared_authorities: Vec::new(),
                deliveries: Vec::new(),
                pending_apply_intent: None,
                recovery_receipts: Vec::new(),
            },
        }
    }

    #[test]
    fn canonical_attempt_preserves_decimal_u32_bounds() {
        assert_eq!(canonical_attempt("0"), Some(0));
        assert_eq!(canonical_attempt("4294967295"), Some(u32::MAX));
        for invalid in [
            "",
            "00",
            "01",
            "+1",
            "-1",
            " 1",
            "1 ",
            "4294967296",
            "99999999999999999999999999999999",
            "１２",
        ] {
            assert_eq!(canonical_attempt(invalid), None);
        }
    }

    /// Isolate the SDK first-FULL V9 shape. The actual installed product may
    /// also retain an independent Legacy V9/SDK M; that mixed owner join is a
    /// separate required contract and this fixture does not claim it.
    fn native_only_initial_full_evidence() -> NativeCutoverEvidenceV1 {
        let mut evidence = fixed_evidence();
        let target_hex = sa_payload_to_hex(&evidence.baseline.canonical_target_digest);
        let target_hex = std::str::from_utf8(&target_hex).expect("fixed lowercase hex");
        evidence.source_facts.retain(|fact| {
            matches!(
                fact,
                Fact::Allocator { .. }
                    | Fact::Reservation { .. }
                    | Fact::Replay { .. }
                    | Fact::PendingStoreCheckpoint { .. }
            )
        });
        let replay = evidence
            .source_facts
            .iter_mut()
            .find(|fact| matches!(fact, Fact::Replay { .. }))
            .expect("fixed V9");
        if let Fact::Replay {
            closeout_id,
            attempt_id,
            ..
        } = replay
        {
            *closeout_id = "initial-native-full:".to_owned() + &"c".repeat(64);
            *attempt_id = format!("source-target:{target_hex}");
        }
        evidence
    }

    // Association fixture only. Pending/ACK, actual Published Q metadata and
    // Source M ownership are validated by separate consumers before cutover.
    fn native_lineage_evidence(successors: u64) -> NativeCutoverEvidenceV1 {
        let mut evidence = native_only_initial_full_evidence();
        let mut prior = evidence.baseline;
        for step in 1..=successors {
            let mut target = prior;
            target.operation.sequence = 7 + step * 4;
            target.event_predecessor = prior.event_sequence;
            target.event_sequence = prior.event_sequence + 1;
            target.canonical_target_digest = Digest([u8::try_from(10 + step).unwrap(); 32]);
            evidence.source_facts.push(Fact::Reservation {
                target,
                evidence: NativeCutoverReservationEvidenceV1::Sdk {
                    identity_kind: "Keyless".into(),
                    attempt_identity_digest: NativeCutoverDigest64V1([b'a'; 64]),
                    routing_identity_digest: NativeCutoverDigest64V1([b'b'; 64]),
                    prior_target_digest: prior.canonical_target_digest,
                },
                candidate_root_nonce: Some(Digest([22; 32])),
            });
            let mut replay = evidence.source_facts.iter().find_map(|fact| match fact {
                Fact::Replay { native_update_request: Some(request), .. }
                    if request.operation == evidence.baseline.operation => Some(fact.clone()),
                _ => None,
            }).expect("initial Native V9 fixture");
            if let Fact::Replay { native_update_request: Some(request), closeout_id, attempt_id, .. } = &mut replay {
                request.operation = target.operation;
                request.expected_view = target.expected_view;
                *closeout_id = format!("change-event:plan-{step}");
                *attempt_id = format!("change-event:plan-{step}:attempt-1");
            }
            evidence.source_facts.push(replay);
            prior = target;
        }
        evidence.v5_artifacts[0].operation = prior.operation;
        if successors != 0 {
            evidence.v5_artifacts[0].previous_transition_digest = Some(
                NativeCutoverDigest64V1([b'a'; 64]));
            evidence.v5_artifacts[0].base_source_digest = Some(
                NativeCutoverDigest64V1([b'b'; 64]));
        }
        if let Some(Fact::Allocator { next_operation_sequence }) = evidence.source_facts
            .iter_mut().find(|fact| matches!(fact, Fact::Allocator { .. })) {
            *next_operation_sequence = prior.operation.sequence + 1;
        }
        evidence
    }

    fn associate_lineage(evidence: &NativeCutoverEvidenceV1)
        -> Result<Vec<(u64, u64)>, NativeCutoverSourceJoinErrorV1<&'static str>> {
        let mut reservations = Vec::with_capacity(evidence.source_facts.len());
        let mut replays = Vec::with_capacity(evidence.source_facts.len());
        let lineage = fold_native_source_lineage_v1(
            evidence, &mut reservations, &mut replays, &mut |_| Ok::<_, &'static str>(()),
        )?;
        (0..lineage.len()).map(|index| {
            let Some((Fact::Reservation { target, .. }, Fact::Replay {
                native_update_request: Some(request), ..
            })) = lineage.at(index) else { return Err(refuse("lineage pair absent")); };
            Ok((target.operation.sequence, request.operation.sequence))
        }).collect()
    }

    #[test]
    fn native_association_accepts_two_and_three_events_with_operation_gaps() {
        assert_eq!(associate_lineage(&native_lineage_evidence(1)).unwrap(),
            vec![(7, 7), (11, 11)]);
        assert_eq!(associate_lineage(&native_lineage_evidence(2)).unwrap(),
            vec![(7, 7), (11, 11), (15, 15)]);
    }

    fn two_candidate_roots() -> NativeCutoverEvidenceV1 {
        let mut evidence = native_only_initial_full_evidence();
        let old_target = evidence.baseline;
        let mut old_replay = evidence.source_facts.iter().find_map(|fact| match fact {
            Fact::Replay { native_update_request: Some(_), .. } => Some(fact.clone()),
            _ => None,
        }).expect("initial Native replay");
        let mut old_checkpoint = evidence.source_facts.iter().find_map(|fact| match fact {
            Fact::PendingStoreCheckpoint { .. } => Some(fact.clone()),
            _ => None,
        }).expect("initial Native checkpoint");
        if let Fact::Replay { aggregate_identity_digest, .. } = &mut old_replay {
            *aggregate_identity_digest = "2".repeat(64);
        }
        if let Fact::PendingStoreCheckpoint { aggregate_identity_digest, .. } = &mut old_checkpoint {
            *aggregate_identity_digest = "2".repeat(64);
        }
        evidence.baseline.operation.sequence = 41;
        evidence.header.original_full_operation = evidence.baseline.operation;
        evidence.v5_artifacts[0].operation = evidence.baseline.operation;
        for fact in &mut evidence.source_facts {
            match fact {
                Fact::Allocator { next_operation_sequence } => *next_operation_sequence = 42,
                Fact::Reservation { target, .. } => *target = evidence.baseline,
                Fact::Replay { native_update_request: Some(request), .. } => {
                    request.operation = evidence.baseline.operation;
                }
                _ => {}
            }
        }
        evidence.source_facts.push(Fact::Reservation {
            target: old_target,
            evidence: NativeCutoverReservationEvidenceV1::Initial,
            candidate_root_nonce: Some(Digest([33; 32])),
        });
        evidence.source_facts.push(old_replay);
        evidence.source_facts.push(old_checkpoint);
        evidence.source_facts.sort_by(|left, right| mixed_rank(left).cmp(&mixed_rank(right))
            .then_with(|| mixed_key(left).cmp(&mixed_key(right))));
        evidence
    }

    #[test]
    fn native_association_accepts_two_initial_roots_with_global_operation_gaps() {
        let evidence = two_candidate_roots();
        assert_eq!(associate_lineage(&evidence).unwrap(), vec![(7, 7), (41, 41)]);
    }

    #[test]
    fn native_join_keeps_selected_initial_full_separate_from_historical_root() {
        let evidence = two_candidate_roots();
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("both settled initial FULL roots");
        assert_eq!(joined.baseline.operation.sequence, 41);
        assert_eq!(joined.current_native_target.operation.sequence, 41);
        assert_eq!(joined.baseline_candidate_root_nonce, Some(Digest([22; 32])));
        assert!(joined.sdk_acked_binding.is_none());
    }

    #[test]
    fn native_join_allows_later_operations_in_another_root() {
        let mut evidence = two_candidate_roots();
        let initial = native_only_initial_full_evidence();
        evidence.baseline = initial.baseline;
        evidence.header.original_full_operation = initial.baseline.operation;
        evidence.v5_artifacts = initial.v5_artifacts;

        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("another root cannot advance the selected root's Source cursor");
        assert_eq!(joined.current_native_target.operation.sequence, 7);
        assert_eq!(joined.baseline_candidate_root_nonce, Some(Digest([33; 32])));
    }

    #[test]
    fn native_association_keeps_historical_unknown_root_but_refuses_selected_unknown() {
        let mut evidence = two_candidate_roots();
        for fact in &mut evidence.source_facts {
            if let Fact::Reservation { target, candidate_root_nonce, .. } = fact
                && target.operation.sequence == 7 {
                *candidate_root_nonce = None;
            }
        }
        assert_eq!(associate_lineage(&evidence).unwrap(), vec![(7, 7), (41, 41)]);
        for fact in &mut evidence.source_facts {
            if let Fact::Reservation { target, candidate_root_nonce, .. } = fact
                && target.operation.sequence == 41 {
                *candidate_root_nonce = None;
            }
        }
        assert!(matches!(associate_lineage(&evidence),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));
    }

    #[test]
    fn native_association_refuses_duplicate_operation_scope_and_event_gap() {
        let evidence = two_candidate_roots();
        let mut duplicate = evidence.clone();
        // Duplicate an entire old root pair: the global OperationKey cannot repeat.
        let old_reservation = evidence.source_facts.iter().find(|fact| matches!(fact,
            Fact::Reservation { target, .. } if target.operation.sequence == 7)).unwrap().clone();
        let old_replay = evidence.source_facts.iter().find(|fact| matches!(fact,
            Fact::Replay { native_update_request: Some(request), .. }
                if request.operation.sequence == 7)).unwrap().clone();
        duplicate.source_facts.extend([old_reservation, old_replay]);
        assert!(matches!(associate_lineage(&duplicate),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));

        let mut wrong_scope = evidence.clone();
        for fact in &mut wrong_scope.source_facts {
            if let Fact::Reservation { target, candidate_root_nonce, .. } = fact
                && target.operation.sequence == 7 {
                *candidate_root_nonce = Some(Digest([22; 32]));
            }
        }
        assert!(matches!(associate_lineage(&wrong_scope),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));

        let mut gap = evidence;
        for fact in &mut gap.source_facts {
            if let Fact::Reservation { target, .. } = fact
                && target.operation.sequence == 7 {
                target.event_sequence = 2;
            }
        }
        assert!(matches!(associate_lineage(&gap),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));
    }

    #[test]
    fn native_association_refuses_foreign_prior_gap_orphan_and_work() {
        let mut evidence = native_lineage_evidence(2);
        for fact in &mut evidence.source_facts {
            if let Fact::Reservation { target, evidence: NativeCutoverReservationEvidenceV1::Sdk {
                prior_target_digest, ..
            }, .. } = fact && target.event_sequence == 3 {
                *prior_target_digest = Digest([0; 32]);
            }
        }
        assert!(matches!(associate_lineage(&evidence),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));

        let mut evidence = native_lineage_evidence(2);
        for fact in &mut evidence.source_facts {
            if let Fact::Reservation { target, .. } = fact && target.event_sequence == 3 {
                target.event_predecessor = 0;
            }
        }
        assert!(matches!(associate_lineage(&evidence),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));

        let mut evidence = native_lineage_evidence(2);
        let orphan = evidence.source_facts.iter().find(|fact| matches!(fact,
            Fact::Replay { native_update_request: Some(_), .. })).unwrap().clone();
        evidence.source_facts.push(orphan);
        assert!(matches!(associate_lineage(&evidence),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));

        let mut evidence = native_lineage_evidence(2);
        evidence.v5_artifacts[0].operation = evidence.baseline.operation;
        assert!(matches!(associate_lineage(&evidence),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));

        let evidence = native_lineage_evidence(2);
        let mut unfunded_reservations = Vec::new();
        let mut unfunded_replays = Vec::new();
        assert!(matches!(fold_native_source_lineage_v1(&evidence,
            &mut unfunded_reservations, &mut unfunded_replays,
            &mut |_| Ok::<_, &'static str>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));

        let evidence = native_lineage_evidence(2);
        let mut reservations = Vec::with_capacity(evidence.source_facts.len());
        let mut replays = Vec::with_capacity(evidence.source_facts.len());
        assert!(matches!(fold_native_source_lineage_v1(&evidence, &mut reservations,
            &mut replays, &mut |_| Err::<(), _>("work refused")),
            Err(NativeCutoverSourceJoinErrorV1::Work("work refused"))));
    }

    #[test]
    fn native_only_initial_full_admits_exact_v9_and_global_checkpoint_without_pending() {
        let evidence = native_only_initial_full_evidence();
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("isolated Native first FULL V9 form");
        assert_eq!(
            joined.native_update_request_v1().unwrap().operation,
            evidence.header.original_full_operation
        );
        assert!(joined.pending_carrier.is_none());
        assert!(joined.pending_attempt.is_none());
        assert!(joined.custody_head.is_none());
        assert!(joined.terminal_receipt.is_none());
        assert!(joined.checkpoint_ack.is_some());
    }

    #[test]
    fn mixed_legacy_and_native_v9_refuses_until_independent_owner_join() {
        let mut evidence = native_only_initial_full_evidence();
        let legacy_v9 = evidence
            .source_facts
            .iter()
            .find(|fact| matches!(fact, Fact::Replay { .. }))
            .expect("fixed Native V9")
            .clone();
        let Fact::Replay {
            native_update_request: _,
            ..
        } = legacy_v9
        else {
            unreachable!();
        };
        let mut legacy_v9 = legacy_v9;
        if let Fact::Replay {
            native_update_request,
            closeout_id,
            attempt_id,
            ..
        } = &mut legacy_v9
        {
            *native_update_request = None;
            *closeout_id = "legacy-sdk-closeout".into();
            *attempt_id = "legacy-sdk-attempt".into();
        }
        evidence.source_facts.push(legacy_v9);
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))
        ));
    }

    #[test]
    fn native_only_initial_full_refuses_missing_or_foreign_producer_and_pending_state() {
        let refuses = |evidence: &NativeCutoverEvidenceV1| {
            assert!(matches!(
                join_test(evidence, &mut |_| Ok::<_, ()>(())),
                Err(NativeCutoverSourceJoinErrorV1::Refused(_))
            ));
        };
        let mut evidence = native_only_initial_full_evidence();
        evidence
            .source_facts
            .retain(|fact| !matches!(fact, Fact::PendingStoreCheckpoint { .. }));
        refuses(&evidence);

        let mut evidence = native_only_initial_full_evidence();
        if let Some(Fact::Replay { closeout_id, .. }) = evidence
            .source_facts
            .iter_mut()
            .find(|fact| matches!(fact, Fact::Replay { .. }))
        {
            *closeout_id = "initial-native-full:".to_owned() + &"d".repeat(64);
        }
        refuses(&evidence);

        let mut evidence = native_only_initial_full_evidence();
        if let Some(Fact::Replay { attempt_id, .. }) = evidence
            .source_facts
            .iter_mut()
            .find(|fact| matches!(fact, Fact::Replay { .. }))
        {
            *attempt_id = "source-target:".to_owned() + &"e".repeat(64);
        }
        refuses(&evidence);

        let mut evidence = native_only_initial_full_evidence();
        if let Some(Fact::Replay { phase, .. }) = evidence
            .source_facts
            .iter_mut()
            .find(|fact| matches!(fact, Fact::Replay { .. }))
        {
            *phase = NativeCutoverReplayPhaseV1::Terminal;
        }
        refuses(&evidence);

        let mut evidence = native_only_initial_full_evidence();
        let pending = fixed_evidence()
            .source_facts
            .into_iter()
            .find(|fact| matches!(fact, Fact::PendingReplay { .. }))
            .expect("fixed pending carrier");
        evidence.source_facts.push(pending);
        refuses(&evidence);
    }

    #[test]
    fn normal_full_retains_v9_carrier_custody_terminal_and_ack() {
        let evidence = fixed_evidence();
        let mut charged = 0;
        let joined = join_test(&evidence, &mut |units| {
            charged += units;
            Ok::<_, ()>(())
        })
        .expect("fixed retained FULL history");
        assert_eq!(
            joined.native_update_request_v1().unwrap().operation,
            evidence.header.original_full_operation
        );
        assert!(joined.custody_head.is_some());
        assert_eq!(
            joined.fresh_sdk.committed_source_snapshot_id,
            joined.v5_artifact.published_snapshot_id
        );
        assert_eq!(
            joined.v5_artifact.operation,
            evidence.header.original_full_operation
        );
        assert_ne!(
            joined.v5_artifact.aggregate_receipt_digest.0, [b'4'; 64],
            "V5 source-bound aggregate receipt has a different domain from V9 terminal receipt"
        );
        assert!(charged > evidence.source_facts.len() as u64);
    }

    #[test]
    fn history_scan_charges_each_source_fact_regardless_of_variant() {
        let evidence = fixed_evidence();
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("fixed retained FULL history");
        let first_custody = evidence.source_facts.iter().find(|fact| {
            matches!(
                fact,
                Fact::PendingStoreCustody {
                    custody_revision: 1,
                    ..
                }
            )
        });
        assert!(first_custody.is_some());
        let mut scanned = 0;
        join_history(&joined, first_custody, &evidence, &mut |units| {
            if units == 1 {
                scanned += 1;
            }
            Ok::<_, ()>(())
        })
        .expect("every mixed source fact is scanned");
        assert_eq!(scanned, evidence.source_facts.len());
    }

    #[test]
    fn initial_full_requires_exact_fresh_v5_replay_and_terminal_bindings() {
        let refusal = |evidence: &NativeCutoverEvidenceV1| {
            assert!(matches!(
                join_test(evidence, &mut |_| Ok::<_, ()>(())),
                Err(NativeCutoverSourceJoinErrorV1::Refused(_))
            ));
        };
        let mut evidence = fixed_evidence();
        evidence.v5_artifacts.clear();
        refusal(&evidence);

        let mut evidence = fixed_evidence();
        evidence.v5_artifacts.push(evidence.v5_artifacts[0].clone());
        refusal(&evidence);

        let mut evidence = fixed_evidence();
        evidence.v5_artifacts[0].operation.sequence += 1;
        refusal(&evidence);

        let mut evidence = fixed_evidence();
        evidence.fresh_sdk.target_snapshot_id += 1;
        evidence.fresh_sdk.committed_source_snapshot_id += 1;
        refusal(&evidence);

        let mut evidence = fixed_evidence();
        evidence.fresh_sdk.committed_source_snapshot_id += 1;
        refusal(&evidence);

        let mut evidence = fixed_evidence();
        evidence.fresh_sdk.committed_source_snapshot_digest = NativeCutoverDigest64V1([b'0'; 64]);
        refusal(&evidence);

        for wrong_digest in [
            NativeCutoverDigest64V1([b'd'; 64]),
            NativeCutoverDigest64V1([b'e'; 64]),
            NativeCutoverDigest64V1([b'f'; 64]),
        ] {
            let mut evidence = fixed_evidence();
            evidence.fresh_sdk.sdk_generation_digest = wrong_digest;
            refusal(&evidence);
        }

        let mut evidence = fixed_evidence();
        evidence
            .source_facts
            .retain(|fact| !matches!(fact, Fact::Replay { .. }));
        refusal(&evidence);

        let mut evidence = fixed_evidence();
        if let Fact::Replay {
            published_snapshot_id,
            ..
        } = &mut evidence.source_facts[2]
        {
            *published_snapshot_id = None;
        }
        refusal(&evidence);

        let mut evidence = fixed_evidence();
        if let Fact::Replay {
            prepared_artifact_digest,
            ..
        } = &mut evidence.source_facts[2]
        {
            *prepared_artifact_digest = Some(NativeCutoverDigest64V1([b'0'; 64]));
        }
        refusal(&evidence);

        let mut evidence = fixed_evidence();
        if let Fact::Replay {
            source_bound_aggregate_receipt_digest,
            ..
        } = &mut evidence.source_facts[2]
        {
            *source_bound_aggregate_receipt_digest = Some(NativeCutoverDigest64V1([b'4'; 64]));
        }
        refusal(&evidence);

        let mut evidence = fixed_evidence();
        if let Fact::PendingStoreTerminal {
            published_snapshot_id,
            ..
        } = &mut evidence.source_facts[6]
        {
            *published_snapshot_id += 1;
        }
        refusal(&evidence);
    }

    #[test]
    fn mismatched_or_missing_history_and_work_refusal_are_closed() {
        let mut evidence = fixed_evidence();
        if let Fact::PendingStoreTerminal {
            commit_owner_token, ..
        } = &mut evidence.source_facts[6]
        {
            *commit_owner_token = 99;
        }
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))
        ));
        let removed = evidence.source_facts.remove(6);
        assert!(matches!(removed, Fact::PendingStoreTerminal { .. }));
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))
        ));
        let evidence = fixed_evidence();
        assert!(matches!(
            join_test(&evidence, &mut |_| Err::<(), _>("work cap")),
            Err(NativeCutoverSourceJoinErrorV1::Work("work cap"))
        ));
        let mut evidence = fixed_evidence();
        evidence.source_facts.push(Fact::Allocator {
            next_operation_sequence: 8,
        });
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))
        ));
    }

    #[test]
    fn intermediate_custody_and_cleanup_transition_cannot_hide() {
        let mut evidence = fixed_evidence();
        let mut successor = evidence.source_facts[5].clone();
        if let Fact::PendingStoreCustody {
            custody_revision,
            predecessor_owner_token,
            predecessor_workspace_identity_digest,
            predecessor_runtime_generation,
            owner_token,
            owner_workspace_identity_digest,
            owner_runtime_generation,
            ..
        } = &mut successor
        {
            *custody_revision = 2;
            *predecessor_owner_token = 12;
            *predecessor_workspace_identity_digest = NativeCutoverDigest64V1([b'8'; 64]);
            *predecessor_runtime_generation = 2;
            *owner_token = 13;
            *owner_workspace_identity_digest = NativeCutoverDigest64V1([b'9'; 64]);
            *owner_runtime_generation = 3;
        }
        evidence.source_facts.insert(6, successor);
        if let Fact::PendingReplay {
            custody_revision, ..
        } = &mut evidence.source_facts[3]
        {
            *custody_revision = 2;
        }
        if let Fact::PendingStoreTerminal {
            commit_owner_token, ..
        } = &mut evidence.source_facts[7]
        {
            *commit_owner_token = 13;
        }
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("two exact custody revisions");
        assert!(joined.custody_head.is_some());
        if let Fact::PendingStoreCustody { plan_id, .. } = &mut evidence.source_facts[5] {
            *plan_id = "foreign-plan".into();
        }
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))
        ));
        let mut evidence = fixed_evidence();
        if let Fact::Replay { phase, .. } = &mut evidence.source_facts[2] {
            *phase = NativeCutoverReplayPhaseV1::CleanupFencing;
        }
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))
        ));
        let mut evidence = fixed_evidence();
        if let Fact::PendingStoreCheckpoint {
            terminal_state_revision,
            ..
        } = &mut evidence.source_facts[7]
        {
            *terminal_state_revision = 3;
        }
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))
        ));
    }

    fn with_published_ast(mut evidence: NativeCutoverEvidenceV1) -> NativeCutoverEvidenceV1 {
        let full = evidence.header.original_full_operation;
        let ast_stream = StreamId(*b"smca.ast.strm.v1");
        let ast_operation = crate::index::OperationKey {
            workspace: full.workspace,
            epoch: full.epoch,
            producer: NATIVE_STUB_AST_PRODUCER,
            stream: ast_stream,
            stream_epoch: 1,
            sequence: 1,
        };
        let source_view = ViewRef {
            workspace: full.workspace,
            epoch: full.epoch,
            sequence: 2,
            digest: Digest([15; 32]),
        };
        let published_view = ViewRef {
            sequence: 3,
            digest: Digest([16; 32]),
            ..source_view
        };
        let source_cut = SourceCut {
            owners: MapRoot(Some(ObjectRef(Digest([19; 32])))),
            paths: MapRoot(None),
            frontiers: MapRoot(Some(ObjectRef(Digest([20; 32])))),
        };
        let parser = Digest([17; 32]);
        let mut transition = ProjectionTransition {
            projection: NATIVE_STUB_AST_PROJECTION,
            producer: NATIVE_STUB_AST_PRODUCER,
            expected: None,
            target_source: source_cut,
            schema: NATIVE_STUB_AST_SCHEMA,
            producer_policy: native_stub_certificate_policy(),
            dependency_context: parser,
            completeness: ProducerCompleteness::CompleteAffectedDomain {
                certificate: ObjectRef(Digest([21; 32])),
            },
            owners: OwnerMutationSet {
                root: MapRoot(None),
                count: 0,
                digest: Digest([22; 32]),
            },
            events: vec![SourceEvent {
                producer: ast_operation.producer,
                stream: ast_operation.stream,
                stream_epoch: 1,
                predecessor: 0,
                sequence: 1,
                payload: Digest([0; 32]),
            }],
        };
        transition.events[0].payload = transition.payload_digest().expect("fixed AST payload");
        let request = UpdateRequest {
            operation: ast_operation,
            expected_view: source_view,
            kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
        };
        evidence.source_facts.push(Fact::AstFrontier {
            candidate_root_nonce: Some(Digest([22; 32])),
            workspace: full.workspace,
            workspace_epoch: 1,
            producer: NATIVE_STUB_AST_PRODUCER,
            stream: ast_stream,
            stream_epoch: 1,
            next_operation_sequence: 2,
            committed_event_sequence: 1,
        });
        evidence.source_facts.push(Fact::AstWork {
            candidate_root_nonce: Some(Digest([22; 32])),
            aggregate_digest: "1".repeat(64),
            source_operation: full,
            lexical_artifact_digest: Digest([18; 32]),
            source_view,
            source_cut,
            revision: 4,
            phase: NativeCutoverAstWorkPhaseV1::Published,
            parser_registration: Some(parser),
            ast_operation: Some(ast_operation),
            ast_event_sequence: Some(1),
            claim_owner_epoch: Some(1),
            claim_fence: Some(2),
            lease_expires_epoch_ms: Some(100),
            refusal: None,
            published_view: Some(published_view),
            superseded_by_aggregate_digest: None,
            created_at_epoch_ms: 2,
            updated_at_epoch_ms: 3,
            request: Some(request),
        });
        evidence
    }

    #[test]
    fn published_ast_joins_exact_v9_source_frontier_and_payload() {
        let evidence = with_published_ast(fixed_evidence());
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("one exact Published AST");
        assert_eq!(
            joined.ast_update_request_v1().unwrap().operation.sequence,
            1
        );
        let mut evidence = evidence.clone();
        if let Fact::AstWork {
            aggregate_digest, ..
        } = evidence.source_facts.last_mut().unwrap()
        {
            *aggregate_digest = "f".repeat(64);
        }
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))
        ));
        let mut evidence = with_published_ast(fixed_evidence());
        if let Fact::AstFrontier {
            committed_event_sequence,
            ..
        } = &mut evidence.source_facts[8]
        {
            *committed_event_sequence = 0;
        }
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))
        ));
        let mut evidence = with_published_ast(fixed_evidence());
        if let Fact::AstWork {
            request: Some(request),
            ..
        } = evidence.source_facts.last_mut().unwrap()
        {
            if let UpdateKind::ProjectionAdvance(transition) = &mut request.kind {
                transition.events[0].payload = Digest([99; 32]);
            }
        }
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))
        ));
        let mut evidence = with_published_ast(fixed_evidence());
        if let Fact::AstWork {
            request: Some(request),
            ..
        } = evidence.source_facts.last_mut().unwrap()
        {
            request.expected_view.sequence = 1;
        }
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))
        ));
    }

    #[test]
    fn ast_inventory_refuses_duplicate_global_operation_across_roots() {
        let mut evidence = with_published_ast(fixed_evidence());
        let mut second = evidence.source_facts.iter().find(|fact|
            matches!(fact, Fact::AstWork { .. })).unwrap().clone();
        if let Fact::AstWork { aggregate_digest, candidate_root_nonce, .. } = &mut second {
            *aggregate_digest = "2".repeat(64);
            *candidate_root_nonce = Some(Digest([33; 32]));
        }
        let mut second_frontier = evidence.source_facts.iter().find(|fact|
            matches!(fact, Fact::AstFrontier { .. })).unwrap().clone();
        if let Fact::AstFrontier { candidate_root_nonce, .. } = &mut second_frontier {
            *candidate_root_nonce = Some(Digest([33; 32]));
        }
        evidence.source_facts.extend([second_frontier, second]);
        evidence.source_facts.sort_by(|left, right| mixed_rank(left).cmp(&mixed_rank(right))
            .then_with(|| mixed_key(left).cmp(&mixed_key(right))));
        let mut operations = Vec::with_capacity(evidence.source_facts.len());
        let mut published = Vec::with_capacity(evidence.source_facts.len());
        assert!(matches!(validate_ast_inventory_v1(&evidence, &mut operations,
            &mut published, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused("AST global operation is duplicated"))));
    }

    #[test]
    fn ast_inventory_refuses_published_gap_and_cross_root_borrow() {
        let mut gap = with_published_ast(fixed_evidence());
        for fact in &mut gap.source_facts {
            match fact {
                Fact::AstFrontier { committed_event_sequence, .. } => *committed_event_sequence = 2,
                Fact::AstWork { ast_event_sequence, .. } => *ast_event_sequence = Some(2),
                _ => {}
            }
        }
        let mut operations = Vec::with_capacity(gap.source_facts.len());
        let mut published = Vec::with_capacity(gap.source_facts.len());
        assert!(matches!(validate_ast_inventory_v1(&gap, &mut operations,
            &mut published, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused("Published AST root event has a gap or duplicate"))));

        let mut foreign = with_published_ast(fixed_evidence());
        for fact in &mut foreign.source_facts {
            if let Fact::AstWork { candidate_root_nonce, .. } = fact {
                *candidate_root_nonce = Some(Digest([33; 32]));
            }
        }
        let mut operations = Vec::with_capacity(foreign.source_facts.len());
        let mut published = Vec::with_capacity(foreign.source_facts.len());
        assert!(matches!(validate_ast_inventory_v1(&foreign, &mut operations,
            &mut published, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));
    }

    #[test]
    fn ast_inventory_requires_funded_scratch_and_charges_work() {
        let evidence = with_published_ast(fixed_evidence());
        assert!(matches!(validate_ast_inventory_v1(&evidence, &mut Vec::new(),
            &mut Vec::new(), &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));
        let mut operations = Vec::with_capacity(evidence.source_facts.len());
        let mut published = Vec::with_capacity(evidence.source_facts.len());
        assert!(matches!(validate_ast_inventory_v1(&evidence, &mut operations,
            &mut published, &mut |_| Err::<(), _>("work refused")),
            Err(NativeCutoverSourceJoinErrorV1::Work("work refused"))));
    }

    fn signed_mixed_legacy_fixture() -> (NativeCutoverEvidenceV1, NativeCutoverEvidenceV1) {
        let native = native_only_initial_full_evidence();
        let legacy = fixed_evidence();
        let pick = |facts: &[Fact], predicate: fn(&Fact) -> bool| -> Fact {
            facts
                .iter()
                .find(|fact| predicate(fact))
                .expect("fixture fact")
                .clone()
        };
        let mut native_replay = pick(&native.source_facts, |f| matches!(f, Fact::Replay { .. }));
        if let Fact::Replay {
            aggregate_identity_digest,
            ..
        } = &mut native_replay
        {
            *aggregate_identity_digest = "9".repeat(64);
        }
        let mut native_checkpoint = pick(&native.source_facts, |f| {
            matches!(f, Fact::PendingStoreCheckpoint { .. })
        });
        if let Fact::PendingStoreCheckpoint {
            aggregate_identity_digest,
            ..
        } = &mut native_checkpoint
        {
            *aggregate_identity_digest = "9".repeat(64);
        }
        let mut legacy_replay = pick(&legacy.source_facts, |f| matches!(f, Fact::Replay { .. }));
        if let Fact::Replay {
            native_update_request,
            ..
        } = &mut legacy_replay
        {
            *native_update_request = None;
        }
        let binding = NativeCutoverEvidenceSdkBindingV1 {
            source_intent_digest: "a".repeat(64),
            completion_pointer_digest: "d".repeat(64),
            transition_digest: "e".repeat(64),
            target_source_digest: "f".repeat(64),
            target_snapshot: 17,
            ingress_kind: "sdk".into(),
            workspace_identity_digest: "1".repeat(64),
            request_fingerprint_digest: "c".repeat(64),
            client_idempotency_digest: None,
        };
        let sdk_ack = Fact::PendingStoreSdkAck {
            plan_id: "plan".into(),
            attempt_id: 1,
            commit_owner_token: 12,
            terminal_receipt_digest: NativeCutoverDigest64V1([b'2'; 64]),
            binding: binding.clone(),
            canonical_receipt_digest: "2".repeat(64),
            acked_binding: Some(binding),
            checkpoint_generation: 9,
            checkpoint_payload_digest: Digest([0x33; 32]),
            checkpoint_acked_at_epoch_ms: 5,
            cleanup_completed_at_epoch_ms: Some(6),
            created_at_epoch_ms: 4,
        };
        let roots = SemanticContentRootsV1 {
            row_root_digest: format!("sha256:{}", "a".repeat(64)),
            membership_root_digest: format!("sha256:{}", "b".repeat(64)),
        };
        let generation = ManifestGeneration::new(2);
        let snapshot = GenerationSnapshot {
            repo_id: RepoId::new("repo").expect("canonical repo"),
            revision_id: RevisionId::new("c".repeat(64)).expect("canonical revision"),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: generation,
            manifest_digest: "manifest".into(),
        };
        let mut semantic = snapshot.clone();
        semantic.track = SearchPlaneTrackKind::Semantic;
        let mut receipt =
            BatchPublishReceipt::empty_for(generation, Some("manifest".into()), "batch");
        receipt.sealed = true;
        receipt.durable_sequence = 1;
        receipt.semantic_content = Some(roots.clone());
        let mut mixed = native.clone();
        mixed.legacy_publication = Some(NativeCutoverEvidenceLegacyPublicationV1 {
            receipt,
            history: None,
            repomap: crate::native_cutover_evidence_v1::NativeCutoverEvidenceLegacyRepoMapV1::NotApplicable {
                reason_code: "fixture-without-repomap".into(),
                policy_digest: NativeCutoverDigest64V1([b'a'; 64]),
                product_scope_digest: NativeCutoverDigest64V1([b'b'; 64]),
            },
            activation: SearchPlaneSearchCorpusActivationCasAck {
                active: SearchCorpusActiveHeadV1 {
                    generation: SearchCorpusGenerationIdentityV1 {
                        lexical: snapshot,
                        semantic,
                        semantic_content: roots,
                    },
                    activation_token: SearchCorpusActivationTokenV1::new(
                        [1; 16],
                        NonZeroU64::new(1).expect("nonzero"),
                    )
                    .expect("nonzero root"),
                },
                previous_sealed_active: None,
            },
        });
        mixed.source_facts = vec![
            pick(&native.source_facts, |f| {
                matches!(f, Fact::Allocator { .. })
            }),
            pick(&native.source_facts, |f| {
                matches!(f, Fact::Reservation { .. })
            }),
            legacy_replay,
            native_replay,
            pick(&legacy.source_facts, |f| {
                matches!(f, Fact::PendingReplay { .. })
            }),
            pick(&legacy.source_facts, |f| {
                matches!(f, Fact::PendingStoreAttempt { .. })
            }),
            pick(&legacy.source_facts, |f| {
                matches!(f, Fact::PendingStoreCustody { .. })
            }),
            pick(&legacy.source_facts, |f| {
                matches!(f, Fact::PendingStoreTerminal { .. })
            }),
            sdk_ack,
            pick(&legacy.source_facts, |f| {
                matches!(f, Fact::PendingStoreCheckpoint { .. })
            }),
            native_checkpoint,
        ];
        (native, mixed)
    }

    fn ordinary_initial_legacy_fixture() -> NativeCutoverEvidenceV1 {
        let (_, mut evidence) = signed_mixed_legacy_fixture();
        evidence.source_facts.retain(|fact| matches!(
            fact,
            Fact::Allocator { .. }
                | Fact::Reservation { .. }
                | Fact::Replay { .. }
                | Fact::PendingStoreCheckpoint { .. }
        ));
        let replay = evidence.source_facts.iter_mut().find(|fact| matches!(
            fact, Fact::Replay { native_update_request: None, .. }
        )).expect("ordinary initial V9");
        let Fact::Replay {
            closeout_id,
            attempt_id,
            ordinary_source_binding,
            ..
        } = replay else { unreachable!() };
        *closeout_id = "ordinary-initial-full".into();
        *attempt_id = "ordinary-initial-attempt".into();
        *ordinary_source_binding = Some(NativeCutoverEvidenceSourcePublicationBindingV1 {
            transition_digest: NativeCutoverDigest64V1([b'e'; 64]),
            completion_pointer_digest: NativeCutoverDigest64V1([b'd'; 64]),
            physical_target_source_digest: NativeCutoverDigest64V1([b'f'; 64]),
            target_snapshot_id: 17,
        });
        evidence
    }

    fn ordinary_initial_binding_mut(
        evidence: &mut NativeCutoverEvidenceV1,
    ) -> &mut NativeCutoverEvidenceSourcePublicationBindingV1 {
        evidence.source_facts.iter_mut().find_map(|fact| match fact {
            Fact::Replay { native_update_request: None,
                ordinary_source_binding: Some(binding), .. } => Some(binding),
            _ => None,
        }).expect("ordinary initial binding")
    }

    fn settled_native_successor_evidence(successors: u8) -> NativeCutoverEvidenceV1 {
        let (_, mut evidence) = signed_mixed_legacy_fixture();
        let templates = evidence.source_facts.clone();
        evidence.legacy_publication = None;
        let mut prior = evidence.baseline;
        for step in 1..=successors {
            let mut target = prior;
            target.operation.sequence = 7 + u64::from(step) * 4;
            target.event_predecessor = prior.event_sequence;
            target.event_sequence = prior.event_sequence + 1;
            target.canonical_target_digest = Digest([10 + step; 32]);
            let plan = format!("np{step}");
            let aggregate_byte = b'a' + step - 1;
            let aggregate = char::from(aggregate_byte).to_string().repeat(64);
            let snapshot = 17 + u64::from(step);
            evidence.source_facts.push(Fact::Reservation {
                target,
                evidence: NativeCutoverReservationEvidenceV1::Sdk {
                    identity_kind: "Keyless".into(),
                    attempt_identity_digest: NativeCutoverDigest64V1([b'a'; 64]),
                    routing_identity_digest: NativeCutoverDigest64V1([b'b'; 64]),
                    prior_target_digest: prior.canonical_target_digest,
                },
                candidate_root_nonce: Some(Digest([22; 32])),
            });
            let pick = |predicate: fn(&Fact) -> bool| -> Fact {
                templates.iter().find(|fact| predicate(fact)).unwrap().clone()
            };
            let mut replay = pick(|fact| matches!(fact, Fact::Replay {
                native_update_request: Some(_), ..
            }));
            if let Fact::Replay {
                aggregate_identity_digest,
                closeout_id,
                attempt_id,
                native_update_request: Some(request),
                published_snapshot_id,
                ..
            } = &mut replay {
                *aggregate_identity_digest = aggregate.clone();
                *closeout_id = format!("change-event:{plan}");
                *attempt_id = format!("change-event:{plan}:attempt-1");
                request.operation = target.operation;
                request.expected_view = target.expected_view;
                *published_snapshot_id = Some(snapshot);
            }
            evidence.source_facts.push(replay);
            let mut checkpoint = pick(|fact| matches!(fact, Fact::PendingStoreCheckpoint {
                aggregate_identity_digest, ..
            } if aggregate_identity_digest == &"9".repeat(64)));
            if let Fact::PendingStoreCheckpoint { aggregate_identity_digest, .. } = &mut checkpoint {
                *aggregate_identity_digest = aggregate.clone();
            }
            evidence.source_facts.push(checkpoint);
            for mut group in templates.iter().filter(|fact| matches!(fact,
                Fact::PendingReplay { .. } | Fact::PendingStoreAttempt { .. }
                | Fact::PendingStoreCustody { .. } | Fact::PendingStoreTerminal { .. }
                | Fact::PendingStoreSdkAck { .. }
            )).cloned() {
                match &mut group {
                    Fact::PendingReplay { plan_id, aggregate_identity_digest, .. } => {
                        *plan_id = plan.clone();
                        *aggregate_identity_digest = aggregate.clone();
                    }
                    Fact::PendingStoreAttempt { plan_id, .. } => *plan_id = plan.clone(),
                    Fact::PendingStoreCustody { plan_id, aggregate_identity_digest, .. } => {
                        *plan_id = plan.clone();
                        *aggregate_identity_digest = NativeCutoverDigest64V1([aggregate_byte; 64]);
                    }
                    Fact::PendingStoreTerminal { plan_id, published_snapshot_id, .. } => {
                        *plan_id = plan.clone();
                        *published_snapshot_id = snapshot;
                    }
                    Fact::PendingStoreSdkAck { plan_id, binding, acked_binding, .. } => {
                        *plan_id = plan.clone();
                        binding.transition_digest = char::from(b'0' + step).to_string().repeat(64);
                        binding.target_snapshot = snapshot;
                        *acked_binding = Some(binding.clone());
                    }
                    _ => unreachable!("settled group template"),
                }
                evidence.source_facts.push(group);
            }
            prior = target;
        }
        evidence.v5_artifacts[0].operation = prior.operation;
        evidence.v5_artifacts[0].published_snapshot_id = 17 + u64::from(successors);
        evidence.v5_artifacts[0].previous_transition_digest = Some(NativeCutoverDigest64V1(
            [if successors == 1 { b'e' } else { b'0' + successors - 1 }; 64],
        ));
        evidence.v5_artifacts[0].base_source_digest = Some(NativeCutoverDigest64V1([b'f'; 64]));
        evidence.fresh_sdk.target_snapshot_id = 17 + u64::from(successors);
        evidence.fresh_sdk.committed_source_snapshot_id = evidence.fresh_sdk.target_snapshot_id;
        evidence.fresh_sdk.source_manifest_digest = NativeCutoverDigest64V1([b'0' + successors; 64]);
        if let Some(Fact::Allocator { next_operation_sequence }) = evidence.source_facts
            .iter_mut().find(|fact| matches!(fact, Fact::Allocator { .. })) {
            *next_operation_sequence = prior.operation.sequence + 1;
        }
        evidence.source_facts.sort_by(|left, right| mixed_rank(left).cmp(&mixed_rank(right))
            .then_with(|| mixed_key(left).cmp(&mixed_key(right))));
        evidence
    }

    fn new_initial_root_after_native_sdk_m() -> NativeCutoverEvidenceV1 {
        let mut evidence = settled_native_successor_evidence(1);
        let mut new_target = evidence.baseline;
        new_target.operation.sequence = 41;
        new_target.canonical_target_digest = Digest([44; 32]);
        evidence.baseline = new_target;
        evidence.header.original_full_operation = new_target.operation;
        let mut new_replay = evidence.source_facts.iter().find(|fact| matches!(fact,
            Fact::Replay { native_update_request: Some(request), .. }
                if request.operation.sequence == 7)).expect("old initial V9").clone();
        if let Fact::Replay { aggregate_identity_digest, closeout_id, attempt_id,
            native_update_request: Some(request), published_snapshot_id, .. } = &mut new_replay {
            *aggregate_identity_digest = "3".repeat(64);
            *closeout_id = format!("initial-native-full:{}", "d".repeat(64));
            let hex = sa_payload_to_hex(&new_target.canonical_target_digest);
            *attempt_id = format!("source-target:{}", std::str::from_utf8(&hex).unwrap());
            request.operation = new_target.operation;
            request.expected_view = new_target.expected_view;
            *published_snapshot_id = Some(99);
        }
        let mut new_checkpoint = evidence.source_facts.iter().find(|fact| matches!(fact,
            Fact::PendingStoreCheckpoint { aggregate_identity_digest, .. }
                if aggregate_identity_digest == &"9".repeat(64)))
            .expect("old initial checkpoint").clone();
        if let Fact::PendingStoreCheckpoint { aggregate_identity_digest, .. } = &mut new_checkpoint {
            *aggregate_identity_digest = "3".repeat(64);
        }
        evidence.source_facts.push(Fact::Reservation {
            target: new_target,
            evidence: NativeCutoverReservationEvidenceV1::Initial,
            candidate_root_nonce: Some(Digest([44; 32])),
        });
        evidence.source_facts.extend([new_replay, new_checkpoint]);
        if let Some(Fact::Allocator { next_operation_sequence }) = evidence.source_facts.iter_mut()
            .find(|fact| matches!(fact, Fact::Allocator { .. })) {
            *next_operation_sequence = 42;
        }
        evidence.v5_artifacts[0].operation = new_target.operation;
        evidence.v5_artifacts[0].published_snapshot_id = 99;
        evidence.v5_artifacts[0].previous_transition_digest = None;
        evidence.v5_artifacts[0].base_source_digest = None;
        evidence.v5_artifacts[0].lexical_revision_digest = NativeCutoverDigest64V1([b'd'; 64]);
        evidence.fresh_sdk.target_snapshot_id = 99;
        evidence.fresh_sdk.committed_source_snapshot_id = 99;
        evidence.fresh_sdk.sdk_generation_digest = NativeCutoverDigest64V1([b'd'; 64]);
        evidence.fresh_sdk.source_manifest_digest = NativeCutoverDigest64V1([b'9'; 64]);
        evidence.fresh_sdk.source_pointer_digest = NativeCutoverDigest64V1([b'8'; 64]);
        evidence.fresh_sdk.physical_target_source_digest = NativeCutoverDigest64V1([b'7'; 64]);
        evidence.fresh_sdk.committed_source_snapshot_digest = NativeCutoverDigest64V1([b'7'; 64]);
        evidence.source_facts.sort_by(|left, right| mixed_rank(left).cmp(&mixed_rank(right))
            .then_with(|| mixed_key(left).cmp(&mixed_key(right))));
        evidence
    }

    fn initial_source_from_fresh_v1(
        evidence: &NativeCutoverEvidenceV1,
    ) -> NativeCutoverEvidenceSourcePublicationBindingV1 {
        NativeCutoverEvidenceSourcePublicationBindingV1 {
            transition_digest: evidence.fresh_sdk.source_manifest_digest,
            completion_pointer_digest: evidence.fresh_sdk.source_pointer_digest,
            physical_target_source_digest: evidence.fresh_sdk.physical_target_source_digest,
            target_snapshot_id: evidence.fresh_sdk.target_snapshot_id,
        }
    }

    fn native_initial_source_fact_v1(
        evidence: &NativeCutoverEvidenceV1,
        sequence: u64,
        source: NativeCutoverEvidenceSourcePublicationBindingV1,
    ) -> Fact {
        let (operation, aggregate_identity_digest) = evidence.source_facts.iter()
            .find_map(|fact| match fact {
                Fact::Replay {
                    native_update_request: Some(request),
                    aggregate_identity_digest,
                    ..
                } if request.operation.sequence == sequence =>
                    Some((request.operation, aggregate_identity_digest.clone())),
                _ => None,
            }).expect("fixed initial V9");
        Fact::NativeInitialSourceBinding { operation, aggregate_identity_digest, source }
    }

    #[test]
    fn native_initial_source_is_optional_borrowed_and_current_parity_is_conditional() {
        let mut initial = native_only_initial_full_evidence();
        assert!(join_test(&initial, &mut |_| Ok::<_, ()>(()))
            .expect("old initial without Source4 remains joined")
            .native_initial_source_for_operation_v1(initial.baseline.operation).is_none());
        initial.fresh_sdk.target_snapshot_id = initial.fresh_sdk.target_snapshot_id.checked_sub(1)
            .filter(|snapshot| *snapshot != 0)
            .expect("fixed Source pointer precedes frozen FULL N");
        let source = initial_source_from_fresh_v1(&initial);
        initial.source_facts.push(native_initial_source_fact_v1(
            &initial, initial.baseline.operation.sequence, source,
        ));
        let joined = join_test(&initial, &mut |_| Ok::<_, ()>(()))
            .expect("current initial M pointer snapshot differs from frozen N snapshot");
        assert_ne!(source.target_snapshot_id, joined.v5_artifact.published_snapshot_id);
        assert_eq!(joined.native_initial_source_for_operation_v1(initial.baseline.operation)
            .expect("borrowed initial Source4").0, &source);

        for axis in 0..3 {
            let mut wrong_current = initial.clone();
            if let Some(Fact::NativeInitialSourceBinding { source, .. }) = wrong_current.source_facts
                .iter_mut().find(|fact| matches!(fact, Fact::NativeInitialSourceBinding { .. })) {
                match axis {
                    0 => source.transition_digest = NativeCutoverDigest64V1([b'a'; 64]),
                    1 => source.completion_pointer_digest = NativeCutoverDigest64V1([b'a'; 64]),
                    _ => source.physical_target_source_digest = NativeCutoverDigest64V1([b'a'; 64]),
                }
            }
            assert!(matches!(join_test(&wrong_current, &mut |_| Ok::<_, ()>(())),
                Err(NativeCutoverSourceJoinErrorV1::Refused(
                    "selected Native initial Source pointer or digest differs from fresh SDK"))));
        }
        let mut foreign_m = initial.clone();
        if let Some(Fact::NativeInitialSourceBinding { source, .. }) = foreign_m.source_facts
            .iter_mut().find(|fact| matches!(fact, Fact::NativeInitialSourceBinding { .. })) {
            source.target_snapshot_id += 1;
        }
        assert!(matches!(join_test(&foreign_m, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "selected Native initial Source pointer or digest differs from fresh SDK"))));
        let mut foreign_n = initial.clone();
        foreign_n.fresh_sdk.committed_source_snapshot_id += 1;
        assert!(matches!(join_test(&foreign_n, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));

        let mut older = new_initial_root_after_native_sdk_m();
        let old_source = NativeCutoverEvidenceSourcePublicationBindingV1 {
            transition_digest: NativeCutoverDigest64V1([b'a'; 64]),
            completion_pointer_digest: NativeCutoverDigest64V1([b'b'; 64]),
            physical_target_source_digest: NativeCutoverDigest64V1([b'c'; 64]),
            target_snapshot_id: 55,
        };
        older.source_facts.push(native_initial_source_fact_v1(&older, 7, old_source));
        let joined = join_test(&older, &mut |_| Ok::<_, ()>(()))
            .expect("historical Source pointer snapshot is independent of old Q V9");
        let old_operation = older.source_facts.iter().find_map(|fact| match fact {
            Fact::NativeInitialSourceBinding { operation, .. } => Some(*operation),
            _ => None,
        }).expect("old initial Source operation");
        assert_eq!(joined.native_initial_source_for_operation_v1(old_operation)
            .expect("old borrowed Source4").0.target_snapshot_id, 55);
        assert!(joined.native_initial_source_for_operation_v1(older.baseline.operation).is_none());

        let mut legacy_after_initial = legacy_m_at_new_initial_root();
        legacy_after_initial.source_facts.push(native_initial_source_fact_v1(
            &legacy_after_initial, 41, old_source,
        ));
        let joined = join_test(&legacy_after_initial, &mut |_| Ok::<_, ()>(()))
            .expect("later selected Legacy M owns fresh parity");
        assert!(joined.current_legacy_replay.is_some());
        assert_eq!(joined.native_initial_source_for_operation_v1(
            legacy_after_initial.baseline.operation).unwrap().0, &old_source);
    }

    #[test]
    fn native_initial_source_refuses_orphan_duplicate_noninitial_and_aggregate() {
        let baseline = native_only_initial_full_evidence();
        let source = initial_source_from_fresh_v1(&baseline);
        let first = native_initial_source_fact_v1(&baseline,
            baseline.baseline.operation.sequence, source);
        let mut duplicate = baseline.clone();
        duplicate.source_facts.extend([first.clone(), first.clone()]);
        assert!(matches!(join_test(&duplicate, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "Native initial Source operation duplicated"))));

        let mut orphan = baseline.clone();
        let mut foreign = first.clone();
        if let Fact::NativeInitialSourceBinding { operation, .. } = &mut foreign {
            operation.sequence += 1;
        }
        orphan.source_facts.push(foreign);
        assert!(matches!(join_test(&orphan, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));

        let mut aggregate = baseline;
        let mut mismatched = first;
        if let Fact::NativeInitialSourceBinding { aggregate_identity_digest, .. } = &mut mismatched {
            *aggregate_identity_digest = "f".repeat(64);
        }
        aggregate.source_facts.push(mismatched);
        assert!(matches!(join_test(&aggregate, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "Native initial Source operation or V9 aggregate differs"))));

        let mut sdk = settled_native_successor_evidence(1);
        let mut sdk_source = initial_source_from_fresh_v1(&sdk);
        // This older fixture uses `g` for its SDK transition marker, which
        // is not a canonical Source4 digest. The non-Initial rejection must
        // be reached with an otherwise valid new fact.
        sdk_source.transition_digest = NativeCutoverDigest64V1([b'a'; 64]);
        sdk.source_facts.push(native_initial_source_fact_v1(&sdk, 11, sdk_source));
        assert!(matches!(join_test(&sdk, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "Native initial Source has no Initial reservation"))));

        let mut unsorted = new_initial_root_after_native_sdk_m();
        let source = initial_source_from_fresh_v1(&unsorted);
        let new = native_initial_source_fact_v1(&unsorted, 41, source);
        let old = native_initial_source_fact_v1(&unsorted, 7, source);
        unsorted.source_facts.extend([new, old]);
        assert!(matches!(join_test(&unsorted, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "Native initial Source operation duplicated or unsorted"))));
        assert!(matches!(join_test(&unsorted, &mut |_| Err::<(), _>("join work cap")),
            Err(NativeCutoverSourceJoinErrorV1::Work("join work cap"))));
    }

    #[test]
    fn new_initial_full_joins_old_native_sdk_m_without_physical_axis_conflation() {
        let evidence = new_initial_root_after_native_sdk_m();
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("settled old Native M and independent new initial FULL");
        assert_eq!(joined.baseline.operation.sequence, 41);
        assert_eq!(joined.current_native_target.operation.sequence, 41);
        assert_eq!(joined.baseline_candidate_root_nonce, Some(Digest([44; 32])));
        assert!(joined.sdk_acked_binding.is_none());
        assert!(joined.prior_sdk_acked_binding.is_none());
        assert!(evidence.source_facts.iter().any(|fact| matches!(fact,
            Fact::PendingStoreSdkAck { binding, .. }
                if binding.target_snapshot == 18
                    && binding.transition_digest != "9".repeat(64))));
        assert_eq!(evidence.fresh_sdk.target_snapshot_id, 99);
    }

    fn legacy_m_at_new_initial_root() -> NativeCutoverEvidenceV1 {
        let mut evidence = new_initial_root_after_native_sdk_m();
        let mut publication = signed_mixed_legacy_fixture()
            .1
            .legacy_publication
            .expect("sealed Legacy publication");
        let generation = &mut publication.activation.active.generation;
        let revision = RevisionId::new("d".repeat(64)).expect("fresh SDK revision");
        generation.lexical.revision_id = revision.clone();
        generation.semantic.revision_id = revision;
        evidence.legacy_publication = Some(publication);
        bind_legacy_fixture_to_fresh_sdk(&mut evidence, "plan");
        evidence
    }

    fn bind_legacy_fixture_to_fresh_sdk(evidence: &mut NativeCutoverEvidenceV1, plan: &str) {
        let closeout = format!("change-event:{plan}");
        let snapshot = evidence.fresh_sdk.target_snapshot_id;
        for fact in &mut evidence.source_facts {
            match fact {
                Fact::Replay {
                    closeout_id,
                    native_update_request,
                    published_snapshot_id,
                    ..
                } if closeout_id.as_str() == closeout.as_str() => {
                    *native_update_request = None;
                    *published_snapshot_id = Some(snapshot);
                }
                Fact::PendingStoreTerminal {
                    plan_id,
                    published_snapshot_id,
                    ..
                } if plan_id.as_str() == plan => *published_snapshot_id = snapshot,
                Fact::PendingStoreSdkAck {
                    plan_id,
                    binding,
                    acked_binding,
                    ..
                } if plan_id.as_str() == plan => {
                    binding.target_snapshot = snapshot;
                    binding.transition_digest = "9".repeat(64);
                    binding.completion_pointer_digest = "8".repeat(64);
                    binding.target_source_digest = "7".repeat(64);
                    *acked_binding = Some(binding.clone());
                }
                _ => {}
            }
        }
    }

    #[test]
    fn multi_root_initial_full_returns_only_the_current_legacy_m() {
        let evidence = legacy_m_at_new_initial_root();
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("fresh Legacy M and settled historical Native root");
        assert_eq!(joined.current_native_target.operation.sequence, 41);
        assert_eq!(joined.baseline_candidate_root_nonce, Some(Digest([44; 32])));
        assert!(matches!(
            joined.current_legacy_replay,
            Some(Fact::Replay { closeout_id, native_update_request: None,
                published_snapshot_id: Some(99), .. }) if closeout_id == "change-event:plan"
        ));
        assert!(matches!(
            joined.current_legacy_sdk_ack,
            Some(Fact::PendingStoreSdkAck { plan_id, binding, .. })
                if plan_id == "plan" && binding.target_snapshot == 99
        ));
        assert!(joined.sdk_acked_binding.is_none());
        assert!(joined.prior_sdk_acked_binding.is_none());
    }

    #[test]
    fn multi_root_initial_full_refuses_stale_foreign_or_missing_legacy_m() {
        let mut evidence = legacy_m_at_new_initial_root();
        let ack = evidence
            .source_facts
            .iter_mut()
            .find(|fact| {
                matches!(fact, Fact::PendingStoreSdkAck { plan_id, .. } if plan_id == "plan")
            })
            .expect("current Legacy SDK ACK");
        let Fact::PendingStoreSdkAck { binding, acked_binding, .. } = ack else {
            unreachable!("selected SDK ACK");
        };
        binding.completion_pointer_digest = "d".repeat(64);
        *acked_binding = Some(binding.clone());
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "current SDK M binding differs from fresh Source"
            ))
        ));

        let mut evidence = legacy_m_at_new_initial_root();
        for fact in &mut evidence.source_facts {
            if let Fact::PendingStoreSdkAck { plan_id, binding, acked_binding, .. } = fact
                && plan_id == "plan"
            {
                binding.transition_digest = "e".repeat(64);
                *acked_binding = Some(binding.clone());
            }
        }
        let assert_absent = |evidence: &NativeCutoverEvidenceV1| {
            assert!(matches!(
                join_test(evidence, &mut |_| Ok::<_, ()>(())),
                Err(NativeCutoverSourceJoinErrorV1::Refused("current Legacy SDK M ACK is absent"))
            ));
        };
        assert_absent(&evidence);
        evidence.source_facts.retain(|fact| {
            !matches!(fact, Fact::PendingStoreSdkAck { plan_id, .. } if plan_id == "plan")
        });
        assert_absent(&evidence);
    }

    #[test]
    fn multi_root_initial_full_refuses_two_legacy_acks_for_the_fresh_sdk() {
        let mut evidence = legacy_m_at_new_initial_root();
        // Make the other settled change event a Legacy publication with its
        // own carrier, V9 and checkpoint, but with the same fresh SDK identity.
        evidence.source_facts.retain(|fact| !matches!(
            fact, Fact::Reservation { target, .. } if target.operation.sequence == 11
        ));
        bind_legacy_fixture_to_fresh_sdk(&mut evidence, "np1");
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused("multiple current SDK M ACKs remain"))
        ));
    }

    #[test]
    fn native_successor_refuses_a_second_current_publication() {
        let mut evidence = settled_native_successor_evidence(1);
        evidence.legacy_publication = signed_mixed_legacy_fixture().1.legacy_publication;
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "current Legacy publication conflicts with selected Native successor"
            ))
        ));
    }

    #[test]
    fn multi_root_initial_full_refuses_a_native_history_ack_for_current_legacy_m() {
        let mut evidence = legacy_m_at_new_initial_root();
        let ack = evidence
            .source_facts
            .iter_mut()
            .find(|fact| {
                matches!(fact, Fact::PendingStoreSdkAck { plan_id, .. } if plan_id == "np1")
            })
            .expect("historical Native SDK ACK");
        let Fact::PendingStoreSdkAck { binding, acked_binding, .. } = ack else {
            unreachable!("selected SDK ACK");
        };
        binding.transition_digest = "9".repeat(64);
        *acked_binding = Some(binding.clone());
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "current Legacy SDK M conflicts with Native history ACK"
            ))
        ));
    }

    fn settled_native_ast_history(successors: u8, superseded_prefix: u8)
        -> NativeCutoverEvidenceV1
    {
        assert!(successors >= 1 && superseded_prefix <= successors);
        let mut evidence = settled_native_successor_evidence(successors);
        let template = with_published_ast(fixed_evidence()).source_facts
            .into_iter().find(|fact| matches!(fact, Fact::AstWork { .. }))
            .expect("AST work template");
        let full = evidence.header.original_full_operation;
        evidence.source_facts.push(Fact::AstFrontier {
            candidate_root_nonce: Some(Digest([22; 32])),
            workspace: full.workspace,
            workspace_epoch: full.epoch,
            producer: NATIVE_STUB_AST_PRODUCER,
            stream: StreamId(*b"smca.ast.strm.v1"),
            stream_epoch: 1,
            next_operation_sequence: u64::from(successors - superseded_prefix) + 2,
            committed_event_sequence: u64::from(successors - superseded_prefix) + 1,
        });
        for step in 0..=successors {
            let mut work = template.clone();
            let aggregate = if step == 0 {
                "9".repeat(64)
            } else {
                char::from(b'a' + step - 1).to_string().repeat(64)
            };
            let successor = if step < successors {
                Some(char::from(b'a' + step).to_string().repeat(64))
            } else {
                None
            };
            let source_operation_sequence = 7 + u64::from(step) * 4;
            let source_view_sequence = 2 + u64::from(step) * 2;
            let reserved_sequence = u64::from(step - superseded_prefix.min(step)) + 1;
            if let Fact::AstWork {
                aggregate_digest, source_operation, source_view, phase,
                published_view, superseded_by_aggregate_digest, ast_operation,
                ast_event_sequence, parser_registration, claim_owner_epoch,
                claim_fence, lease_expires_epoch_ms, request, ..
            } = &mut work {
                *aggregate_digest = aggregate;
                source_operation.sequence = source_operation_sequence;
                source_view.sequence = source_view_sequence;
                source_view.digest = Digest([15 + step; 32]);
                if step < superseded_prefix {
                    *phase = NativeCutoverAstWorkPhaseV1::Superseded;
                    *published_view = None;
                    *superseded_by_aggregate_digest = successor;
                    *ast_operation = None;
                    *ast_event_sequence = None;
                    *parser_registration = None;
                    *claim_owner_epoch = None;
                    *claim_fence = None;
                    *lease_expires_epoch_ms = None;
                    *request = None;
                } else {
                    let operation_sequence = reserved_sequence;
                    let event_sequence = reserved_sequence;
                    ast_operation.as_mut().expect("AST operation").sequence = operation_sequence;
                    *ast_event_sequence = Some(event_sequence);
                    published_view.as_mut().expect("AST Published view").sequence =
                        source_view_sequence + 1;
                    published_view.as_mut().expect("AST Published view").digest =
                        Digest([30 + step; 32]);
                    let bound = request.as_mut().expect("AST bound request");
                    bound.operation.sequence = operation_sequence;
                    bound.expected_view = *source_view;
                    let UpdateKind::ProjectionAdvance(transition) = &mut bound.kind else {
                        panic!("AST projection template");
                    };
                    let event = transition.events.first_mut().expect("AST event");
                    event.predecessor = event_sequence - 1;
                    event.sequence = event_sequence;
                    let payload = transition.payload_digest().expect("AST canonical payload");
                    transition.events.first_mut().expect("AST event").payload = payload;
                }
            }
            evidence.source_facts.push(work);
        }
        evidence.source_facts.sort_by(|left, right|
            mixed_rank(left).cmp(&mixed_rank(right))
                .then_with(|| mixed_key(left).cmp(&mixed_key(right))));
        evidence
    }

    #[test]
    fn native_ast_history_selects_latest_published_and_resolves_superseded_chain() {
        for successors in [1, 2] {
            let evidence = settled_native_ast_history(successors, 0);
            let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
                .expect("all historical and current AST Published rows settle");
            assert_eq!(joined.ast_update_request_v1().unwrap().operation.sequence,
                u64::from(successors) + 1);
        }
        let evidence = settled_native_ast_history(2, 2);
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("strictly later successor chain ends at Published");
        assert_eq!(joined.ast_update_request_v1().unwrap().operation.sequence, 1);
    }

    #[test]
    fn new_initial_full_accepts_ast_history_without_projecting_an_old_root() {
        let mut evidence = legacy_m_at_new_initial_root();
        evidence.source_facts.extend(
            settled_native_ast_history(1, 0)
                .source_facts
                .into_iter()
                .filter(|fact| matches!(fact, Fact::AstFrontier { .. } | Fact::AstWork { .. })),
        );
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("settled historical AST and new initial FULL without AST");
        assert!(joined.ast_frontier.is_none());
        assert!(joined.ast_update_request_v1().is_none());
        assert!(joined.current_legacy_sdk_ack.is_some());

        let old_publication = evidence
            .source_facts
            .iter_mut()
            .find_map(|fact| match fact {
                Fact::AstWork { published_view, .. } => Some(published_view),
                _ => None,
            })
            .expect("historical AST work");
        *old_publication = None;
        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "Published AST has no terminal view"
            ))
        ));
    }

    #[test]
    fn new_initial_full_refuses_duplicate_or_reversed_historical_ast_views() {
        let mut evidence = legacy_m_at_new_initial_root();
        evidence.source_facts.extend(
            settled_native_ast_history(1, 0)
                .source_facts
                .into_iter()
                .filter(|fact| matches!(fact, Fact::AstFrontier { .. } | Fact::AstWork { .. })),
        );
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("other publications may advance the view between AST events");
        assert!(joined.ast_update_request_v1().is_none());

        for first_sequence in [5, 6] {
            let first_view = evidence
                .source_facts
                .iter_mut()
                .find_map(|fact| match fact {
                    Fact::AstWork {
                        ast_event_sequence: Some(1),
                        published_view: Some(view),
                        ..
                    } => Some(view),
                    _ => None,
                })
                .expect("first historical AST publication");
            first_view.sequence = first_sequence;

            assert!(matches!(
                join_test(&evidence, &mut |_| Ok::<_, ()>(())),
                Err(NativeCutoverSourceJoinErrorV1::Refused(
                    "Published AST view does not advance in root event order"
                ))
            ));
        }
    }

    #[test]
    fn native_ast_history_rejects_missing_cycle_active_foreign_and_unpaid_work() {
        let refuse = |evidence: &NativeCutoverEvidenceV1| {
            assert!(matches!(join_test(evidence, &mut |_| Ok::<_, ()>(())),
                Err(NativeCutoverSourceJoinErrorV1::Refused(_))));
        };
        let mut missing = settled_native_ast_history(2, 2);
        if let Some(Fact::AstWork { superseded_by_aggregate_digest, .. }) = missing.source_facts
            .iter_mut().find(|fact| matches!(fact, Fact::AstWork {
                aggregate_digest, .. } if aggregate_digest == &"9".repeat(64))) {
            *superseded_by_aggregate_digest = Some("f".repeat(64));
        }
        refuse(&missing);
        let mut cycle = settled_native_ast_history(2, 2);
        if let Some(Fact::AstWork { superseded_by_aggregate_digest, .. }) = cycle.source_facts
            .iter_mut().find(|fact| matches!(fact, Fact::AstWork {
                aggregate_digest, .. } if aggregate_digest == &"a".repeat(64))) {
            *superseded_by_aggregate_digest = Some("9".repeat(64));
        }
        refuse(&cycle);
        let mut active = settled_native_ast_history(1, 0);
        if let Some(Fact::AstWork { phase, .. }) = active.source_facts.iter_mut()
            .find(|fact| matches!(fact, Fact::AstWork {
                aggregate_digest, .. } if aggregate_digest == &"a".repeat(64))) {
            *phase = NativeCutoverAstWorkPhaseV1::Bound;
        }
        refuse(&active);
        let mut foreign = settled_native_ast_history(1, 0);
        if let Some(Fact::AstWork { source_operation, .. }) = foreign.source_facts.iter_mut()
            .find(|fact| matches!(fact, Fact::AstWork {
                aggregate_digest, .. } if aggregate_digest == &"a".repeat(64))) {
            source_operation.sequence = 99;
        }
        refuse(&foreign);
        let mut wrong_aggregate = settled_native_ast_history(1, 0);
        if let Some(Fact::AstWork { aggregate_digest, .. }) = wrong_aggregate.source_facts
            .iter_mut().find(|fact| matches!(fact, Fact::AstWork {
                aggregate_digest, .. } if aggregate_digest == &"a".repeat(64))) {
            *aggregate_digest = "f".repeat(64);
        }
        refuse(&wrong_aggregate);
        let mut stale_source = settled_native_ast_history(1, 0);
        if let Some(Fact::AstWork { source_view, .. }) = stale_source.source_facts
            .iter_mut().find(|fact| matches!(fact, Fact::AstWork {
                aggregate_digest, .. } if aggregate_digest == &"a".repeat(64))) {
            source_view.sequence = 1;
        }
        refuse(&stale_source);
        let mut zero_lexical_member = settled_native_ast_history(1, 0);
        if let Some(Fact::AstWork { lexical_artifact_digest, .. }) =
            zero_lexical_member.source_facts.iter_mut().find(|fact|
                matches!(fact, Fact::AstWork { aggregate_digest, .. }
                    if aggregate_digest == &"a".repeat(64))) {
            *lexical_artifact_digest = Digest([0; 32]);
        }
        refuse(&zero_lexical_member);
        let evidence = settled_native_ast_history(2, 1);
        assert!(matches!(join_test(&evidence, &mut |_| Err::<(), _>("AST work cap")),
            Err(NativeCutoverSourceJoinErrorV1::Work("AST work cap"))));
    }

    #[test]
    fn native_two_and_three_event_groups_join_prior_legacy_and_current_sdk_m() {
        for successors in [1, 2] {
            let evidence = settled_native_successor_evidence(successors);
            let mut reservations = Vec::with_capacity(evidence.source_facts.len());
            let mut replays = Vec::with_capacity(evidence.source_facts.len());
            let joined = join_native_cutover_source_facts_v1(&evidence,
                &mut reservations, &mut replays, &mut |_| Ok::<_, ()>(()))
                .expect("settled Native SDK lineage and historical Legacy M");
            assert_eq!(joined.baseline.operation.sequence, 7);
            assert_eq!(joined.current_native_target.operation.sequence,
                7 + u64::from(successors) * 4);
            assert_eq!(joined.current_native_update_request_v1().unwrap().operation,
                joined.current_native_target.operation);
            assert!(joined.prior_sdk_acked_binding.is_some());
            assert!(joined.sdk_acked_binding.is_some());
        }
        let mut evidence = settled_native_successor_evidence(1);
        evidence.source_facts.push(settled_outbox_fixture("Lexical"));
        evidence.source_facts.sort_by(|left, right| mixed_rank(left).cmp(&mixed_rank(right))
            .then_with(|| mixed_key(left).cmp(&mixed_key(right))));
        let mut reservations = Vec::with_capacity(evidence.source_facts.len());
        let mut replays = Vec::with_capacity(evidence.source_facts.len());
        let _joined = join_native_cutover_source_facts_v1(&evidence, &mut reservations, &mut replays,
            &mut |_| Ok::<_, ()>(())).expect("settled historical outbox is joined once");
    }

    #[test]
    fn native_join_refuses_a_stale_v5_within_the_selected_root() {
        let mut evidence = settled_native_successor_evidence(2);
        let stale = settled_native_successor_evidence(1);
        evidence.fresh_sdk = stale.fresh_sdk;
        evidence.v5_artifacts = stale.v5_artifacts;

        assert!(matches!(
            join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "current V5 absent"
            ))
        ));
    }

    #[test]
    fn historical_v5_rows_require_unique_exact_native_v9_and_keep_current_selection() {
        let mut evidence = settled_native_successor_evidence(2);
        let mut older = settled_native_successor_evidence(1);
        let old_v5 = older.v5_artifacts.remove(0);
        let old_operation = old_v5.operation;
        evidence.v5_artifacts.insert(0, old_v5);
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("bounded old V5 is exact Native V9 DATA");
        assert_eq!(joined.v5_artifact_for_operation_v1(old_operation).unwrap().operation, old_operation);
        assert_eq!(joined.v5_artifact.operation, joined.current_native_target.operation);

        let mut duplicate = evidence.clone();
        duplicate.v5_artifacts.insert(1, duplicate.v5_artifacts[0].clone());
        assert!(matches!(join_test(&duplicate, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));
        let mut foreign_aggregate = evidence.clone();
        foreign_aggregate.v5_artifacts[0].aggregate_receipt_digest = NativeCutoverDigest64V1([b'9'; 64]);
        assert!(matches!(join_test(&foreign_aggregate, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused("V5 artifact differs from its Native V9"))));
        let mut orphan = evidence;
        orphan.v5_artifacts[0].operation.sequence += 1;
        assert!(matches!(join_test(&orphan, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));
    }

    fn current_native_sdk_source_fact(evidence: &NativeCutoverEvidenceV1) -> Fact {
        let current = evidence.v5_artifacts[0].operation;
        let aggregate = evidence.source_facts.iter().find_map(|fact| match fact {
            Fact::Replay { native_update_request: Some(request), aggregate_identity_digest, .. }
                if request.operation == current => Some(aggregate_identity_digest.clone()),
            _ => None,
        }).expect("current Native V9 aggregate");
        let binding = evidence.source_facts.iter().find_map(|fact| match fact {
            Fact::PendingStoreSdkAck { binding, .. }
                if binding.transition_digest.as_bytes()
                    == evidence.fresh_sdk.source_manifest_digest.0.as_slice() => Some(binding),
            _ => None,
        }).expect("current Native SDK ACK");
        let digest = |text: &str| NativeCutoverDigest64V1(
            text.as_bytes().try_into().expect("fixture digest64"));
        Fact::NativeSdkSourceBinding {
            operation: current,
            aggregate_identity_digest: aggregate,
            source: NativeCutoverEvidenceSourcePublicationBindingV1 {
                transition_digest: digest(&binding.transition_digest),
                completion_pointer_digest: digest(&binding.completion_pointer_digest),
                physical_target_source_digest: digest(&binding.target_source_digest),
                target_snapshot_id: binding.target_snapshot,
            },
        }
    }

    #[test]
    fn native_sdk_source_binding_joins_exact_ack_and_v9_without_creating_another_publication() {
        let mut evidence = settled_native_successor_evidence(1);
        let sdk_source = current_native_sdk_source_fact(&evidence);
        evidence.source_facts.push(sdk_source);
        let operation = evidence.v5_artifacts[0].operation;
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("actual SDK ACK four axes bind settled Native V9");
        assert_eq!(joined.native_sdk_source_for_operation_v1(operation)
            .expect("borrowed SDK Source association").1,
            evidence.source_facts.iter().find_map(|fact| match fact {
                Fact::Replay { native_update_request: Some(request), aggregate_identity_digest, .. }
                    if request.operation == operation => Some(aggregate_identity_digest.as_str()),
                _ => None,
            }).unwrap());
        let mut malformed = evidence.clone();
        if let Some(Fact::NativeSdkSourceBinding { source, .. }) = malformed.source_facts.last_mut() {
            source.transition_digest = NativeCutoverDigest64V1([b'g'; 64]);
        }
        assert!(matches!(join_test(&malformed, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused("Native SDK Source shape differs"))));
        let mut foreign = evidence.clone();
        if let Some(Fact::NativeSdkSourceBinding { source, .. }) = foreign.source_facts.last_mut() {
            source.transition_digest = NativeCutoverDigest64V1([b'9'; 64]);
        }
        assert!(matches!(join_test(&foreign, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused("Native SDK Source differs from exact SDK ACK/V9"))));
        let mut wrong_kind = evidence;
        let baseline_operation = wrong_kind.baseline.operation;
        if let Some(Fact::NativeSdkSourceBinding { operation, .. }) = wrong_kind.source_facts.last_mut() {
            *operation = baseline_operation;
        }
        assert!(matches!(join_test(&wrong_kind, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));
    }

    fn completed_watch_fact(
        evidence: &NativeCutoverEvidenceV1,
        sequence: u64,
        event_ids: Vec<u128>,
    ) -> Fact {
        let (operation, snapshot) = evidence.source_facts.iter().find_map(|fact| match fact {
            Fact::Replay {
                native_update_request: Some(request),
                published_snapshot_id: Some(snapshot),
                ..
            } if request.operation.sequence == sequence => Some((request.operation, *snapshot)),
            _ => None,
        }).expect("Native terminal Watch V9");
        Fact::CompletedSourceRequest {
            native_operation: Some(operation),
            source: NativeCutoverEvidenceSourcePublicationBindingV1 {
                transition_digest: NativeCutoverDigest64V1([b'a'; 64]),
                completion_pointer_digest: NativeCutoverDigest64V1([b'b'; 64]),
                physical_target_source_digest: NativeCutoverDigest64V1([b'c'; 64]),
                target_snapshot_id: snapshot,
            },
            event_ids,
        }
    }

    fn historical_watch_evidence() -> NativeCutoverEvidenceV1 {
        let mut evidence = settled_native_successor_evidence(2);
        let prior_target_digest = evidence.baseline.canonical_target_digest;
        if let Some(Fact::Reservation { evidence: kind, .. }) = evidence.source_facts.iter_mut()
            .find(|fact| matches!(fact, Fact::Reservation { target, .. }
                if target.operation.sequence == 11)) {
            *kind = NativeCutoverReservationEvidenceV1::Watch {
                idempotency_key: "watch-1".into(),
                verified_batch_digest: Digest([7; 32]),
                prior_target_digest,
            };
        }
        evidence.source_facts.retain(|fact| !matches!(fact,
            Fact::PendingStoreSdkAck { plan_id, .. } if plan_id == "np1"));
        evidence.v5_artifacts[0].previous_transition_digest =
            Some(NativeCutoverDigest64V1([b'e'; 64]));
        let completed = completed_watch_fact(&evidence, 11, vec![17, 19]);
        evidence.source_facts.push(completed);
        evidence
    }

    #[test]
    fn source_only_completed_request_has_no_native_publication_and_global_event_ids_are_unique() {
        let mut evidence = historical_watch_evidence();
        let mut source_only = completed_watch_fact(&evidence, 11, vec![31, 33]);
        let Fact::CompletedSourceRequest { native_operation, source, .. } = &mut source_only else {
            unreachable!()
        };
        *native_operation = None;
        source.transition_digest = NativeCutoverDigest64V1([b'1'; 64]);
        evidence.source_facts.insert(evidence.source_facts.len() - 1, source_only);
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("signed Source-only ancestry has no fabricated Native reservation");
        assert_eq!(joined.completed_source_request_for_event_v1(31, &mut |_| Ok::<_, ()>(())).unwrap()
            .expect("exact Source-only event membership").2, None);
        assert!(joined.completed_source_request_for_event_v1(19, &mut |_| Ok::<_, ()>(())).unwrap()
            .expect("actual Native Watch event membership").2.is_some());
        assert!(joined.completed_source_request_for_event_v1(34, &mut |_| Ok::<_, ()>(())).unwrap().is_none());
        assert_eq!(joined.completed_source_request_for_event_v1(31, &mut |_| Err::<(), _>("event lookup cap")),
            Err("event lookup cap"));

        let mut duplicate = evidence;
        if let Some(Fact::CompletedSourceRequest { event_ids, .. }) = duplicate.source_facts.iter_mut()
            .find(|fact| matches!(fact, Fact::CompletedSourceRequest { native_operation: None, .. })) {
            *event_ids = vec![19, 33];
        }
        assert!(matches!(join_test(&duplicate, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "completed Watch event ID is duplicated across operations"))));
    }

    #[test]
    fn historical_watch_can_precede_current_sdk_but_current_watch_refuses() {
        let mut evidence = historical_watch_evidence();
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("historical Watch does not manufacture an M ACK");
        let operation = evidence.source_facts.iter().find_map(|fact| match fact {
            Fact::CompletedSourceRequest { native_operation: Some(operation), .. } => Some(*operation),
            _ => None,
        }).expect("completed Watch operation");
        assert_eq!(joined.completed_watch_source_for_operation_v1(operation)
            .expect("borrowed completed Watch source").1, &[17, 19]);
        if let Some(Fact::Reservation { evidence: kind, .. }) = evidence.source_facts.iter_mut()
            .find(|fact| matches!(fact, Fact::Reservation { target, .. }
                if target.operation.sequence == 15)) {
            *kind = NativeCutoverReservationEvidenceV1::Watch {
                idempotency_key: "watch-2".into(),
                verified_batch_digest: Digest([8; 32]),
                prior_target_digest: Digest([11; 32]),
            };
        }
        evidence.source_facts.retain(|fact| !matches!(fact,
            Fact::PendingStoreSdkAck { plan_id, .. } if plan_id == "np2"));
        let current_completed = completed_watch_fact(&evidence, 15, vec![23]);
        evidence.source_facts.push(current_completed);
        assert!(matches!(join_test(&evidence, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(_))));
    }

    #[test]
    fn completed_watch_source_requires_exact_bijective_terminal_and_unique_event_ids() {
        let mut missing = historical_watch_evidence();
        missing.source_facts.retain(|fact| !matches!(fact,
            Fact::CompletedSourceRequest { .. }));
        assert!(matches!(join_test(&missing, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused("completed Watch Source absent"))));

        let mut duplicate = historical_watch_evidence();
        let completed = completed_watch_fact(&duplicate, 11, vec![23]);
        duplicate.source_facts.push(completed);
        assert!(matches!(join_test(&duplicate, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "completed Watch operation is duplicated"))));

        let mut foreign = historical_watch_evidence();
        if let Some(Fact::CompletedSourceRequest { native_operation: Some(operation), .. }) = foreign.source_facts
            .iter_mut().find(|fact| matches!(fact, Fact::CompletedSourceRequest { .. })) {
            operation.sequence = 12;
        }
        assert!(matches!(join_test(&foreign, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "completed Watch Source operation differs"))));

        let mut non_watch = settled_native_successor_evidence(1);
        let non_watch_completed = completed_watch_fact(&non_watch, 11, vec![17]);
        non_watch.source_facts.push(non_watch_completed);
        assert!(matches!(join_test(&non_watch, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "completed Watch Source has no Watch reservation"))));

        let mut wrong_snapshot = historical_watch_evidence();
        if let Some(Fact::CompletedSourceRequest { source, .. }) = wrong_snapshot.source_facts
            .iter_mut().find(|fact| matches!(fact, Fact::CompletedSourceRequest { .. })) {
            source.target_snapshot_id += 1;
        }
        assert!(matches!(join_test(&wrong_snapshot, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "completed Watch V9/Source publication differs"))));

        let mut shared_event = historical_watch_evidence();
        let second_completed = completed_watch_fact(&shared_event, 15, vec![19, 21]);
        shared_event.source_facts.push(second_completed);
        assert!(matches!(join_test(&shared_event, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "completed Watch event ID is duplicated across operations"))));
        let charged_watch = historical_watch_evidence();
        let watch_shape_work = 2 * std::mem::size_of::<u128>() + 3 * 64
            + std::mem::size_of::<u64>();
        assert!(matches!(join_test(&charged_watch, &mut |units| {
            if units == watch_shape_work as u64 { Err("Watch work cap") } else { Ok(()) }
        }),
            Err(NativeCutoverSourceJoinErrorV1::Work("Watch work cap"))));
    }

    #[test]
    fn native_group_join_refuses_foreign_m_missing_ack_and_active_ast() {
        let refuses = |evidence: &NativeCutoverEvidenceV1| {
            let mut reservations = Vec::with_capacity(evidence.source_facts.len());
            let mut replays = Vec::with_capacity(evidence.source_facts.len());
            assert!(matches!(join_native_cutover_source_facts_v1(evidence,
                &mut reservations, &mut replays, &mut |_| Ok::<_, ()>(())),
                Err(NativeCutoverSourceJoinErrorV1::Refused(_))));
        };
        let mut evidence = settled_native_successor_evidence(1);
        if let Some(Fact::Replay { closeout_id, .. }) = evidence.source_facts.iter_mut()
            .find(|fact| matches!(fact, Fact::Replay {
                native_update_request: Some(request), ..
            } if request.operation.sequence == 7)) {
            *closeout_id = "initial-native-full:not-a-digest".into();
        }
        refuses(&evidence);
        let mut evidence = settled_native_successor_evidence(1);
        evidence.v5_artifacts[0].previous_transition_digest =
            Some(NativeCutoverDigest64V1([b'0'; 64]));
        refuses(&evidence);
        let mut evidence = settled_native_successor_evidence(1);
        evidence.fresh_sdk.committed_source_snapshot_id += 1;
        refuses(&evidence);
        let mut evidence = settled_native_successor_evidence(1);
        evidence.fresh_sdk.committed_source_snapshot_digest = NativeCutoverDigest64V1([b'0'; 64]);
        refuses(&evidence);
        let mut evidence = settled_native_successor_evidence(1);
        evidence.source_facts.retain(|fact| !matches!(fact,
            Fact::PendingStoreSdkAck { plan_id, .. } if plan_id == "np1"));
        refuses(&evidence);
        let mut evidence = settled_native_successor_evidence(1);
        if let Some(Fact::PendingStoreSdkAck { binding, acked_binding, .. }) = evidence.source_facts
            .iter_mut().find(|fact| matches!(fact,
                Fact::PendingStoreSdkAck { plan_id, .. } if plan_id == "np1")) {
            binding.transition_digest = "0".repeat(64);
            *acked_binding = Some(binding.clone());
        }
        refuses(&evidence);
        let mut evidence = settled_native_successor_evidence(1);
        evidence.source_facts.retain(|fact| !matches!(fact,
            Fact::Replay { native_update_request: Some(request), .. }
                if request.operation.sequence == 11));
        refuses(&evidence);
        let mut evidence = settled_native_successor_evidence(1);
        if let Some(Fact::Replay { phase, .. }) = evidence.source_facts.iter_mut()
            .find(|fact| matches!(fact, Fact::Replay {
                native_update_request: Some(request), ..
            } if request.operation.sequence == 11)) {
            *phase = NativeCutoverReplayPhaseV1::Terminal;
        }
        refuses(&evidence);
        let mut evidence = settled_native_successor_evidence(1);
        evidence.source_facts.retain(|fact| !matches!(fact,
            Fact::PendingStoreCustody { plan_id, .. } if plan_id == "np1"));
        refuses(&evidence);
        let mut evidence = settled_native_successor_evidence(1);
        let duplicate = evidence.source_facts.iter().find(|fact| matches!(fact,
            Fact::PendingStoreAttempt { plan_id, .. } if plan_id == "np1"))
            .unwrap().clone();
        evidence.source_facts.push(duplicate);
        refuses(&evidence);
        let mut evidence = settled_native_successor_evidence(1);
        let full = evidence.baseline.operation;
        evidence.source_facts.push(Fact::AstFrontier {
            candidate_root_nonce: Some(Digest([22; 32])),
            workspace: full.workspace, workspace_epoch: full.epoch,
            producer: NATIVE_STUB_AST_PRODUCER,
            stream: StreamId(*b"smca.ast.strm.v1"), stream_epoch: 1,
            next_operation_sequence: 1, committed_event_sequence: 0,
        });
        refuses(&evidence);
        let evidence = settled_native_successor_evidence(1);
        let mut reservations = Vec::with_capacity(evidence.source_facts.len());
        let mut replays = Vec::with_capacity(evidence.source_facts.len());
        assert!(matches!(join_native_cutover_source_facts_v1(&evidence,
            &mut reservations, &mut replays, &mut |_| Err::<(), _>("work cap")),
            Err(NativeCutoverSourceJoinErrorV1::Work("work cap"))));
    }

    fn settled_outbox_fixture(track: &str) -> Fact {
        let lexical = track == "Lexical";
        let dirty = track == "Dirty";
        // Fixed vectors from the S V2 length-prefixed BLAKE3 dispatch domain:
        // repo, historical-revision, generation 7, identity manifest a*64.
        // The auxiliary Semantic/Dirty binding b*64 is a separate domain.
        let (dispatch_id, delivery_manifest) = match track {
            "Lexical" => ("search-plane:v2:edc78b7c3219d8c669c497b61a9fcd2d4bdd6162cd76e28ce2a63c0a4682e6ea", "a"),
            "Structural" => ("search-plane:v2:3d1e3085c05659b8f337b4c89ad0f6e6e165f26113611efb825303d74057dc6c", "a"),
            "Semantic" => ("search-plane:v2:8a69d2a25137f6e11b3d44c9ee2cc7ee9bdcd3e0e19297654501e90791a7d6b3", "b"),
            "Dirty" => ("search-plane:v2:65e169b0f66de6200477d90e083038e85cddc6a69cd36dd8827abd9f4b86369a", "b"),
            _ => unreachable!("fixed outbox track"),
        };
        Fact::Outbox {
            // Production validates these IDs under the signed S Original;
            // this fixed-vector fixture isolates Q's settled projection.
            dispatch_id: dispatch_id.into(),
            track: track.into(),
            repo_id: "repo".into(),
            revision_id: "historical-revision".into(),
            state: "Delivered".into(),
            attempt_count: 1,
            publish_binding: Some(NativeCutoverEvidenceOutboxPublishBindingV1 {
                generation: 7,
                manifest_digest_hex: delivery_manifest.repeat(64),
            }),
            delivery_receipt: Some(NativeCutoverEvidenceOutboxDeliveryReceiptV1 {
                generation: 7,
                manifest_digest_hex: delivery_manifest.repeat(64),
                accepted_replace_scopes: 1,
                accepted_tombstone_scopes: 0,
                accepted_semantic_replace_scopes: u32::from(lexical),
                accepted_semantic_tombstone_scopes: 0,
                sealed: !dirty,
                semantic_admission_metrics: lexical.then_some(
                    NativeCutoverEvidenceOutboxSemanticAdmissionMetricsV1 {
                        admitted_symbol_card_count: 1,
                        excluded_generated_source_count: 0,
                        excluded_vendor_source_count: 0,
                        excluded_trivial_accessor_count: 0,
                        excluded_admission_facts_unavailable_count: 0,
                        excluded_unsupported_identity_count: 0,
                        raw_fallback_scope_count: 1,
                        raw_fallback_token_count: 10,
                        total_code_token_count: 100,
                        raw_fallback_ratio_bps: 1_000,
                    },
                ),
            }),
            op_count_hint: 1,
            semantic_replace_count_hint: u32::from(lexical),
            semantic_tombstone_count_hint: 0,
        }
    }

    #[test]
    fn settled_historical_outbox_is_admitted_without_payload_and_sorted_in_mixed_census() {
        let native = native_only_initial_full_evidence();
        let mut with_outbox = native.clone();
        let insertion = with_outbox
            .source_facts
            .iter()
            .position(|fact| matches!(fact, Fact::PendingStoreCheckpoint { .. }))
            .expect("checkpoint category follows outbox");
        let mut rows = ["Dirty", "Lexical", "Semantic", "Structural"]
            .map(settled_outbox_fixture);
        rows.sort_by(|left, right| {
            let Fact::Outbox { dispatch_id: left, .. } = left else { unreachable!() };
            let Fact::Outbox { dispatch_id: right, .. } = right else { unreachable!() };
            left.cmp(right)
        });
        drop(with_outbox.source_facts.splice(insertion..insertion, rows));
        let _joined = join_test(&with_outbox, &mut |_| Ok::<_, ()>(()))
            .expect("settled historical rows need no payload file or current revision");

        let (_, mut mixed) = signed_mixed_legacy_fixture();
        let insertion = mixed
            .source_facts
            .iter()
            .position(|fact| matches!(fact, Fact::PendingStoreAttempt { .. }))
            .expect("pending store category follows outbox");
        let mut rows = ["Dirty", "Lexical", "Semantic", "Structural"]
            .map(settled_outbox_fixture);
        rows.sort_by(|left, right| {
            let Fact::Outbox { dispatch_id: left, .. } = left else { unreachable!() };
            let Fact::Outbox { dispatch_id: right, .. } = right else { unreachable!() };
            left.cmp(right)
        });
        drop(mixed.source_facts.splice(insertion..insertion, rows));
        let native_join = join_test(&native, &mut |_| Ok::<_, ()>(()))
            .expect("Native initial FULL");
        let _current_legacy = join_mixed_legacy(&mixed, &native_join, &mut |_| Ok::<_, ()>(()))
            .expect("sorted settled outbox category participates in mixed census");
        // The signed Source visits PendingReplay before the SQL-ordered outbox.
        // Reversing those categories must still fail the receiver's order guard.
        let mut wrong_order = mixed.clone();
        let pending = wrong_order.source_facts.iter().position(|fact| matches!(fact, Fact::PendingReplay { .. }))
            .expect("pending replay");
        let outbox = wrong_order.source_facts.iter().position(|fact| matches!(fact, Fact::Outbox { .. }))
            .expect("outbox");
        wrong_order.source_facts.swap(pending, outbox);
        assert!(matches!(
            join_mixed_legacy(&wrong_order, &native_join, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused("mixed Source inventory order differs"))
        ));
    }

    #[test]
    fn settled_outbox_refuses_missing_or_mismatched_authority_and_work_cap() {
        for track in ["Lexical", "Structural", "Semantic", "Dirty"] {
            join_settled_outbox(&settled_outbox_fixture(track), "repo", &mut |_| Ok::<_, ()>(()))
                .expect("four settled track policies");
        }
        let refuses = |row: &Fact| {
            assert!(matches!(
                join_settled_outbox(row, "repo", &mut |_| Ok::<_, ()>(())),
                Err(NativeCutoverSourceJoinErrorV1::Refused(_))
            ));
        };
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { state, .. } = &mut row { *state = "InFlight".into(); }
        refuses(&row);
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { state, .. } = &mut row { *state = "Aborted".into(); }
        refuses(&row);
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { track, .. } = &mut row { *track = "Unknown".into(); }
        refuses(&row);
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { repo_id, .. } = &mut row { *repo_id = "foreign".into(); }
        refuses(&row);
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { revision_id, .. } = &mut row { revision_id.clear(); }
        refuses(&row);
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { dispatch_id, .. } = &mut row { dispatch_id.clear(); }
        refuses(&row);
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { publish_binding, .. } = &mut row { *publish_binding = None; }
        refuses(&row);
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { delivery_receipt, .. } = &mut row { *delivery_receipt = None; }
        refuses(&row);
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { op_count_hint, .. } = &mut row { *op_count_hint = 2; }
        refuses(&row);
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { publish_binding: Some(binding), .. } = &mut row {
            binding.generation = 8;
        }
        refuses(&row);
        let mut row = settled_outbox_fixture("Dirty");
        if let Fact::Outbox { delivery_receipt: Some(receipt), .. } = &mut row {
            receipt.manifest_digest_hex = "c".repeat(64);
        }
        refuses(&row);
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { delivery_receipt: Some(receipt), .. } = &mut row {
            receipt.sealed = false;
        }
        refuses(&row);
        let mut row = settled_outbox_fixture("Dirty");
        if let Fact::Outbox { delivery_receipt: Some(receipt), .. } = &mut row {
            receipt.sealed = true;
        }
        refuses(&row);
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { delivery_receipt: Some(receipt), .. } = &mut row {
            receipt.semantic_admission_metrics.as_mut().expect("metrics").raw_fallback_ratio_bps = 999;
        }
        refuses(&row);
        let mut row = settled_outbox_fixture("Lexical");
        if let Fact::Outbox { state, .. } = &mut row { *state = "Aborted".into(); }
        assert!(matches!(
            join_settled_outbox(&row, "repo", &mut |_| Err::<(), _>("work cap")),
            Err(NativeCutoverSourceJoinErrorV1::Work("work cap"))
        ));
    }

    #[test]
    fn ordinary_initial_legacy_selects_signed_replay_without_pending_sdk_ack() {
        let evidence = ordinary_initial_legacy_fixture();
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("ordinary initial Legacy M has an exact signed Source association");
        assert!(matches!(joined.current_legacy_replay, Some(Fact::Replay {
            closeout_id, native_update_request: None, phase: NativeCutoverReplayPhaseV1::Acked,
            ..
        }) if closeout_id == "ordinary-initial-full"));
        assert!(joined.current_legacy_sdk_ack.is_none());
        assert_eq!(joined.current_legacy_ordinary_source_binding_v1(),
            Some(&NativeCutoverEvidenceSourcePublicationBindingV1 {
                transition_digest: NativeCutoverDigest64V1([b'e'; 64]),
                completion_pointer_digest: NativeCutoverDigest64V1([b'd'; 64]),
                physical_target_source_digest: NativeCutoverDigest64V1([b'f'; 64]),
                target_snapshot_id: 17,
            }));
    }

    #[test]
    fn ordinary_initial_legacy_keeps_distinct_history_with_shared_source_and_snapshot() {
        let mut evidence = ordinary_initial_legacy_fixture();
        let mut historical_replay = evidence.source_facts.iter().find(|fact| matches!(fact,
            Fact::Replay { native_update_request: None, .. }
        )).expect("current ordinary V9").clone();
        let mut historical_checkpoint = evidence.source_facts.iter().find(|fact| matches!(fact,
            Fact::PendingStoreCheckpoint { aggregate_identity_digest, .. }
                if aggregate_identity_digest == &"1".repeat(64)
        )).expect("current ordinary checkpoint").clone();
        if let Fact::Replay {
            aggregate_identity_digest,
            ordinary_source_binding: Some(binding),
            ..
        } = &mut historical_replay {
            *aggregate_identity_digest = "0".repeat(64);
            binding.transition_digest = NativeCutoverDigest64V1([b'a'; 64]);
            binding.completion_pointer_digest = NativeCutoverDigest64V1([b'b'; 64]);
            // The signed historical M can have the same physical bytes and
            // snapshot while its manifest and pointer are distinct.
            assert_eq!(binding.physical_target_source_digest, NativeCutoverDigest64V1([b'f'; 64]));
            assert_eq!(binding.target_snapshot_id, 17);
        } else {
            unreachable!("ordinary V9 binding");
        }
        if let Fact::PendingStoreCheckpoint { aggregate_identity_digest, .. } = &mut historical_checkpoint {
            *aggregate_identity_digest = "0".repeat(64);
        }
        evidence.source_facts.insert(2, historical_replay);
        let checkpoint_start = evidence.source_facts.iter().position(|fact|
            matches!(fact, Fact::PendingStoreCheckpoint { .. })).expect("checkpoint range");
        evidence.source_facts.insert(checkpoint_start, historical_checkpoint);
        let joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("distinct historical M is retained without replacing current M");
        assert!(matches!(joined.current_legacy_replay, Some(Fact::Replay {
            aggregate_identity_digest, ..
        }) if aggregate_identity_digest == &"1".repeat(64)));
        assert_eq!(joined.current_legacy_ordinary_source_binding_v1()
            .expect("current ordinary binding").transition_digest,
            NativeCutoverDigest64V1([b'e'; 64]));
    }

    #[test]
    fn ordinary_initial_legacy_refuses_absent_partial_and_foreign_source_axes() {
        let mut missing = ordinary_initial_legacy_fixture();
        for fact in &mut missing.source_facts {
            if let Fact::Replay { native_update_request: None, ordinary_source_binding, .. } = fact {
                *ordinary_source_binding = None;
            }
        }
        assert!(matches!(join_test(&missing, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused("current Legacy SDK M ACK is absent"))));

        for axis in 0..4 {
            let mut foreign = ordinary_initial_legacy_fixture();
            let binding = ordinary_initial_binding_mut(&mut foreign);
            let expected = match axis {
                0 => {
                    binding.transition_digest.0 = [b'a'; 64];
                    "ordinary Source pointer names another transition"
                }
                1 => {
                    binding.completion_pointer_digest.0 = [b'a'; 64];
                    "ordinary Source binding partially matches fresh Source"
                }
                2 => {
                    binding.physical_target_source_digest.0 = [b'a'; 64];
                    "ordinary Source binding partially matches fresh Source"
                }
                3 => {
                    binding.target_snapshot_id += 1;
                    "ordinary Source binding is partial or differs from V9"
                }
                _ => unreachable!(),
            };
            assert!(matches!(join_test(&foreign, &mut |_| Ok::<_, ()>(())),
                Err(NativeCutoverSourceJoinErrorV1::Refused(reason)) if reason == expected));
        }
        let mut malformed = ordinary_initial_legacy_fixture();
        ordinary_initial_binding_mut(&mut malformed).transition_digest.0[0] = b'G';
        assert!(matches!(join_test(&malformed, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "ordinary Source binding is partial or differs from V9"))));
    }

    #[test]
    fn ordinary_initial_legacy_refuses_duplicate_and_conflicting_selection() {
        let mut duplicate = ordinary_initial_legacy_fixture();
        let mut replay = duplicate.source_facts.iter().find(|fact| matches!(fact,
            Fact::Replay { native_update_request: None, .. }
        )).expect("ordinary V9").clone();
        let mut checkpoint = duplicate.source_facts.iter().find(|fact| matches!(fact,
            Fact::PendingStoreCheckpoint { aggregate_identity_digest, .. }
                if aggregate_identity_digest == &"1".repeat(64)
        )).expect("ordinary checkpoint").clone();
        if let Fact::Replay { aggregate_identity_digest, .. } = &mut replay {
            *aggregate_identity_digest = "0".repeat(64);
        }
        if let Fact::PendingStoreCheckpoint { aggregate_identity_digest, .. } = &mut checkpoint {
            *aggregate_identity_digest = "0".repeat(64);
        }
        duplicate.source_facts.insert(2, replay);
        let checkpoint_start = duplicate.source_facts.iter().position(|fact|
            matches!(fact, Fact::PendingStoreCheckpoint { .. })).expect("checkpoint range");
        duplicate.source_facts.insert(checkpoint_start, checkpoint);
        assert!(matches!(join_test(&duplicate, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "multiple current ordinary Source bindings remain"))));

        let native = native_only_initial_full_evidence();
        let native_join = join_test(&native, &mut |_| Ok::<_, ()>(()))
            .expect("Native selected M");
        let mut conflict = ordinary_initial_legacy_fixture();
        conflict.legacy_publication = None;
        assert!(matches!(join_mixed_legacy(&conflict, &native_join, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "ordinary Source M conflicts with selected Native M"))));

        let (_, mut two_producers) = signed_mixed_legacy_fixture();
        let ordinary = ordinary_initial_legacy_fixture();
        let mut ordinary_replay = ordinary.source_facts.iter().find(|fact| matches!(fact,
            Fact::Replay { native_update_request: None, .. }
        )).expect("ordinary replay").clone();
        let mut ordinary_checkpoint = ordinary.source_facts.iter().find(|fact| matches!(fact,
            Fact::PendingStoreCheckpoint { aggregate_identity_digest, .. }
                if aggregate_identity_digest == &"1".repeat(64)
        )).expect("ordinary checkpoint").clone();
        if let Fact::Replay { aggregate_identity_digest, .. } = &mut ordinary_replay {
            *aggregate_identity_digest = "0".repeat(64);
        }
        if let Fact::PendingStoreCheckpoint { aggregate_identity_digest, .. } = &mut ordinary_checkpoint {
            *aggregate_identity_digest = "0".repeat(64);
        }
        two_producers.source_facts.insert(2, ordinary_replay);
        let checkpoint_start = two_producers.source_facts.iter().position(|fact|
            matches!(fact, Fact::PendingStoreCheckpoint { .. })).expect("checkpoint range");
        two_producers.source_facts.insert(checkpoint_start, ordinary_checkpoint);
        assert!(matches!(join_mixed_legacy(&two_producers, &native_join, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "current Legacy M has two producer associations"))));

        let mut disguised = ordinary_initial_legacy_fixture();
        let binding = *ordinary_initial_binding_mut(&mut disguised);
        if let Some(Fact::Replay { ordinary_source_binding, .. }) = disguised.source_facts.iter_mut()
            .find(|fact| matches!(fact, Fact::Replay { native_update_request: Some(_), .. })) {
            *ordinary_source_binding = Some(binding);
        }
        assert!(matches!(join_test(&disguised, &mut |_| Ok::<_, ()>(())),
            Err(NativeCutoverSourceJoinErrorV1::Refused(
                "Native V9 carries an ordinary Source binding"))));
    }

    #[test]
    fn mixed_legacy_current_m_joins_exact_ack_and_multiple_settled_history_rows() {
        let (native, mut mixed) = signed_mixed_legacy_fixture();
        let native_join = join_test(&native, &mut |_| Ok::<_, ()>(()))
            .expect("Native first FULL fixture");
        let selection = join_mixed_legacy(&mixed, &native_join, &mut |_| Ok::<_, ()>(()))
            .expect("Legacy history joins")
            .expect("current Legacy M joins one completed SDK row");
        let LegacyCurrentSelectionV1::ChangeEvent { replay, sdk_ack } = selection else {
            panic!("fixture selects the change-event ACK");
        };
        assert!(matches!(
            replay,
            Fact::Replay {
                native_update_request: None,
                ..
            }
        ));
        assert!(matches!(sdk_ack, Fact::PendingStoreSdkAck { .. }));

        // An independently ACKed historical V9 may have no change-event
        // carrier. Its signed checkpoint must still be retained and joined.
        let mut old = mixed
            .source_facts
            .iter()
            .find(|f| {
                matches!(
                    f,
                    Fact::Replay {
                        native_update_request: None,
                        ..
                    }
                )
            })
            .expect("Legacy V9")
            .clone();
        if let Fact::Replay {
            aggregate_identity_digest,
            closeout_id,
            attempt_id,
            ..
        } = &mut old
        {
            *aggregate_identity_digest = "0".repeat(64);
            *closeout_id = "prior-legacy-full".into();
            *attempt_id = "prior-legacy-attempt".into();
        }
        let mut old_ack = mixed
            .source_facts
            .iter()
            .find(|f| {
                matches!(f, Fact::PendingStoreCheckpoint {
                aggregate_identity_digest, .. } if aggregate_identity_digest == &"1".repeat(64))
            })
            .expect("Legacy checkpoint")
            .clone();
        if let Fact::PendingStoreCheckpoint {
            aggregate_identity_digest,
            ..
        } = &mut old_ack
        {
            *aggregate_identity_digest = "0".repeat(64);
        }
        mixed.source_facts.insert(2, old);
        let checkpoint_start = mixed
            .source_facts
            .iter()
            .position(|f| matches!(f, Fact::PendingStoreCheckpoint { .. }))
            .expect("checkpoint range");
        mixed.source_facts.insert(checkpoint_start, old_ack);
        let _current_legacy = join_mixed_legacy(&mixed, &native_join, &mut |_| Ok::<_, ()>(()))
            .expect("multiple historical Legacy replays are consumed without a history cap");
    }

    #[test]
    fn mixed_legacy_refuses_missing_m_active_phase_orphan_and_work_cap() {
        let (native, mixed) = signed_mixed_legacy_fixture();
        let native_join = join_test(&native, &mut |_| Ok::<_, ()>(()))
            .expect("Native first FULL fixture");
        let refuses = |source: &NativeCutoverEvidenceV1| {
            assert!(matches!(
                join_mixed_legacy(source, &native_join, &mut |_| Ok::<_, ()>(())),
                Err(NativeCutoverSourceJoinErrorV1::Refused(_))
            ));
        };
        let mut absent = mixed.clone();
        absent
            .source_facts
            .retain(|f| !matches!(f, Fact::PendingStoreSdkAck { .. }));
        refuses(&absent);
        let mut active = mixed.clone();
        if let Some(Fact::Replay { phase, .. }) = active.source_facts.iter_mut().find(|f| {
            matches!(
                f,
                Fact::Replay {
                    native_update_request: None,
                    ..
                }
            )
        }) {
            *phase = NativeCutoverReplayPhaseV1::RecoveryRequired;
        }
        refuses(&active);
        let mut foreign_m = mixed.clone();
        if let Some(Fact::PendingStoreSdkAck { binding, .. }) = foreign_m
            .source_facts
            .iter_mut()
            .find(|f| matches!(f, Fact::PendingStoreSdkAck { .. }))
        {
            binding.transition_digest = "d".repeat(64);
        }
        refuses(&foreign_m);
        let mut foreign_pointer = mixed.clone();
        if let Some(Fact::PendingStoreSdkAck { binding, .. }) = foreign_pointer
            .source_facts
            .iter_mut()
            .find(|f| matches!(f, Fact::PendingStoreSdkAck { .. }))
        {
            binding.completion_pointer_digest = "a".repeat(64);
        }
        refuses(&foreign_pointer);
        let mut foreign_repo = mixed.clone();
        let publication = foreign_repo
            .legacy_publication
            .as_mut()
            .expect("Legacy DATA");
        publication.activation.active.generation.lexical.repo_id =
            RepoId::new("foreign").expect("canonical foreign repo");
        publication.activation.active.generation.semantic.repo_id =
            RepoId::new("foreign").expect("canonical foreign repo");
        refuses(&foreign_repo);
        let mut foreign_revision = mixed.clone();
        let publication = foreign_revision
            .legacy_publication
            .as_mut()
            .expect("Legacy DATA");
        publication.activation.active.generation.lexical.revision_id =
            RevisionId::new("d".repeat(64)).expect("canonical foreign revision");
        publication
            .activation
            .active
            .generation
            .semantic
            .revision_id = RevisionId::new("d".repeat(64)).expect("canonical foreign revision");
        refuses(&foreign_revision);
        let mut wrong_q_generation = mixed.clone();
        wrong_q_generation
            .legacy_publication
            .as_mut()
            .expect("Legacy DATA")
            .receipt
            .generation = ManifestGeneration::new(3);
        refuses(&wrong_q_generation);
        let mut orphan = mixed.clone();
        let orphan_position = orphan
            .source_facts
            .iter()
            .position(|f| matches!(f, Fact::PendingStoreCustody { .. }))
            .expect("custody range");
        orphan.source_facts.insert(
            orphan_position,
            Fact::PendingStoreAttempt {
                plan_id: "zz-orphan".into(),
                attempt_id: 1,
                state: "TerminalCommitted".into(),
                execution_owner_token: None,
                fence: NativeCutoverAttemptFenceWireV1::LegacyUnfenced,
                started_at_epoch_ms: 1,
                terminal_at_epoch_ms: Some(2),
                updated_at_epoch_ms: 2,
            },
        );
        refuses(&orphan);
        assert!(matches!(
            join_mixed_legacy(&mixed, &native_join, &mut |_| Err::<(), _>("work cap")),
            Err(NativeCutoverSourceJoinErrorV1::Work("work cap"))
        ));
    }

    #[test]
    fn completed_native_change_event_uses_one_sdk_ack_row() {
        let (_, mixed) = signed_mixed_legacy_fixture();
        let sdk_ack = mixed
            .source_facts
            .iter()
            .find(|f| matches!(f, Fact::PendingStoreSdkAck { .. }))
            .expect("completed SDK row")
            .clone();
        let mut evidence = fixed_evidence();
        let checkpoint = evidence
            .source_facts
            .iter()
            .position(|f| matches!(f, Fact::PendingStoreCheckpoint { .. }))
            .expect("checkpoint");
        evidence.source_facts.insert(checkpoint, sdk_ack.clone());
        let _joined = join_test(&evidence, &mut |_| Ok::<_, ()>(()))
            .expect("completed SDK emits SdkAck without a duplicate Sdk row");
        if let Fact::PendingStoreSdkAck {
            plan_id,
            attempt_id,
            commit_owner_token,
            terminal_receipt_digest,
            binding,
            created_at_epoch_ms,
            ..
        } = sdk_ack
        {
            let plain = Fact::PendingStoreSdk {
                plan_id,
                attempt_id,
                commit_owner_token,
                terminal_receipt_digest,
                binding,
                created_at_epoch_ms,
            };
            let checkpoint = evidence
                .source_facts
                .iter()
                .position(|f| matches!(f, Fact::PendingStoreCheckpoint { .. }))
                .expect("checkpoint");
            evidence.source_facts.insert(checkpoint, plain);
            assert!(matches!(
                join_test(&evidence, &mut |_| Ok::<_, ()>(())),
                Err(NativeCutoverSourceJoinErrorV1::Refused(_))
            ));
        }
    }
}

/// Cheap DATA consistency only. The actual Q catalog still validates the
/// receipt, activation token and frozen SearchCorpus owner independently.
fn validate_legacy_publication_axes<E>(
    evidence: &NativeCutoverEvidenceV1,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<(), NativeCutoverSourceJoinErrorV1<E>> {
    let legacy = evidence
        .legacy_publication
        .as_ref()
        .ok_or_else(|| refuse("current Legacy publication DATA is absent"))?;
    let candidate = &legacy.activation.active.generation;
    let receipt = &legacy.receipt;
    if let crate::native_cutover_evidence_v1::NativeCutoverEvidenceLegacyRepoMapV1::Required {
        publish,
        activation,
        ..
    } = &legacy.repomap {
        for receipt in [publish, activation] {
            let ack = &receipt.mutation;
            work(charge, ack.repo_id.as_str().len().saturating_add(ack.revision_id.as_str().len()))?;
            if ack.repo_id != candidate.lexical.repo_id || ack.revision_id != candidate.lexical.revision_id {
                return Err(refuse("Legacy RepoMap receipt differs from the current SDK repo or revision"));
            }
        }
    }
    let mut bytes = evidence
        .header
        .repo_id
        .len()
        .saturating_add(receipt.batch_digest.len())
        .saturating_add(receipt.manifest_digest.as_ref().map_or(0, String::len));
    for snapshot in [&candidate.lexical, &candidate.semantic] {
        bytes = bytes
            .saturating_add(snapshot.repo_id.as_str().len())
            .saturating_add(snapshot.revision_id.as_str().len())
            .saturating_add(snapshot.manifest_digest.len());
    }
    bytes = bytes
        .saturating_add(candidate.semantic_content.row_root_digest.len())
        .saturating_add(candidate.semantic_content.membership_root_digest.len());
    if let Some(roots) = receipt.semantic_content.as_ref() {
        bytes = bytes
            .saturating_add(roots.row_root_digest.len())
            .saturating_add(roots.membership_root_digest.len());
    }
    work(charge, bytes)?;
    if legacy.activation.active.validate_v1().is_err()
        || candidate.lexical.repo_id.as_str() != evidence.header.repo_id
        || candidate.semantic.repo_id.as_str() != evidence.header.repo_id
        || candidate.lexical.revision_id.as_str().as_bytes()
            != evidence.fresh_sdk.sdk_generation_digest.0.as_slice()
        || candidate.semantic.revision_id.as_str().as_bytes()
            != evidence.fresh_sdk.sdk_generation_digest.0.as_slice()
        || candidate.lexical.manifest_generation != receipt.generation
        || candidate.semantic.manifest_generation != receipt.generation
        || receipt.manifest_digest.as_deref() != Some(candidate.lexical.manifest_digest.as_str())
        || receipt.semantic_content.as_ref() != Some(&candidate.semantic_content)
        || !receipt.sealed
        || receipt.batch_digest.is_empty()
        || receipt.durable_sequence == 0
    {
        return Err(refuse("Legacy publication DATA differs from current SDK M"));
    }
    Ok(())
}

// A signed Source inventory is emitted table by table. Recheck that order on
// reception before using binary lookups: malformed or rearranged DATA must not
// convert an absent row into an apparent historical closeout.
fn mixed_rank(fact: &Fact) -> usize {
    match fact {
        Fact::Allocator { .. } => 0,
        Fact::Reservation { .. } => 1,
        Fact::Replay { .. } => 2,
        Fact::PendingReplay { .. } => 3,
        Fact::Outbox { .. } => 4,
        Fact::PendingStoreAttempt { .. } => 5,
        Fact::PendingStoreCustody { .. } => 6,
        Fact::PendingStoreTerminal { .. } => 7,
        Fact::PendingStoreSdk { .. } | Fact::PendingStoreSdkAck { .. } => 8,
        Fact::PendingStoreCheckpoint { .. } => 9,
        Fact::AstFrontier { .. } => 10,
        Fact::AstWork { .. } => 11,
        Fact::CompletedSourceRequest { .. } => 12,
        Fact::NativeInitialSourceBinding { .. } => 13,
        Fact::NativeSdkSourceBinding { .. } => 14,
    }
}

fn mixed_key(fact: &Fact) -> Option<(&str, u32, u64)> {
    match fact {
        Fact::Replay {
            aggregate_identity_digest,
            ..
        }
        | Fact::PendingStoreCheckpoint {
            aggregate_identity_digest,
            ..
        } => Some((aggregate_identity_digest, 0, 0)),
        Fact::PendingReplay {
            plan_id,
            attempt_id,
            ..
        }
        | Fact::PendingStoreAttempt {
            plan_id,
            attempt_id,
            ..
        }
        | Fact::PendingStoreTerminal {
            plan_id,
            attempt_id,
            ..
        }
        | Fact::PendingStoreSdk {
            plan_id,
            attempt_id,
            ..
        }
        | Fact::PendingStoreSdkAck {
            plan_id,
            attempt_id,
            ..
        } => Some((plan_id, *attempt_id, 0)),
        Fact::PendingStoreCustody {
            plan_id,
            attempt_id,
            custody_revision,
            ..
        } => Some((plan_id, *attempt_id, *custody_revision)),
        Fact::Outbox { dispatch_id, .. } => Some((dispatch_id, 0, 0)),
        Fact::AstWork { aggregate_digest, .. } => Some((aggregate_digest, 0, 0)),
        _ => None,
    }
}

fn mixed_range<E>(
    ranges: &[Range<usize>; 15],
    rank: usize,
) -> Result<Range<usize>, NativeCutoverSourceJoinErrorV1<E>> {
    ranges
        .get(rank)
        .cloned()
        .ok_or_else(|| refuse("mixed category out of bounds"))
}

fn mixed_slice<'a, E>(
    facts: &'a [Fact],
    ranges: &[Range<usize>; 15],
    rank: usize,
) -> Result<&'a [Fact], NativeCutoverSourceJoinErrorV1<E>> {
    facts
        .get(mixed_range(ranges, rank)?)
        .ok_or_else(|| refuse("mixed category slice out of bounds"))
}

fn mixed_ranges<E>(
    facts: &[Fact],
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<[Range<usize>; 15], NativeCutoverSourceJoinErrorV1<E>> {
    let mut ranges: [Range<usize>; 15] = std::array::from_fn(|_| 0..0);
    let mut prior_rank = 0;
    let mut prior_key: Option<(&str, u32, u64)> = None;
    for (index, fact) in facts.iter().enumerate() {
        work(charge, 1)?;
        let rank = mixed_rank(fact);
        if rank < prior_rank {
            return Err(refuse("mixed Source inventory order differs"));
        }
        let key = mixed_key(fact);
        if let Some((text, ..)) = key {
            work(charge, text.len())?;
        }
        if rank == prior_rank {
            if let (Some(prior), Some(current)) = (prior_key, key) {
                work(charge, prior.0.len().saturating_add(current.0.len()))?;
                if prior >= current {
                    return Err(refuse("duplicate or unsorted mixed Source key"));
                }
            }
        } else {
            for skipped in prior_rank + 1..rank {
                *ranges
                    .get_mut(skipped)
                    .ok_or_else(|| refuse("mixed category out of bounds"))? = index..index;
            }
            *ranges
                .get_mut(rank)
                .ok_or_else(|| refuse("mixed category out of bounds"))? = index..index;
        }
        let current = ranges
            .get_mut(rank)
            .ok_or_else(|| refuse("mixed category out of bounds"))?;
        if current.start >= current.end {
            current.start = index;
        }
        current.end = index + 1;
        prior_rank = rank;
        prior_key = key;
    }
    for skipped in prior_rank + 1..ranges.len() {
        *ranges
            .get_mut(skipped)
            .ok_or_else(|| refuse("mixed category out of bounds"))? = facts.len()..facts.len();
    }
    Ok(ranges)
}

fn mixed_lower_bound<E>(
    facts: &[Fact],
    range: Range<usize>,
    wanted: (&str, u32, u64),
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<usize, NativeCutoverSourceJoinErrorV1<E>> {
    let mut low = range.start;
    let mut high = range.end;
    while low < high {
        work(charge, 1)?;
        let middle = low + (high - low) / 2;
        let actual = mixed_key(
            facts
                .get(middle)
                .ok_or_else(|| refuse("mixed row out of bounds"))?,
        )
        .ok_or_else(|| refuse("mixed Source key absent"))?;
        work(charge, actual.0.len().saturating_add(wanted.0.len()))?;
        if actual < wanted {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    Ok(low)
}

fn mixed_find<'a, E>(
    facts: &'a [Fact],
    range: Range<usize>,
    wanted: (&str, u32, u64),
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<Option<&'a Fact>, NativeCutoverSourceJoinErrorV1<E>> {
    let index = mixed_lower_bound(facts, range.clone(), wanted, charge)?;
    if index < range.end {
        if let Some(actual) = mixed_key(
            facts
                .get(index)
                .ok_or_else(|| refuse("mixed row out of bounds"))?,
        ) {
            work(charge, actual.0.len().saturating_add(wanted.0.len()))?;
        }
    }
    if index < range.end
        && mixed_key(
            facts
                .get(index)
                .ok_or_else(|| refuse("mixed row out of bounds"))?,
        ) == Some(wanted)
    {
        Ok(facts.get(index))
    } else {
        Ok(None)
    }
}

fn mixed_find_pending<'a, E>(
    facts: &'a [Fact],
    range: Range<usize>,
    plan: &str,
    attempt: u32,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<&'a Fact, NativeCutoverSourceJoinErrorV1<E>> {
    mixed_find(facts, range, (plan, attempt, 0), charge)?
        .ok_or_else(|| refuse("Legacy pending group is incomplete"))
}

enum LegacyCurrentSelectionV1<'a> {
    ChangeEvent { replay: &'a Fact, sdk_ack: &'a Fact },
    Ordinary {
        replay: &'a Fact,
        binding: &'a NativeCutoverEvidenceSourcePublicationBindingV1,
    },
}

fn join_mixed_legacy<'a, E>(
    evidence: &'a NativeCutoverEvidenceV1,
    native: &NativeCutoverJoinedSourceFactsV1<'a>,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<Option<LegacyCurrentSelectionV1<'a>>, NativeCutoverSourceJoinErrorV1<E>> {
    let facts = &evidence.source_facts;
    let ranges = &mixed_ranges(facts, charge)?;
    let current_is_legacy = evidence.legacy_publication.is_some();
    if current_is_legacy {
        validate_legacy_publication_axes(evidence, charge)?;
    }
    for outbox in mixed_slice(facts, ranges, 4)? {
        join_settled_outbox(outbox, &evidence.header.repo_id, charge)?;
    }
    let mut current_sdk_ack = None;
    let mut current_replay = None;
    let mut current_ordinary = None;
    let mut legacy_count = 0_u64;
    for replay in mixed_slice(facts, ranges, 2)? {
        let Fact::Replay {
            workspace_root,
            contract_key,
            aggregate_identity_digest,
            closeout_id,
            attempt_id,
            format_version,
            state_revision,
            phase,
            native_update_request,
            published_snapshot_id,
            terminal_projection_receipt_digest,
            ordinary_source_binding,
            ..
        } = replay
        else {
            return Err(refuse("mixed replay category differs"));
        };
        if native_update_request.is_some() {
            if ordinary_source_binding.is_some() {
                return Err(refuse("Native V9 carries an ordinary Source binding"));
            }
            continue;
        }
        work(
            charge,
            aggregate_identity_digest
                .len()
                .saturating_add(closeout_id.len())
                .saturating_add(attempt_id.len())
                .saturating_add(
                    terminal_projection_receipt_digest
                        .as_ref()
                        .map_or(0, String::len),
                ),
        )?;
        legacy_count = legacy_count
            .checked_add(1)
            .ok_or_else(|| refuse("Legacy replay count overflow"))?;
        if !text_eq(
            workspace_root,
            &evidence.header.canonical_workspace_root,
            charge,
        )? || !text_eq(contract_key, &evidence.header.contract_key, charge)?
            || !canonical_hex_digest(aggregate_identity_digest)
            || closeout_id.is_empty()
            || attempt_id.is_empty()
            || !matches!(*format_version, 3 | 4)
            || *state_revision == 0
            || !matches!(
                phase,
                NativeCutoverReplayPhaseV1::Acked | NativeCutoverReplayPhaseV1::Cleaned
            )
            || terminal_projection_receipt_digest
                .as_deref()
                .is_none_or(|d| !canonical_hex_digest(d))
        {
            return Err(refuse("Legacy V9 is foreign, active or malformed"));
        }
        let checkpoint = mixed_find(
            facts,
            mixed_range(ranges, 9)?,
            (aggregate_identity_digest, 0, 0),
            charge,
        )?
        .ok_or_else(|| refuse("Legacy V9 lacks durable checkpoint"))?;
        // Reuse the same signed V9/embedded/global checkpoint predicate as the
        // Native branch, without lending a historical Legacy row as Q authority.
        let historical = NativeCutoverJoinedSourceFactsV1 {
            baseline: native.baseline,
            baseline_candidate_root_nonce: native.baseline_candidate_root_nonce,
            current_native_target: native.current_native_target,
            fresh_sdk: native.fresh_sdk,
            v5_artifact: native.v5_artifact,
            v5_artifacts: native.v5_artifacts,
            initial_reservation: native.initial_reservation,
            replay: Some(replay),
            current_native_replay: native.current_native_replay,
            pending_carrier: None,
            pending_attempt: None,
            custody_head: None,
            terminal_receipt: None,
            sdk_binding: None,
            sdk_acked_binding: None,
            checkpoint_ack: Some(checkpoint),
            ast_frontier: None,
            ast_published_work: None,
            current_legacy_replay: None,
            current_legacy_ordinary_binding: None,
            current_legacy_sdk_ack: None,
            prior_sdk_acked_binding: None,
            completed_watch_sources: &[],
            completed_source_requests: &[],
            native_initial_sources: &[],
            native_sdk_sources: &[],
        };
        join_checkpoint_ack(&historical, charge)?;
        if let Some(binding) = ordinary_source_binding {
            if closeout_id.starts_with("change-event:") {
                return Err(refuse("Legacy V9 has both ordinary and change-event custody"));
            }
            work(charge, 64 * 7)?;
            if ![
                &binding.transition_digest,
                &binding.completion_pointer_digest,
                &binding.physical_target_source_digest,
            ]
            .into_iter()
            .all(|digest| canonical_hex_digest_bytes(&digest.0))
                || binding.target_snapshot_id == 0
                || *published_snapshot_id != Some(binding.target_snapshot_id)
            {
                return Err(refuse("ordinary Source binding is partial or differs from V9"));
            }
            let current_transition =
                binding.transition_digest == evidence.fresh_sdk.source_manifest_digest;
            let current_pointer =
                binding.completion_pointer_digest == evidence.fresh_sdk.source_pointer_digest;
            if current_transition {
                if !current_pointer
                    || binding.physical_target_source_digest
                        != evidence.fresh_sdk.physical_target_source_digest
                    || binding.target_snapshot_id != evidence.fresh_sdk.target_snapshot_id
                {
                    return Err(refuse("ordinary Source binding partially matches fresh Source"));
                }
                if !current_is_legacy {
                    return Err(refuse("ordinary Source M conflicts with selected Native M"));
                }
                if current_ordinary.replace((replay, binding)).is_some() {
                    return Err(refuse("multiple current ordinary Source bindings remain"));
                }
            } else if current_pointer {
                // The pointer checksum includes the manifest digest. A row
                // naming the current pointer with another transition cannot
                // be a separate historical publication.
                return Err(refuse("ordinary Source pointer names another transition"));
            }
        }
        if let Some(plan) = closeout_id.strip_prefix("change-event:") {
            let Some((attempt_plan, suffix)) = attempt_id
                .strip_prefix("change-event:")
                .and_then(|v| v.rsplit_once(":attempt-"))
            else {
                return Err(refuse("Legacy change-event attempt is malformed"));
            };
            let number = canonical_attempt(suffix)
                .ok_or_else(|| refuse("Legacy attempt number is not canonical"))?;
            if !text_eq(plan, attempt_plan, charge)? {
                return Err(refuse("Legacy closeout and attempt differ"));
            }
            let carrier = mixed_find_pending(facts, mixed_range(ranges, 3)?, plan, number, charge)?;
            let sdk_ack = join_mixed_pending_group(facts, ranges, replay, carrier, charge)?;
            if current_is_legacy && let Some(ack @ Fact::PendingStoreSdkAck { binding, .. }) = sdk_ack {
                // The transition is the signed current SDK M selector. Its
                // other axes must match even if an older M had the same target.
                if ascii_digest_eq(
                    &binding.transition_digest,
                    &evidence.fresh_sdk.source_manifest_digest,
                    charge,
                )? {
                    if !ascii_digest_eq(
                        &binding.target_source_digest,
                        &evidence.fresh_sdk.physical_target_source_digest,
                        charge,
                    )? || !ascii_digest_eq(
                        &binding.completion_pointer_digest,
                        &evidence.fresh_sdk.source_pointer_digest,
                        charge,
                    )? || binding.target_snapshot != evidence.fresh_sdk.target_snapshot_id
                    {
                        return Err(refuse("current SDK M binding differs from fresh Source"));
                    }
                    if current_sdk_ack.replace(ack).is_some() {
                        return Err(refuse("multiple current SDK M ACKs remain"));
                    }
                    current_replay = Some(replay);
                }
            }
        }
    }
    if current_is_legacy && legacy_count == 0 {
        return Err(refuse("current Legacy M V9 is absent"));
    }
    // Every pending row must map to its exact change-event V9. This reverse
    // sweep rejects duplicate aggregates and rows unrelated to the selected M.
    for carrier in mixed_slice(facts, ranges, 3)? {
        work(charge, 1)?;
        let Fact::PendingReplay {
            aggregate_identity_digest,
            plan_id,
            attempt_id,
            ..
        } = carrier
        else {
            return Err(refuse("pending carrier category differs"));
        };
        let replay = mixed_find(
            facts,
            mixed_range(ranges, 2)?,
            (aggregate_identity_digest, 0, 0),
            charge,
        )?
        .ok_or_else(|| refuse("pending carrier has no V9"))?;
        let Fact::Replay {
            native_update_request,
            closeout_id,
            attempt_id: replay_attempt,
            ..
        } = replay
        else {
            return Err(refuse("pending carrier points to absent V9"));
        };
        if native_update_request.is_some() {
            continue;
        }
        if !change_event_identity(closeout_id, replay_attempt, plan_id, *attempt_id, charge)? {
            return Err(refuse("pending carrier and Legacy V9 key differ"));
        }
    }
    for rank in [5_usize, 6, 7, 8] {
        for fact in mixed_slice(facts, ranges, rank)? {
            work(charge, 1)?;
            let (plan, attempt, revision) =
                mixed_key(fact).ok_or_else(|| refuse("orphan pending row lacks key"))?;
            if rank == 6 && revision == 0 {
                return Err(refuse("zero custody revision is not canonical"));
            }
            let _carrier = mixed_find_pending(facts, mixed_range(ranges, 3)?, plan, attempt, charge)?;
        }
    }
    for checkpoint in mixed_slice(facts, ranges, 9)? {
        work(charge, 1)?;
        let (aggregate, _, _) =
            mixed_key(checkpoint).ok_or_else(|| refuse("checkpoint lacks aggregate key"))?;
        let _replay = mixed_find(facts, mixed_range(ranges, 2)?, (aggregate, 0, 0), charge)?
            .ok_or_else(|| refuse("orphan durable checkpoint ACK"))?;
    }
    match (current_replay, current_sdk_ack, current_ordinary, current_is_legacy) {
        (Some(replay), Some(sdk_ack), None, true) => {
            Ok(Some(LegacyCurrentSelectionV1::ChangeEvent { replay, sdk_ack }))
        }
        (None, None, Some((replay, binding)), true) => {
            Ok(Some(LegacyCurrentSelectionV1::Ordinary { replay, binding }))
        }
        (None, None, None, false) => Ok(None),
        (Some(_), Some(_), Some(_), true) => {
            Err(refuse("current Legacy M has two producer associations"))
        }
        _ => Err(refuse("current Legacy SDK M ACK is absent")),
    }
}

fn join_mixed_pending_group<'a, E>(
    facts: &'a [Fact],
    ranges: &[Range<usize>; 15],
    replay: &'a Fact,
    carrier: &'a Fact,
    charge: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<Option<&'a Fact>, NativeCutoverSourceJoinErrorV1<E>> {
    let Fact::Replay {
        workspace_root: replay_root,
        contract_key: replay_contract,
        aggregate_identity_digest: aggregate,
        closeout_id,
        attempt_id: replay_attempt,
        published_snapshot_id: replay_snapshot,
        terminal_projection_receipt_digest: Some(projection_digest),
        ..
    } = replay
    else {
        return Err(refuse("pending group lacks exact Legacy V9"));
    };
    let Fact::PendingReplay {
        workspace_root,
        contract_key,
        aggregate_identity_digest: pending_aggregate,
        plan_id,
        attempt_id,
        owner_token,
        workspace_identity_digest,
        runtime_generation,
        owner_fence_digest,
        custody_revision,
    } = carrier
    else {
        return Err(refuse("pending group lacks exact carrier"));
    };
    if !change_event_identity(closeout_id, replay_attempt, plan_id, *attempt_id, charge)?
        || !text_eq(aggregate, pending_aggregate, charge)?
        || *attempt_id == 0
        || *owner_token == 0
        || *runtime_generation == 0
        || owner_fence_digest.is_empty()
    {
        return Err(refuse("Legacy V9 and pending carrier identity differ"));
    }
    if !text_eq(workspace_root, replay_root, charge)?
        || !text_eq(contract_key, replay_contract, charge)?
    {
        return Err(refuse("pending carrier workspace differs from V9"));
    }
    let attempt = mixed_find_pending(facts, mixed_range(ranges, 5)?, plan_id, *attempt_id, charge)?;
    let Fact::PendingStoreAttempt {
        state,
        execution_owner_token,
        fence,
        terminal_at_epoch_ms,
        ..
    } = attempt
    else {
        return Err(refuse("pending attempt category differs"));
    };
    work(charge, state.len())?;
    if state != "TerminalCommitted"
        || execution_owner_token.is_some()
        || terminal_at_epoch_ms.is_none()
    {
        return Err(refuse("Legacy pending attempt is not terminal"));
    }
    match fence {
        NativeCutoverAttemptFenceWireV1::BoundV2 {
            immutable_owner_token,
            workspace_identity_digest: fence_identity,
            runtime_generation: fence_generation,
        } if immutable_owner_token == owner_token
            && fence_identity == workspace_identity_digest
            && fence_generation == runtime_generation => {}
        _ => return Err(refuse("Legacy pending owner fence differs")),
    }
    let mut owner = *owner_token;
    let mut predecessor_identity = *workspace_identity_digest;
    let mut predecessor_generation = *runtime_generation;
    let mut revision = 0_u64;
    let mut cursor = mixed_lower_bound(
        facts,
        mixed_range(ranges, 6)?,
        (plan_id, *attempt_id, 1),
        charge,
    )?;
    while cursor < mixed_range(ranges, 6)?.end {
        let Fact::PendingStoreCustody {
            plan_id: custody_plan,
            attempt_id: custody_attempt,
            custody_revision: next_revision,
            aggregate_identity_digest: custody_aggregate,
            predecessor_owner_token,
            predecessor_workspace_identity_digest,
            predecessor_runtime_generation,
            owner_token: next_owner,
            owner_workspace_identity_digest,
            owner_runtime_generation,
            ..
        } = facts
            .get(cursor)
            .ok_or_else(|| refuse("mixed custody row out of bounds"))?
        else {
            return Err(refuse("custody category differs"));
        };
        work(
            charge,
            1 + custody_plan.len().saturating_add(plan_id.len()) + 128,
        )?;
        if custody_plan != plan_id || custody_attempt != attempt_id {
            break;
        }
        if revision.checked_add(1) != Some(*next_revision)
            || custody_aggregate.0.as_slice() != aggregate.as_bytes()
            || *predecessor_owner_token != owner
            || *predecessor_workspace_identity_digest != predecessor_identity
            || *predecessor_runtime_generation != predecessor_generation
            || *next_owner == 0
            || *owner_runtime_generation == 0
        {
            return Err(refuse("Legacy pending custody chain differs"));
        }
        owner = *next_owner;
        predecessor_identity = *owner_workspace_identity_digest;
        predecessor_generation = *owner_runtime_generation;
        revision = *next_revision;
        cursor += 1;
    }
    if revision != *custody_revision {
        return Err(refuse("Legacy carrier custody revision differs"));
    }
    let terminal =
        mixed_find_pending(facts, mixed_range(ranges, 7)?, plan_id, *attempt_id, charge)?;
    let Fact::PendingStoreTerminal {
        commit_owner_token,
        canonical_receipt_digest,
        published_snapshot_id,
        checkpoint_generation,
        checkpoint_payload_digest,
        checkpoint_acked_at_epoch_ms,
        ..
    } = terminal
    else {
        return Err(refuse("Legacy terminal category differs"));
    };
    if *commit_owner_token != owner
        || *published_snapshot_id == 0
        || replay_snapshot != &Some(*published_snapshot_id)
        || checkpoint_generation.is_none()
        || checkpoint_payload_digest.is_none()
        || checkpoint_acked_at_epoch_ms.is_none()
        || !text_eq(canonical_receipt_digest, projection_digest, charge)?
    {
        return Err(refuse("Legacy terminal and V9 receipt differ"));
    }
    let sdk = mixed_find(
        facts,
        mixed_range(ranges, 8)?,
        (plan_id, *attempt_id, 0),
        charge,
    )?;
    join_sdk_ack_pair(plan_id, *attempt_id, owner, terminal, None, sdk, charge)?;
    if let Some(Fact::PendingStoreSdkAck { binding, .. }) = sdk {
        if binding.target_snapshot != *published_snapshot_id {
            return Err(refuse("Legacy SDK ACK snapshot differs from terminal"));
        }
    }
    Ok(sdk)
}
