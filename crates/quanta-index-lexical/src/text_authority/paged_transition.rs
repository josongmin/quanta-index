//! Independent Native root-transition proof over authenticated changed map keys.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_core::lexical_maintenance::LexicalMaintenancePolicy;
use quanta_index_core::{CoreError, RequestBudgetV1};
use sha2::{Digest as _, Sha256};

use crate::lexical_allocation::DocVersionReservation;
use crate::lexical_runs::{
    RunCandidate, RunSelectionProgress, SegmentInput, SegmentRemoval, bounded_candidate_scan_limit,
};
use crate::lexical_store::{
    Authority, CandidateWitness, EligibilityWitness, IneligibleReason, MapRoot, ObjectKind,
    ObjectStore, ScanLane, WindowWitness,
};
use crate::maintenance_merge_facts::{NativeMergeOutputFacts, NativeRunMergeFacts};
use crate::text_authority::paged_manifest::NativeRoot;
use crate::text_authority::paged_merge_proof::NativeMergeProof;
use crate::text_authority::paged_segment::decode_body_for_preparation;
use crate::text_authority::paged_wire::{
    CurrentRecord, MAX_BODY_BYTES, NativeSourceEdit, SegmentDescriptor, current_key, encode_source,
    membership_key, source_digest,
};
use crate::text_authority::paged_writer::{
    read_current, read_current_with_ref_budget, read_route_inventory_budget, read_segment,
    source_ids,
};
use crate::text_authority::types::AddedTextDoc;

const MAX_RECORD_BYTES: u64 = 64 * 1024;

struct CleanSelection {
    segments: BTreeSet<u64>,
    run_ids: Vec<u64>,
    input_index_encoded_bytes: u64,
    progress: RunSelectionProgress,
}

impl CleanSelection {
    fn without_merge(progress: RunSelectionProgress) -> Self {
        Self {
            segments: BTreeSet::new(),
            run_ids: Vec::new(),
            input_index_encoded_bytes: 0,
            progress,
        }
    }
}

/// Verify a sealed root's declared mutation and maintenance scope.
///
/// It may change source bundles, fresh catalog-reserved IDs, at most two
/// queued debt segments, and complete clean cohorts selected by policy.
///
/// The independent changed-segment body/posting oracle runs separately.
#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "the transition proof receives the declared mutation and its sealed roots together"
)]
pub(crate) fn verify_transition_for_seal(
    store: &ObjectStore,
    root: &NativeRoot,
    base: Option<&NativeRoot>,
    reservation: &DocVersionReservation,
    source_edits: &[NativeSourceEdit],
    retired: &[u64],
    added: &[AddedTextDoc],
    clear_all: bool,
    first_new_segment_id: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    verify_transition_for_seal_with_policy(
        store,
        root,
        base,
        reservation,
        source_edits,
        retired,
        added,
        clear_all,
        first_new_segment_id,
        budget,
        None,
        None,
    )
    .map(|_proof| ())
}

#[expect(
    clippy::too_many_arguments,
    reason = "the transition proof receives the declared mutation, sealed roots, and service policy together"
)]
pub(crate) fn verify_transition_for_seal_with_policy(
    store: &ObjectStore,
    root: &NativeRoot,
    base: Option<&NativeRoot>,
    reservation: &DocVersionReservation,
    source_edits: &[NativeSourceEdit],
    retired: &[u64],
    added: &[AddedTextDoc],
    clear_all: bool,
    first_new_segment_id: u64,
    budget: Option<&RequestBudgetV1>,
    policy: Option<&LexicalMaintenancePolicy>,
    hard_scan: Option<&ScanLane>,
) -> Result<NativeMergeProof, CoreError> {
    // This proof must not inherit authenticated pages from the producer.
    let verification_store = store.metered_clone();
    let store = &verification_store;
    reservation.validate()?;
    root.validate()?;
    if root.allocation_scope_sha256 != reservation.scope_sha256
        || base.is_some_and(|old| old.allocation_scope_sha256 != reservation.scope_sha256)
    {
        return Err(corrupt("allocation scope differs from reservation"));
    }
    let empty = NativeRoot::empty(reservation.scope_sha256);
    let old = if clear_all {
        &empty
    } else {
        base.unwrap_or(&empty)
    };
    if first_new_segment_id != base.map_or(1, |prior| prior.next_segment_id)
        || root.next_segment_id < first_new_segment_id
        || !matches!(u64::try_from(added.len()), Ok(count) if count == reservation.count)
        || retired
            .windows(2)
            .any(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a, b)| a >= b))
        || added.windows(2).any(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(a, b)| a.doc_id >= b.doc_id)
        })
        || clear_all && !retired.is_empty()
    {
        return Err(corrupt("mutation range, ordering, or count differs"));
    }
    let range_last = if reservation.count == 0 {
        0
    } else {
        reservation
            .first
            .checked_add(reservation.count.saturating_sub(1))
            .ok_or_else(|| corrupt("reservation range overflows"))?
    };
    let added_ids = added.iter().map(|doc| doc.doc_id).collect::<BTreeSet<_>>();
    if added_ids.len() != added.len()
        || added
            .iter()
            .any(|doc| doc.doc_id == 0 || doc.doc_id < reservation.first || doc.doc_id > range_last)
    {
        return Err(corrupt("added IDs differ from fresh reservation"));
    }
    let mut allowed_sources = BTreeSet::new();
    let mut expected_retired = Vec::new();
    let mut upserted = BTreeSet::new();
    for edit in source_edits {
        checkpoint(budget, "lexical:native:transition-source")?;
        let digest = source_digest(edit.key())?;
        if !allowed_sources.insert(digest) {
            return Err(corrupt("source edit repeats or digest collides"));
        }
        if !clear_all {
            expected_retired.extend(source_ids(store, old, edit.key())?);
        }
        let actual = store.map_get(&root.source, &digest, budget)?;
        match edit {
            NativeSourceEdit::Upsert(coverage) => {
                let _new = upserted.insert(digest);
                let reference = actual.ok_or_else(|| corrupt("upserted source absent"))?;
                if reference.root.kind != ObjectKind::Record
                    || store.read_blob(&reference, MAX_RECORD_BYTES, budget)?
                        != encode_source(coverage)?
                {
                    return Err(corrupt("upserted source differs from declared coverage"));
                }
            }
            NativeSourceEdit::Delete(_) if actual.is_some() => {
                return Err(corrupt("deleted source remains current"));
            }
            NativeSourceEdit::Delete(_) => {}
        }
    }
    expected_retired.sort_unstable();
    if expected_retired != retired {
        return Err(corrupt(
            "retired IDs differ from complete source memberships",
        ));
    }
    if hard_scan.is_some() && (base.is_none() || clear_all || policy.is_none()) {
        return Err(corrupt(
            "Native hard checkpoint without old policy authority",
        ));
    }
    let clean_selection = match (base, policy) {
        (Some(prior), Some(policy)) if !clear_all => {
            independently_select_clean_cohorts(store, prior, retired, policy, budget, hard_scan)?
        }
        _ => CleanSelection::without_merge(old.run_registry.selection_progress()),
    };
    let clean_merge_segments = &clean_selection.segments;
    let selection_progress = clean_selection.progress;
    for doc in added {
        if !upserted.contains(&source_digest(&doc.source_file)?) {
            return Err(corrupt("added document has no upserted source"));
        }
        let row = read_current(store, root, doc.doc_id)?
            .ok_or_else(|| corrupt("added current document is absent"))?;
        let indexed = crate::normalize::nfc(&doc.text);
        let expected_sha256: [u8; 32] = Sha256::digest(indexed.as_bytes()).into();
        let expected_source = &doc.source_file;
        let expected_candidate_id = &doc.candidate_id;
        let source_matches = &row.source == expected_source;
        let candidate_matches = &row.candidate_id == expected_candidate_id;
        if !source_matches
            || !candidate_matches
            || row.indexed_sha256 != expected_sha256
            || row.segment_key < first_new_segment_id
        {
            return Err(corrupt(
                "added current identity differs from declared input",
            ));
        }
    }
    let retired_ids = retired.iter().copied().collect::<BTreeSet<_>>();
    if added_ids.iter().any(|id| retired_ids.contains(id)) {
        return Err(corrupt("fresh and retired IDs overlap"));
    }
    let added_count = u64::try_from(added.len())
        .map_err(|error| corrupt(&format!("added count width: {error}")))?;
    let expected_live = old
        .live_docs
        .checked_sub(
            u64::try_from(retired.len())
                .map_err(|error| corrupt(&format!("retire width: {error}")))?,
        )
        .and_then(|count| count.checked_add(added_count))
        .ok_or_else(|| corrupt("live count transition overflows"))?;
    if root.live_docs != expected_live {
        return Err(corrupt("live count differs from declared mutation"));
    }
    store.map_changed_keys(&old.source, &root.source, budget, &mut |key, _, _| {
        let digest: [u8; 32] = key
            .try_into()
            .map_err(|error| corrupt(&format!("changed source key width differs: {error}")))?;
        if !allowed_sources.contains(&digest) {
            return Err(corrupt("unrelated source changed"));
        }
        Ok(())
    })?;

    let mut touched_segments = BTreeMap::new();
    for id in retired {
        let record =
            read_current(store, old, *id)?.ok_or_else(|| corrupt("retired ID absent in base"))?;
        let entry = touched_segments
            .entry(record.segment_key)
            .or_insert_with(BTreeSet::new);
        if !entry.insert(record.local_id) {
            return Err(corrupt("retired local ID repeats"));
        }
        if store
            .map_get(&root.current, &current_key(*id), budget)?
            .is_some()
            || store
                .map_get(
                    &root.membership,
                    &membership_key(&source_digest(&record.source)?, *id),
                    budget,
                )?
                .is_some()
        {
            return Err(corrupt("retired ID remains current or a member"));
        }
    }
    let mut removed_segments = BTreeSet::new();
    let mut clean_doc_ids = BTreeSet::new();
    let mut moved_ids = BTreeSet::new();
    let mut compacted_segments = 0_usize;
    store.map_changed_keys(
        &old.segments,
        &root.segments,
        budget,
        &mut |key, before, after| {
            checkpoint(budget, "lexical:native:transition-segment")?;
            let id = u64::from_be_bytes(key.try_into().map_err(|error| {
                corrupt(&format!("changed segment key width differs: {error}"))
            })?);
            match (before, after) {
                (None, Some(reference)) => {
                    if id < first_new_segment_id
                        || id >= root.next_segment_id
                        || reference.root.kind != ObjectKind::Segment
                    {
                        return Err(corrupt("segment added outside changed range"));
                    }
                }
                (Some(_), Some(_)) => {
                    let retired_locals = touched_segments
                        .get(&id)
                        .ok_or_else(|| corrupt("unrelated segment descriptor changed"))?;
                    let previous = read_segment(store, old, id)?;
                    let current = read_segment(store, root, id)?;
                    let expected = previous
                        .live_locals
                        .iter()
                        .copied()
                        .filter(|local| !retired_locals.contains(local))
                        .collect::<Vec<_>>();
                    if current.logical_ids != previous.logical_ids
                        || current.route_inventory != previous.route_inventory
                        || current.live_locals != expected
                        || expected.is_empty()
                    {
                        return Err(corrupt("retired segment descriptor differs"));
                    }
                }
                (Some(previous_ref), None) => {
                    let previous = read_segment(store, old, id)?;
                    let clean_merge = clean_merge_segments.contains(&id);
                    if clean_merge {
                        clean_doc_ids.extend(previous.logical_ids.iter().copied());
                    }
                    let retired_locals = touched_segments.get(&id);
                    let survivors = previous
                        .live_locals
                        .iter()
                        .copied()
                        .filter(|local| retired_locals.is_none_or(|set| !set.contains(local)))
                        .collect::<Vec<_>>();
                    if !survivors.is_empty() {
                        if clean_merge
                            && (touched_segments.contains_key(&id)
                                || previous.live_locals.len() != previous.logical_ids.len()
                                || store.map_get(&old.compaction_due, key, budget)?.is_some())
                        {
                            return Err(corrupt("selected clean cohort is not full and untouched"));
                        }
                        if !clean_merge
                            && !touched_segments.contains_key(&id)
                            && store.map_get(&old.compaction_due, key, budget)?
                                != Some(previous_ref.clone())
                        {
                            return Err(corrupt("unrelated non-debt segment was compacted"));
                        }
                        if !clean_merge {
                            compacted_segments = compacted_segments.saturating_add(1);
                        }
                        for local in survivors {
                            let logical = *previous
                                .logical_ids
                                .get(usize::try_from(local).map_err(|error| {
                                    corrupt(&format!("removed segment local width: {error}"))
                                })?)
                                .ok_or_else(|| corrupt("removed segment local outside table"))?;
                            let _inserted = moved_ids.insert(logical);
                        }
                    } else if !touched_segments.contains_key(&id) {
                        return Err(corrupt("unrelated empty segment was removed"));
                    }
                    let _inserted = removed_segments.insert(id);
                }
                (None, None) => return Err(corrupt("changed segment has no side")),
            }
            Ok(())
        },
    )?;
    if compacted_segments > 2 {
        return Err(corrupt(
            "compaction removed more than two unrelated segments",
        ));
    }
    if !clean_merge_segments.is_subset(&removed_segments) {
        return Err(corrupt("selected clean cohort was not physically replaced"));
    }
    for id in touched_segments.keys() {
        if let Some(reference) = store.map_get(&root.segments, &id.to_be_bytes(), budget)? {
            let previous = read_segment(store, old, *id)?;
            let current = read_segment(store, root, *id)?;
            if current.live_locals.len() < previous.live_locals.len()
                && store.map_get(&root.compaction_due, &id.to_be_bytes(), budget)?
                    != Some(reference)
            {
                return Err(corrupt("partially retired segment lacks compaction debt"));
            }
        }
    }
    for id in &moved_ids {
        let old_row = read_current(store, old, *id)?
            .ok_or_else(|| corrupt("moved logical ID missing in base"))?;
        let new_row = read_current(store, root, *id)?
            .ok_or_else(|| corrupt("moved logical ID missing in new root"))?;
        if !same_document_identity(&old_row, &new_row) || new_row.segment_key < first_new_segment_id
        {
            return Err(corrupt(
                "compaction changed document identity or old locator",
            ));
        }
    }
    store.map_changed_keys(
        &old.current,
        &root.current,
        budget,
        &mut |key, before, after| {
            let id = u64::from_be_bytes(key.try_into().map_err(|error| {
                corrupt(&format!("changed current key width differs: {error}"))
            })?);
            if retired_ids.contains(&id) && after.is_none()
                || added_ids.contains(&id) && before.is_none() && after.is_some()
                || moved_ids.contains(&id) && before.is_some() && after.is_some()
            {
                return Ok(());
            }
            Err(corrupt("unrelated current ID changed"))
        },
    )?;
    store.map_changed_keys(
        &old.membership,
        &root.membership,
        budget,
        &mut |key, before, after| {
            if key.len() != 40 {
                return Err(corrupt("changed membership key width differs"));
            }
            let id = u64::from_be_bytes(
                key.get(32..)
                    .ok_or_else(|| corrupt("membership ID suffix missing"))?
                    .try_into()
                    .map_err(|error| corrupt(&format!("membership ID width differs: {error}")))?,
            );
            if retired_ids.contains(&id) && after.is_none()
                || added_ids.contains(&id) && before.is_none() && after.is_some()
                || moved_ids.contains(&id) && before.is_some() && after.is_some()
            {
                return Ok(());
            }
            Err(corrupt("unrelated source membership changed"))
        },
    )?;
    for id in &moved_ids {
        let new_row =
            read_current(store, root, *id)?.ok_or_else(|| corrupt("moved current row absent"))?;
        let current = store.map_get(&root.current, &current_key(*id), budget)?;
        let member = store.map_get(
            &root.membership,
            &membership_key(&source_digest(&new_row.source)?, *id),
            budget,
        )?;
        if current != member {
            return Err(corrupt("moved membership/current references differ"));
        }
    }
    for id in &removed_segments {
        let previous = read_segment(store, old, *id)?;
        for route in read_route_inventory_budget(store, &previous, budget)? {
            let surface = route
                .first()
                .copied()
                .ok_or_else(|| corrupt("empty route"))?;
            let map = route_map(root, surface)?;
            if store.map_get(map, &route, budget)?.is_some() {
                return Err(corrupt("removed segment retains route"));
            }
        }
    }
    verify_route_diff(
        store,
        old,
        root,
        first_new_segment_id,
        &removed_segments,
        budget,
    )?;
    store.map_changed_keys(
        &old.compaction_due,
        &root.compaction_due,
        budget,
        &mut |key, before, after| {
            let id =
                u64::from_be_bytes(key.try_into().map_err(|error| {
                    corrupt(&format!("changed debt key width differs: {error}"))
                })?);
            if after.is_some() && !touched_segments.contains_key(&id)
                || before.is_some() && after.is_none() && !removed_segments.contains(&id)
            {
                return Err(corrupt("unrelated compaction debt changed"));
            }
            if let Some(reference) = after
                && store.map_get(&root.segments, key, budget)?.as_ref() != Some(reference)
            {
                return Err(corrupt("compaction debt differs from segment"));
            }
            Ok(())
        },
    )?;
    // Reconstruct run changes from the sealed physical segment diff, not
    // from the producer's run-edit list. A descriptor rewrite removes its
    // prior version and adds its independently measured new version.
    let prior = base.unwrap_or(&empty);
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    store.map_changed_keys(
        &prior.segments,
        &root.segments,
        budget,
        &mut |key, before, after| {
            if let Some(reference) = before {
                removed.push(SegmentRemoval {
                    key: key.to_vec(),
                    reference: reference.clone(),
                });
            }
            if let Some(reference) = after {
                changed.push((key.to_vec(), reference.clone()));
            }
            Ok(())
        },
    )?;
    let old_by_key = removed
        .iter()
        .map(|row| (row.key.as_slice(), &row.reference))
        .collect::<BTreeMap<_, _>>();
    let mut added = Vec::new();
    let mut new_segment_index_bytes = BTreeMap::new();
    for (key, reference) in changed {
        let genuinely_new = !old_by_key.contains_key(key.as_slice());
        let inherited_bytes = old_by_key
            .get(key.as_slice())
            .map(|old_ref| {
                independently_inherit_unchanged_posting_bytes(
                    store, prior, root, &key, old_ref, &reference, budget,
                )
            })
            .transpose()?
            .flatten();
        let index_encoded_bytes = if let Some(bytes) = inherited_bytes {
            bytes
        } else {
            super::paged_verify::independently_count_segment_index(
                store, root, &key, &reference, budget,
            )?
        };
        if genuinely_new {
            let id = u64::from_be_bytes(
                key.as_slice()
                    .try_into()
                    .map_err(|_width| corrupt("changed segment key width differs"))?,
            );
            let _previous = new_segment_index_bytes.insert(id, index_encoded_bytes);
        }
        added.push(SegmentInput {
            key,
            reference,
            index_encoded_bytes,
        });
    }
    prior.run_registry.verify_transition_with_progress(
        &root.run_registry,
        store,
        &removed,
        &added,
        budget,
        selection_progress,
    )?;
    if clean_selection.run_ids.is_empty() {
        return Ok(NativeMergeProof::without_merge());
    }
    Ok(NativeMergeProof {
        input: NativeRunMergeFacts {
            input_runs: u64::try_from(clean_selection.run_ids.len())
                .map_err(|_width| corrupt("selected run count width differs"))?,
            input_segments: u64::try_from(clean_merge_segments.len())
                .map_err(|_width| corrupt("selected segment count width differs"))?,
            input_index_encoded_bytes: clean_selection.input_index_encoded_bytes,
            output: Some(NativeMergeOutputFacts {
                segments: 0,
                index_encoded_bytes: 0,
            }),
        },
        clean_doc_ids,
        new_segment_index_bytes,
    })
}

/// Independently prove descriptor-only cost.
///
/// This does not consume the
/// producer's inherited byte result: it re-reads both descriptor objects,
/// their complete direct dependency inventories, all old/new posting route
/// references, and the prior canonical owner charge.
fn independently_inherit_unchanged_posting_bytes(
    store: &ObjectStore,
    old: &NativeRoot,
    next: &NativeRoot,
    key: &[u8],
    old_ref: &crate::lexical_store::BlobRef,
    next_ref: &crate::lexical_store::BlobRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<u64>, CoreError> {
    let id = u64::from_be_bytes(
        key.try_into()
            .map_err(|_error| corrupt("rewritten segment key width differs"))?,
    );
    if old_ref.root.kind != ObjectKind::Segment || next_ref.root.kind != ObjectKind::Segment {
        return Ok(None);
    }
    let old_descriptor =
        SegmentDescriptor::decode(&store.read_blob(old_ref, 64 * 1024, budget)?)?;
    let next_descriptor =
        SegmentDescriptor::decode(&store.read_blob(next_ref, 64 * 1024, budget)?)?;
    if old_descriptor.id != id
        || next_descriptor.id != id
        || old_descriptor.logical_ids != next_descriptor.logical_ids
        || old_descriptor.route_inventory != next_descriptor.route_inventory
    {
        return Ok(None);
    }
    let Some(inventory) = next_descriptor.route_inventory.as_ref() else {
        return Ok(None);
    };
    if store.object_dependencies(&old_ref.root, budget)? != vec![inventory.root.clone()]
        || store.object_dependencies(&next_ref.root, budget)? != vec![inventory.root.clone()]
    {
        return Ok(None);
    }
    for route in read_route_inventory_budget(store, &next_descriptor, budget)? {
        let (old_map, next_map, kind) = match route.first().copied() {
            Some(
                crate::text_authority::paged_segment::GRAM_RAW
                | crate::text_authority::paged_segment::GRAM_FOLDED,
            ) => (
                &old.gram_routes,
                &next.gram_routes,
                ObjectKind::NativeTrigrams,
            ),
            Some(
                crate::text_authority::paged_segment::TERM_RAW
                | crate::text_authority::paged_segment::TERM_FOLDED,
            ) => (
                &old.term_routes,
                &next.term_routes,
                ObjectKind::NativePositions,
            ),
            _ => return Err(corrupt("rewritten segment route surface differs")),
        };
        let before = store.map_get(old_map, &route, budget)?;
        let after = store.map_get(next_map, &route, budget)?;
        if before.is_none()
            || before != after
            || before.as_ref().is_some_and(|row| row.root.kind != kind)
        {
            return Ok(None);
        }
    }
    let old_charge = old
        .run_registry
        .verify_segment_owner(store, key, old_ref, budget)?;
    let next_charge = old_charge
        .checked_sub(old_ref.root.bytes)
        .and_then(|subtotal| subtotal.checked_add(next_ref.root.bytes))
        .ok_or_else(|| corrupt("unchanged posting charge overflows"))?;
    Ok(Some(next_charge))
}

fn verifier_native_root_sha256(root: &NativeRoot) -> Result<[u8; 32], CoreError> {
    root.validate()?;
    let encoded = crate::channel_payloads::encode_cbor(root, "Native hard scan verifier root")?;
    Ok(Sha256::digest(&encoded).into())
}

fn verifier_native_header_sha256(
    store: &ObjectStore,
    root: &NativeRoot,
    candidate: &RunCandidate,
    budget: Option<&RequestBudgetV1>,
) -> Result<[u8; 32], CoreError> {
    let reference = store
        .map_get(
            &root.run_registry.runs,
            &candidate.run_id.to_be_bytes(),
            budget,
        )?
        .ok_or_else(|| corrupt("Native hard scan header missing during independent replay"))?;
    let members =
        crate::channel_payloads::encode_cbor(&candidate.members, "Native verifier run members")?;
    let mut hasher = Sha256::new();
    hasher.update(b"native-hard-scan-header-v1");
    hasher.update(reference.root.sha256);
    hasher.update(candidate.run_id.to_be_bytes());
    hasher.update(candidate.index_encoded_bytes.to_be_bytes());
    hasher.update(&members);
    Ok(hasher.finalize().into())
}

/// Independent reference-only charge proof. It validates the same old-root
/// metadata through verifier reads without borrowing producer selection.
fn independently_probe_clean_metadata(
    store: &ObjectStore,
    old: &NativeRoot,
    candidate: &RunCandidate,
    affected: &BTreeSet<u64>,
    policy: &LexicalMaintenancePolicy,
    budget: Option<&RequestBudgetV1>,
) -> Result<EligibilityWitness, CoreError> {
    if candidate.index_encoded_bytes > policy.max_merge_input_encoded_bytes() {
        return Ok(EligibilityWitness::Ineligible(IneligibleReason::InputCap));
    }
    if candidate.members.entries == 0 {
        return Ok(EligibilityWitness::Ineligible(
            IneligibleReason::EmptyCohort,
        ));
    }
    let mut staging = 0_u64;
    let mut ineligible = None;
    let mut staging_cap = false;
    old.run_registry
        .visit_run_members(store, candidate, budget, &mut |key, reference, _| {
            if ineligible.is_some() {
                return Ok(());
            }
            checkpoint(budget, "lexical:native:replay-clean-member")?;
            let id = u64::from_be_bytes(key.try_into().map_err(|error| {
                corrupt(&format!("Native hard scan member key width: {error}"))
            })?);
            if affected.contains(&id) || store.map_get(&old.compaction_due, key, budget)?.is_some()
            {
                ineligible = Some(IneligibleReason::TouchedOrDebt);
                return Ok(());
            }
            let Some(segment_ref) = store.map_get(&old.segments, key, budget)? else {
                ineligible = Some(IneligibleReason::RetiredDueOrStale);
                return Ok(());
            };
            if &segment_ref != reference {
                ineligible = Some(IneligibleReason::RetiredDueOrStale);
                return Ok(());
            }
            if segment_ref.root.kind != ObjectKind::Segment {
                return Err(corrupt("Native hard scan segment kind differs"));
            }
            let descriptor =
                SegmentDescriptor::decode(&store.read_blob(&segment_ref, 64 * 1024, budget)?)?;
            if descriptor.id != id {
                return Err(corrupt("Native hard scan descriptor ID differs"));
            }
            if descriptor.live_locals.len() != descriptor.logical_ids.len() {
                ineligible = Some(IneligibleReason::PartialSegment);
                return Ok(());
            }
            let slots =
                u64::try_from(descriptor.live_locals.len())
                    .map_err(|error| corrupt(&format!("Native hard scan document slots: {error}")))?
                    .checked_mul(u64::try_from(std::mem::size_of::<AddedTextDoc>()).map_err(
                        |error| corrupt(&format!("Native hard scan slot width: {error}")),
                    )?)
                    .ok_or_else(|| corrupt("Native hard scan document slot charge overflows"))?;
            staging = staging
                .checked_add(
                    reference
                        .bytes
                        .checked_mul(2)
                        .ok_or_else(|| corrupt("Native hard scan descriptor charge overflows"))?,
                )
                .and_then(|value| value.checked_add(slots))
                .and_then(|value| value.checked_add(slots))
                .ok_or_else(|| corrupt("Native hard scan staging charge overflows"))?;
            if staging > policy.max_staging_bytes() {
                staging_cap = true;
            }
            for local in &descriptor.live_locals {
                checkpoint(budget, "lexical:native:replay-clean-current")?;
                let id = *descriptor
                    .logical_ids
                    .get(usize::try_from(*local).map_err(|error| {
                        corrupt(&format!("Native hard scan local width: {error}"))
                    })?)
                    .ok_or_else(|| corrupt("Native hard scan local absent"))?;
                let (current_ref, current) = read_current_with_ref_budget(store, old, id, budget)?
                    .ok_or_else(|| corrupt("Native hard scan current absent"))?;
                if current.segment_key != descriptor.id
                    || current.local_id != *local
                    || current.body.root.kind != ObjectKind::NativeBody
                    || current.body.bytes > MAX_BODY_BYTES
                {
                    return Err(corrupt("Native hard scan current locator differs"));
                }
                if store
                    .map_get(
                        &old.membership,
                        &membership_key(&source_digest(&current.source)?, id),
                        budget,
                    )?
                    .as_ref()
                    != Some(&current_ref)
                {
                    return Err(corrupt("Native hard scan membership differs"));
                }
                staging = staging
                    .checked_add(current_ref.bytes)
                    .and_then(|value| value.checked_add(current.body.bytes.checked_mul(2)?))
                    .ok_or_else(|| corrupt("Native hard scan body/current charge overflows"))?;
                if staging > policy.max_staging_bytes() {
                    ineligible = Some(IneligibleReason::StagingCap);
                    return Ok(());
                }
            }
            Ok(())
        })?;
    Ok(ineligible.map_or(
        EligibilityWitness::NativeEligible {
            effective_input_encoded_bytes: candidate.index_encoded_bytes,
            staging_bytes: staging,
        },
        EligibilityWitness::Ineligible,
    ))
}

/// Recompute one exact frozen window using a fresh verifier read cache.
/// Ordinal and hash-chain decoration belong to the attempt checkpoint owner.
pub(crate) fn replay_hard_scan_window(
    store: &ObjectStore,
    old: &NativeRoot,
    retired: &[u64],
    policy: &LexicalMaintenancePolicy,
    progress: RunSelectionProgress,
    budget: Option<&RequestBudgetV1>,
) -> Result<WindowWitness, CoreError> {
    old.validate()?;
    if retired
        .windows(2)
        .any(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a, b)| a >= b))
    {
        return Err(corrupt("Native hard scan retirements are not canonical"));
    }
    let high = old
        .run_registry
        .next_run_id
        .checked_sub(1)
        .filter(|high| *high != 0)
        .ok_or_else(|| corrupt("Native hard scan high-water underflows"))?;
    if !policy.hard_pressure(old.run_registry.snapshot()?)
        || progress.pending_run.is_some()
        || progress.scan_high_water_run_id != 0 && progress.scan_high_water_run_id != high
    {
        return Err(corrupt("Native hard scan policy or cursor differs"));
    }
    let verification_store = store.metered_clone();
    let store = &verification_store;
    let mut affected = BTreeSet::new();
    for id in retired {
        checkpoint(budget, "lexical:native:replay-retired")?;
        let row = read_current_with_ref_budget(store, old, *id, budget)?
            .ok_or_else(|| corrupt("Native hard scan retired current absent"))?
            .1;
        let _inserted = affected.insert(row.segment_key);
    }
    let window = old
        .run_registry
        .clone()
        .with_selection_progress(progress)?
        .scan_hard_window(store, 32, budget)?;
    if window.scan_high_water_run_id != high {
        return Err(corrupt("Native hard scan frontier differs"));
    }
    let mut candidates = Vec::new();
    candidates
        .try_reserve(window.candidates.len())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: reserve Native verifier witnesses: {error}"
            ))
        })?;
    for candidate in &window.candidates {
        checkpoint(budget, "lexical:native:replay-candidate")?;
        candidates.push(CandidateWitness {
            run_id: candidate.run_id,
            index_encoded_bytes: candidate.index_encoded_bytes,
            eligibility: independently_probe_clean_metadata(
                store, old, candidate, &affected, policy, budget,
            )?,
            header_sha256: verifier_native_header_sha256(store, old, candidate, budget)?,
        });
    }
    Ok(WindowWitness {
        ordinal: 0,
        prior_chain_sha256: [0; 32],
        start_run_id: progress.next_scan_run_id,
        next_run_id: window.next_scan_run_id,
        frozen_high_water_run_id: high,
        round_complete: window.round_complete,
        candidates,
    })
}

type HardRunSelection = (Vec<(u64, u64, u64)>, RunSelectionProgress);

fn independently_select_hard_runs(
    store: &ObjectStore,
    old: &NativeRoot,
    retired: &[u64],
    policy: &LexicalMaintenancePolicy,
    lane: Option<&ScanLane>,
    budget: Option<&RequestBudgetV1>,
) -> Result<HardRunSelection, CoreError> {
    let lane = lane.ok_or_else(|| corrupt("Native hard scan checkpoint absent"))?;
    let high = old
        .run_registry
        .next_run_id
        .checked_sub(1)
        .ok_or_else(|| corrupt("Native hard scan high-water underflows"))?;
    if lane.authority != Authority::Native
        || lane.base_authority_root_sha256 != verifier_native_root_sha256(old)?
        || lane.frozen_high_water_run_id != high
        || !lane
            .windows
            .last()
            .is_some_and(|window| window.round_complete)
    {
        return Err(corrupt("Native hard scan checkpoint stale or incomplete"));
    }
    let mut previous_next = 1_u64;
    let mut earlier = Vec::<(u64, u64, u64)>::new();
    let mut selected = Vec::new();
    let mut accumulated_bytes = 0_u64;
    let mut accumulated_charge = 0_u64;
    let fan_in = usize::try_from(policy.max_merge_fan_in())
        .map_err(|error| corrupt(&format!("Native hard scan fan-in width: {error}")))?;
    if lane.windows.len() > 129 {
        return Err(corrupt("Native hard scan window bound differs"));
    }
    let mut examined = 0_usize;
    for (ordinal, window) in lane.windows.iter().enumerate() {
        let last = ordinal.checked_add(1) == Some(lane.windows.len());
        let end_exclusive = if last {
            high.checked_add(1)
                .ok_or_else(|| corrupt("Native hard scan frontier overflows"))?
        } else {
            window.next_run_id
        };
        if window.start_run_id != previous_next
            || window.frozen_high_water_run_id != high
            || usize::from(window.ordinal) != ordinal
            || window.round_complete != last
            || window.candidates.len() > 32
            || (!last && window.candidates.is_empty())
            || (!last
                && (window.next_run_id <= previous_next
                    || window.next_run_id > high.saturating_add(1)))
            || (last && window.next_run_id != 1)
        {
            return Err(corrupt("Native hard scan window cursor differs"));
        }
        examined = examined
            .checked_add(window.candidates.len())
            .ok_or_else(|| corrupt("Native hard scan count overflows"))?;
        if examined > 4096 {
            return Err(corrupt("Native hard scan count bound differs"));
        }
        let mut prior_id = window.start_run_id.saturating_sub(1);
        for candidate in &window.candidates {
            if candidate.run_id <= prior_id || candidate.run_id >= end_exclusive {
                return Err(corrupt("Native hard scan candidate order differs"));
            }
            prior_id = candidate.run_id;
        }
        let replay = replay_hard_scan_window(
            store,
            old,
            retired,
            policy,
            RunSelectionProgress {
                next_scan_run_id: previous_next,
                scan_high_water_run_id: high,
                pending_run: None,
            },
            budget,
        )?;
        if replay.start_run_id != window.start_run_id
            || replay.next_run_id != window.next_run_id
            || replay.frozen_high_water_run_id != window.frozen_high_water_run_id
            || replay.round_complete != window.round_complete
            || replay.candidates != window.candidates
        {
            return Err(corrupt("Native hard scan witness differs from old root"));
        }
        previous_next = window.next_run_id;
        for candidate in &window.candidates {
            if selected.len() >= fan_in {
                continue;
            }
            let EligibilityWitness::NativeEligible {
                effective_input_encoded_bytes: input,
                staging_bytes: stage,
            } = candidate.eligibility
            else {
                continue;
            };
            if !selected.is_empty() {
                if let (Some(next_bytes), Some(next_charge)) = (
                    accumulated_bytes.checked_add(input),
                    accumulated_charge.checked_add(stage),
                ) && next_bytes <= policy.max_merge_input_encoded_bytes()
                    && next_charge <= policy.max_staging_bytes()
                {
                    accumulated_bytes = next_bytes;
                    accumulated_charge = next_charge;
                    selected.push((candidate.run_id, input, stage));
                }
                continue;
            }
            for &(first, first_bytes, first_charge) in &earlier {
                if let (Some(next_bytes), Some(next_charge)) = (
                    first_bytes.checked_add(input),
                    first_charge.checked_add(stage),
                ) && next_bytes <= policy.max_merge_input_encoded_bytes()
                    && next_charge <= policy.max_staging_bytes()
                {
                    accumulated_bytes = next_bytes;
                    accumulated_charge = next_charge;
                    selected.extend([
                        (first, first_bytes, first_charge),
                        (candidate.run_id, input, stage),
                    ]);
                    break;
                }
            }
            if selected.is_empty() {
                earlier.push((candidate.run_id, input, stage));
            }
        }
    }
    Ok((
        selected,
        RunSelectionProgress {
            next_scan_run_id: lane.windows.last().map_or(1, |window| window.next_run_id),
            scan_high_water_run_id: high,
            pending_run: None,
        },
    ))
}

/// Independent policy oracle over the old authenticated run graph. The
/// producer's selected member vector is never accepted as a proof input.
fn independently_select_clean_cohorts(
    store: &ObjectStore,
    old: &NativeRoot,
    retired: &[u64],
    policy: &LexicalMaintenancePolicy,
    budget: Option<&RequestBudgetV1>,
    hard_scan: Option<&ScanLane>,
) -> Result<CleanSelection, CoreError> {
    let prior_progress = old.run_registry.selection_progress();
    let before = old.run_registry.snapshot()?;
    if !policy.merge_due(before) {
        if hard_scan.is_some() {
            return Err(corrupt(
                "Native hard checkpoint supplied without merge pressure",
            ));
        }
        return Ok(CleanSelection::without_merge(prior_progress));
    }
    let mut affected = BTreeSet::new();
    for id in retired {
        checkpoint(budget, "lexical:native:select-clean-retired")?;
        let row = read_current(store, old, *id)?
            .ok_or_else(|| corrupt("retired ID missing during clean cohort selection"))?;
        let _inserted = affected.insert(row.segment_key);
    }
    let mut selected = BTreeSet::new();
    let mut selected_run_ids = Vec::new();
    let mut run_count = 0_u32;
    let mut total_bytes = 0_u64;
    let mut body_charge = 0_u64;
    if policy.hard_pressure(before) {
        let (selected_runs, progress) =
            independently_select_hard_runs(store, old, retired, policy, hard_scan, budget)?;
        for (id, expected_bytes, expected_charge) in selected_runs {
            let candidate = old
                .run_registry
                .candidate_by_id(store, id, budget)?
                .ok_or_else(|| corrupt("Native hard scan selected run disappeared"))?;
            let prior_charge = body_charge;
            let Some((cohort, bytes, charge)) = independently_try_clean_cohort(
                store,
                old,
                &candidate,
                &affected,
                policy,
                total_bytes,
                body_charge,
                budget,
            )?
            else {
                return Err(corrupt(
                    "Native hard scan selected cohort failed full replay",
                ));
            };
            if candidate.index_encoded_bytes != expected_bytes
                || charge.checked_sub(prior_charge) != Some(expected_charge)
            {
                return Err(corrupt("Native hard scan selected charge differs"));
            }
            total_bytes = bytes;
            body_charge = charge;
            run_count = run_count
                .checked_add(1)
                .ok_or_else(|| corrupt("clean selected run count overflow"))?;
            selected.extend(cohort);
            selected_run_ids.push(id);
        }
        if run_count < 2 {
            return Ok(CleanSelection::without_merge(progress));
        }
        return Ok(CleanSelection {
            segments: selected,
            run_ids: selected_run_ids,
            input_index_encoded_bytes: total_bytes,
            progress,
        });
    }
    if hard_scan.is_some() {
        return Err(corrupt(
            "Native hard checkpoint supplied outside hard pressure",
        ));
    }
    let window = old.run_registry.scan_window(
        store,
        bounded_candidate_scan_limit(policy.max_merge_fan_in()),
        budget,
    )?;
    let mut first_selected_run = if let Some(id) = prior_progress.pending_run
        && let Some(candidate) = old.run_registry.candidate_by_id(store, id, budget)?
        && let Some((cohort, bytes, charge)) = independently_try_clean_cohort(
            store,
            old,
            &candidate,
            &affected,
            policy,
            total_bytes,
            body_charge,
            budget,
        )? {
        total_bytes = bytes;
        body_charge = charge;
        run_count = 1;
        selected.extend(cohort);
        selected_run_ids.push(id);
        Some(id)
    } else {
        None
    };
    for candidate in &window.candidates {
        if run_count >= policy.max_merge_fan_in() {
            break;
        }
        if Some(candidate.run_id) == prior_progress.pending_run {
            continue;
        }
        if let Some((cohort, bytes, charge)) = independently_try_clean_cohort(
            store,
            old,
            candidate,
            &affected,
            policy,
            total_bytes,
            body_charge,
            budget,
        )? {
            total_bytes = bytes;
            body_charge = charge;
            run_count = run_count
                .checked_add(1)
                .ok_or_else(|| corrupt("clean selected run count overflow"))?;
            selected.extend(cohort);
            selected_run_ids.push(candidate.run_id);
            let _first = first_selected_run.get_or_insert(candidate.run_id);
        }
    }
    let mut progress = RunSelectionProgress {
        next_scan_run_id: window.next_scan_run_id,
        scan_high_water_run_id: window.scan_high_water_run_id,
        pending_run: None,
    };
    if run_count < 2 {
        if prior_progress.pending_run.is_some() && window.round_complete {
            progress.next_scan_run_id = prior_progress
                .pending_run
                .and_then(|id| id.checked_add(1))
                .ok_or_else(|| corrupt("pending scan cursor overflows"))?;
            progress.scan_high_water_run_id = old
                .run_registry
                .next_run_id
                .checked_sub(1)
                .ok_or_else(|| corrupt("scan high-water underflow"))?;
        } else if let Some(anchor) = first_selected_run {
            progress.pending_run = Some(anchor);
            if prior_progress.pending_run != Some(anchor) {
                progress.next_scan_run_id = anchor
                    .checked_add(1)
                    .ok_or_else(|| corrupt("pending scan cursor overflows"))?;
                progress.scan_high_water_run_id = old
                    .run_registry
                    .next_run_id
                    .checked_sub(1)
                    .ok_or_else(|| corrupt("scan high-water underflow"))?;
            }
        }
        return Ok(CleanSelection::without_merge(progress));
    }
    Ok(CleanSelection {
        segments: selected,
        run_ids: selected_run_ids,
        input_index_encoded_bytes: total_bytes,
        progress,
    })
}

fn independently_try_clean_cohort(
    store: &ObjectStore,
    old: &NativeRoot,
    candidate: &RunCandidate,
    affected: &BTreeSet<u64>,
    policy: &LexicalMaintenancePolicy,
    selected_bytes: u64,
    body_charge: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<(BTreeSet<u64>, u64, u64)>, CoreError> {
    checkpoint(budget, "lexical:native:select-clean-run")?;
    let Some(proposed_bytes) = selected_bytes.checked_add(candidate.index_encoded_bytes) else {
        return Ok(None);
    };
    if proposed_bytes > policy.max_merge_input_encoded_bytes() {
        return Ok(None);
    }
    let mut cohort = BTreeSet::new();
    let mut healthy = true;
    let mut cohort_body_charge = body_charge;
    old.run_registry.visit_run_members(
        store,
        candidate,
        budget,
        &mut |key, expected, _bytes| {
            checkpoint(budget, "lexical:native:select-clean-member")?;
            if !healthy {
                return Ok(());
            }
            let id = u64::from_be_bytes(
                key.try_into()
                    .map_err(|_error| corrupt("clean run segment key width differs"))?,
            );
            if affected.contains(&id)
                || store.map_get(&old.segments, key, budget)?.as_ref() != Some(expected)
                || store.map_get(&old.compaction_due, key, budget)?.is_some()
            {
                healthy = false;
                return Ok(());
            }
            let descriptor = read_segment(store, old, id)?;
            if descriptor.live_locals.len() == descriptor.logical_ids.len() {
                match independently_probe_clean_body_charge(
                    store,
                    old,
                    expected,
                    &descriptor,
                    cohort_body_charge,
                    policy.max_staging_bytes(),
                    budget,
                )? {
                    Some(charged) => {
                        cohort_body_charge = charged;
                        let _inserted = cohort.insert(id);
                    }
                    None => healthy = false,
                }
            } else {
                healthy = false;
            }
            Ok(())
        },
    )?;
    Ok(healthy.then_some((cohort, proposed_bytes, cohort_body_charge)))
}

/// Recompute per-cohort body and staging feasibility.
///
/// Use old
/// authenticated objects. A cap miss excludes optional maintenance, while a
/// malformed locator/body remains an error. This is a logical payload charge,
/// not an RSS or output-space reservation.
fn independently_probe_clean_body_charge(
    store: &ObjectStore,
    root: &NativeRoot,
    reference: &crate::lexical_store::BlobRef,
    descriptor: &crate::text_authority::paged_wire::SegmentDescriptor,
    mut charged: u64,
    limit: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<u64>, CoreError> {
    let doc_slots = u64::try_from(descriptor.live_locals.len())
        .map_err(|_error| corrupt("clean segment document count overflows"))?
        .checked_mul(
            u64::try_from(std::mem::size_of::<AddedTextDoc>())
                .map_err(|_error| corrupt("clean document slot width overflows"))?,
        )
        .ok_or_else(|| corrupt("clean document slot charge overflows"))?;
    if !charge_clean_body(
        &mut charged,
        reference
            .bytes
            .checked_mul(2)
            .ok_or_else(|| corrupt("clean descriptor charge overflows"))?,
        limit,
    )? || !charge_clean_body(&mut charged, doc_slots, limit)?
    {
        return Ok(None);
    }
    for local in &descriptor.live_locals {
        checkpoint(budget, "lexical:native:select-clean-body")?;
        let id = *descriptor
            .logical_ids
            .get(usize::try_from(*local).map_err(|_error| corrupt("clean local width differs"))?)
            .ok_or_else(|| corrupt("clean local outside document table"))?;
        let (current_ref, current) = read_current_with_ref_budget(store, root, id, budget)?
            .ok_or_else(|| corrupt("clean current row absent"))?;
        if current.segment_key != descriptor.id
            || current.local_id != *local
            || current.body.root.kind != ObjectKind::NativeBody
        {
            return Err(corrupt("clean current locator or body kind differs"));
        }
        if !charge_clean_body(&mut charged, current_ref.bytes, limit)?
            || !charge_clean_body(
                &mut charged,
                current
                    .body
                    .bytes
                    .checked_mul(2)
                    .ok_or_else(|| corrupt("clean body charge overflows"))?,
                limit,
            )?
        {
            return Ok(None);
        }
        let bytes = store.read_blob(&current.body, MAX_BODY_BYTES, budget)?;
        let decoded = decode_body_for_preparation(
            &bytes,
            usize::try_from(charged)
                .map_err(|_error| corrupt("clean body charge exceeds usize"))?,
            match usize::try_from(limit) {
                Ok(limit) => limit,
                Err(_error) => usize::MAX,
            },
        );
        let (candidate, indexed, _folded) = match decoded {
            Ok(decoded) => decoded,
            Err(CoreError::InvalidContract(message))
                if message == "lexical: Native compaction preparation scratch budget exceeded" =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let indexed_sha256: [u8; 32] = Sha256::digest(indexed.as_bytes()).into();
        if candidate != current.candidate_id || indexed_sha256 != current.indexed_sha256 {
            return Err(corrupt("clean body/current identity differs"));
        }
        let member = store.map_get(
            &root.membership,
            &membership_key(&source_digest(&current.source)?, id),
            budget,
        )?;
        if member != Some(current_ref) {
            return Err(corrupt("clean membership/current references differ"));
        }
    }
    if !charge_clean_body(&mut charged, doc_slots, limit)? {
        return Ok(None);
    }
    Ok(Some(charged))
}

fn charge_clean_body(charged: &mut u64, bytes: u64, limit: u64) -> Result<bool, CoreError> {
    *charged = charged
        .checked_add(bytes)
        .ok_or_else(|| corrupt("clean body charge overflows"))?;
    Ok(*charged <= limit)
}

fn verify_route_diff(
    store: &ObjectStore,
    old: &NativeRoot,
    root: &NativeRoot,
    first_new_segment_id: u64,
    removed_segments: &BTreeSet<u64>,
    budget: Option<&RequestBudgetV1>,
) -> Result<(), CoreError> {
    for (before, after, kind) in [
        (
            &old.gram_routes,
            &root.gram_routes,
            ObjectKind::NativeTrigrams,
        ),
        (
            &old.term_routes,
            &root.term_routes,
            ObjectKind::NativePositions,
        ),
    ] {
        let mut added_hashes = BTreeMap::<u64, (Sha256, u64)>::new();
        store.map_changed_keys(before, after, budget, &mut |key, prior, next| {
            checkpoint(budget, "lexical:native:transition-route")?;
            if key.len() < 9 {
                return Err(corrupt("changed route key is short"));
            }
            let segment_key = u64::from_be_bytes(
                key.get(key.len().saturating_sub(8)..)
                    .ok_or_else(|| corrupt("route segment suffix missing"))?
                    .try_into()
                    .map_err(|error| {
                        corrupt(&format!("route segment ID width differs: {error}"))
                    })?,
            );
            match (prior, next) {
                (None, Some(reference))
                    if segment_key >= first_new_segment_id
                        && segment_key < root.next_segment_id =>
                {
                    if reference.root.kind != kind {
                        return Err(corrupt("new route posting kind differs"));
                    }
                    let (hash, count) = added_hashes
                        .entry(segment_key)
                        .or_insert_with(|| (Sha256::new(), 0));
                    hash.update(
                        u64::try_from(key.len())
                            .map_err(|error| corrupt(&format!("route key length width: {error}")))?
                            .to_be_bytes(),
                    );
                    hash.update(key);
                    *count = count
                        .checked_add(1)
                        .ok_or_else(|| corrupt("route count overflow"))?;
                }
                (Some(_), None) if removed_segments.contains(&segment_key) => {}
                _ => return Err(corrupt("unrelated route changed")),
            }
            Ok(())
        })?;
        for segment_key in first_new_segment_id..root.next_segment_id {
            let descriptor = read_segment(store, root, segment_key)?;
            let mut expected_hash = Sha256::new();
            let mut expected_count = 0_u64;
            for key in read_route_inventory_budget(store, &descriptor, budget)? {
                if (kind == ObjectKind::NativeTrigrams) != matches!(key.first(), Some(1 | 2)) {
                    continue;
                }
                expected_hash.update(
                    u64::try_from(key.len())
                        .map_err(|error| corrupt(&format!("route key length width: {error}")))?
                        .to_be_bytes(),
                );
                expected_hash.update(&key);
                expected_count = expected_count
                    .checked_add(1)
                    .ok_or_else(|| corrupt("route count overflow"))?;
            }
            let (actual_hash, actual_count) = added_hashes
                .remove(&segment_key)
                .unwrap_or_else(|| (Sha256::new(), 0));
            if actual_count != expected_count || actual_hash.finalize() != expected_hash.finalize()
            {
                return Err(corrupt("new route inventory differs from map additions"));
            }
        }
        if !added_hashes.is_empty() {
            return Err(corrupt("route additions have unknown segment"));
        }
    }
    Ok(())
}

fn route_map(root: &NativeRoot, surface: u8) -> Result<&MapRoot, CoreError> {
    match surface {
        1 | 2 => Ok(&root.gram_routes),
        3 | 4 => Ok(&root.term_routes),
        _ => Err(corrupt("route surface differs")),
    }
}

fn same_document_identity(a: &CurrentRecord, b: &CurrentRecord) -> bool {
    a.id == b.id
        && a.source == b.source
        && a.candidate_id == b.candidate_id
        && a.indexed_sha256 == b.indexed_sha256
        && a.body == b.body
}

fn checkpoint(budget: Option<&RequestBudgetV1>, stage: &'static str) -> Result<(), CoreError> {
    if let Some(budget) = budget {
        budget.checkpoint(stage)?;
    }
    Ok(())
}

fn corrupt(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        message: format!("lexical: Native transition: {reason}"),
    }
}
