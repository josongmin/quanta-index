//! Independent Native root-transition proof over authenticated changed map keys.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_core::{CoreError, RequestBudgetV1};
use sha2::{Digest as _, Sha256};

use crate::lexical_allocation::DocVersionReservation;
use crate::lexical_store::{MapRoot, ObjectKind, ObjectStore};
use crate::text_authority::paged_manifest::NativeRoot;
use crate::text_authority::paged_wire::{
    CurrentRecord, NativeSourceEdit, current_key, encode_source, membership_key, source_digest,
};
use crate::text_authority::paged_writer::{
    read_current, read_route_inventory_budget, read_segment, source_ids,
};
use crate::text_authority::types::AddedTextDoc;

const MAX_RECORD_BYTES: u64 = 64 * 1024;

/// Prove that a new root changes only the declared source bundles, fresh
/// catalog-reserved IDs, and at most two queued compaction segments.
///
/// The independent changed-segment body/posting oracle runs separately.
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
                    let retired_locals = touched_segments.get(&id);
                    let survivors = previous
                        .live_locals
                        .iter()
                        .copied()
                        .filter(|local| retired_locals.is_none_or(|set| !set.contains(local)))
                        .collect::<Vec<_>>();
                    if !survivors.is_empty() {
                        if !touched_segments.contains_key(&id)
                            && store.map_get(&old.compaction_due, key, budget)?
                                != Some(previous_ref.clone())
                        {
                            return Err(corrupt("unrelated non-debt segment was compacted"));
                        }
                        compacted_segments = compacted_segments.saturating_add(1);
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
    Ok(())
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
