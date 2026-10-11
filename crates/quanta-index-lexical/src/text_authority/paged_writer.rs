//! Publication of changed Native source bundles into immutable segments.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::{ManifestGeneration, SourceFileKey};
use quanta_index_core::{CoreError, RequestBudgetV1};
use sha2::{Digest as _, Sha256};

use crate::lexical_store::{AttemptLease, BlobRef, MapEdit, MapRoot, ObjectKind, ObjectStore};
use crate::text_authority::paged_bootstrap::{BootstrapMap, NativeBootstrapMaps};
use crate::text_authority::paged_manifest::NativeRoot;
use crate::text_authority::paged_posting_page::{HEADER_BYTES, PostingPage, TARGET_BYTES};
use crate::text_authority::paged_segment::{
    BuiltPosting, BuiltSegment, GRAM_FOLDED, GRAM_RAW, TERM_FOLDED, TERM_RAW,
    compaction_scratch_limit, decode_body, decode_body_for_preparation,
};
use crate::text_authority::paged_wire::{
    CurrentRecord, MAX_BODY_BYTES, MAX_POSTING_BYTES, NativeSourceEdit, SEGMENT_DOCS,
    SegmentDescriptor, current_key, decode_source, encode_source, gram_prefix, membership_key,
    prefix_upper, route_key, segment_key, source_digest, term_prefix,
};
use crate::text_authority::types::{AddedTextDoc, TextAuthorityWriteReceipt};

const SEGMENT_TARGET_TEXT_BYTES: usize = 2 * 1024 * 1024;
const MAX_CURRENT_RECORD_BYTES: u64 = 32 * 1024;
const MAX_SOURCE_RECORD_BYTES: u64 = 64 * 1024;

/// Request-local preparation. No object-store reference is published until a
/// target lease binds the derived relations to its own segment ID.
pub(crate) struct PreparedNativeCompaction {
    selected: Vec<(u64, BlobRef, SegmentDescriptor)>,
    current_witnesses: Vec<(u64, BlobRef, BlobRef)>,
    docs: Vec<AddedTextDoc>,
}

struct ScratchAdmission {
    limit: u64,
    charged: u64,
}

impl ScratchAdmission {
    fn charge(&mut self, bytes: u64) -> Result<(), CoreError> {
        self.charged = self
            .charged
            .checked_add(bytes)
            .ok_or_else(compaction_scratch_limit)?;
        if self.charged > self.limit {
            return Err(compaction_scratch_limit());
        }
        Ok(())
    }
}

/// Read at most the two oldest debts and retain their authenticated bodies and
/// exact map witnesses.
///
/// The caller holds the base's retention custody while
/// this read is active. A rejected scratch budget leaves no partial objects.
pub(crate) fn prepare_compaction(
    store: &ObjectStore,
    base: &NativeRoot,
    max_scratch_bytes: u64,
    excluded_ids: &[u64],
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<PreparedNativeCompaction>, CoreError> {
    match prepare_compaction_inner(store, base, max_scratch_bytes, excluded_ids, budget) {
        Err(CoreError::InvalidContract(message))
            if message == "lexical: Native compaction preparation scratch budget exceeded" =>
        {
            Ok(None)
        }
        result => result,
    }
}

fn prepare_compaction_inner(
    store: &ObjectStore,
    base: &NativeRoot,
    max_scratch_bytes: u64,
    excluded_ids: &[u64],
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<PreparedNativeCompaction>, CoreError> {
    if let Some(budget) = budget {
        budget.checkpoint("lexical:native:prepare:begin")?;
    }
    if excluded_ids
        .windows(2)
        .any(|pair| pair.first() >= pair.get(1))
    {
        return Err(CoreError::InvalidContract(
            "lexical: prepared retirement IDs are not sorted and unique".into(),
        ));
    }
    base.validate()?;
    let due = store.map_scan_limit(&base.compaction_due, &[0], None, 2, budget)?;
    if due.is_empty() {
        return Ok(None);
    }
    let mut admission = ScratchAdmission {
        limit: max_scratch_bytes,
        charged: 0,
    };
    // Fixed transient record/descriptor/source-key decode space. Retained
    // source strings are charged by their authenticated record length below.
    admission.charge(8 * MAX_CURRENT_RECORD_BYTES + 2 * MAX_SOURCE_RECORD_BYTES)?;
    let mut selected = Vec::new();
    selected.try_reserve_exact(2).map_err(|error| {
        CoreError::Storage(format!("lexical: reserve prepared descriptors: {error}"))
    })?;
    let mut docs = Vec::new();
    admission.charge(
        u64::try_from(
            SEGMENT_DOCS
                .checked_mul(std::mem::size_of::<AddedTextDoc>())
                .ok_or_else(compaction_scratch_limit)?,
        )
        .map_err(|_error| compaction_scratch_limit())?,
    )?;
    docs.try_reserve_exact(SEGMENT_DOCS).map_err(|error| {
        CoreError::Storage(format!("lexical: reserve prepared documents: {error}"))
    })?;
    for (key, reference) in due {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:native:prepare:segment")?;
        }
        admission.charge(
            reference
                .bytes
                .checked_add(
                    u64::try_from(std::mem::size_of::<(u64, BlobRef, SegmentDescriptor)>())
                        .map_err(|_error| compaction_scratch_limit())?,
                )
                .ok_or_else(compaction_scratch_limit)?,
        )?;
        let id = u64::from_be_bytes(key.as_slice().try_into().map_err(|_error| {
            CoreError::InvalidContract("lexical: Native compaction key width differs".into())
        })?);
        if store.map_get(&base.segments, &key, budget)? != Some(reference.clone()) {
            return Err(CoreError::InvalidContract(
                "lexical: due segment version differs".into(),
            ));
        }
        let descriptor = read_segment_budget(store, base, id, budget)?;
        for local in &descriptor.live_locals {
            let index = usize::try_from(*local).map_err(|error| {
                CoreError::InvalidContract(format!("lexical: prepared local ID width: {error}"))
            })?;
            let id = descriptor.logical_ids.get(index).ok_or_else(|| {
                CoreError::InvalidContract("lexical: prepared local ID absent".into())
            })?;
            if excluded_ids.binary_search(id).is_ok() {
                // Never read a body which this publication will retire.
                return Ok(None);
            }
        }
        let segment_docs =
            live_segment_docs_admitted(store, base, &descriptor, Some(&mut admission), budget)?;
        let added_bytes = segment_docs
            .iter()
            .try_fold(0_usize, |sum, doc| sum.checked_add(doc.text.len()))
            .ok_or_else(compaction_scratch_limit)?;
        let prior_bytes = docs
            .iter()
            .try_fold(0_usize, |sum: usize, doc: &AddedTextDoc| {
                sum.checked_add(doc.text.len())
            })
            .ok_or_else(compaction_scratch_limit)?;
        if !selected.is_empty()
            && (docs.len().saturating_add(segment_docs.len()) > SEGMENT_DOCS
                || prior_bytes.saturating_add(added_bytes) > SEGMENT_TARGET_TEXT_BYTES)
        {
            break;
        }
        docs.extend(segment_docs);
        selected.push((id, reference, descriptor));
    }
    if selected.is_empty() || docs.is_empty() {
        return Err(CoreError::InvalidContract(
            "lexical: Native compaction selected no live segment".into(),
        ));
    }
    docs.sort_unstable_by_key(|doc| doc.doc_id);
    if docs.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(a, b)| a.doc_id == b.doc_id)
    }) {
        return Err(CoreError::InvalidContract(
            "lexical: compaction source segments share a live logical ID".into(),
        ));
    }
    admission.charge(
        u64::try_from(docs.len())
            .map_err(|_error| compaction_scratch_limit())?
            .checked_mul(
                u64::try_from(std::mem::size_of::<(u64, BlobRef, BlobRef)>())
                    .map_err(|_error| compaction_scratch_limit())?,
            )
            .ok_or_else(compaction_scratch_limit)?,
    )?;
    let mut current_witnesses = Vec::new();
    current_witnesses
        .try_reserve_exact(docs.len())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: reserve prepared Native witnesses: {error}"
            ))
        })?;
    for doc in &docs {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:native:prepare:witness")?;
        }
        let (current_ref, current) = read_current_with_ref_budget(store, base, doc.doc_id, budget)?
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: prepared current is absent".into())
            })?;
        let digest = source_digest(&current.source)?;
        let membership_ref = store
            .map_get(
                &base.membership,
                &membership_key(&digest, doc.doc_id),
                budget,
            )?
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: prepared membership is absent".into())
            })?;
        let same_source = current.source == doc.source_file;
        if current_ref != membership_ref || !same_source || current.candidate_id != doc.candidate_id
        {
            return Err(CoreError::InvalidContract(
                "lexical: prepared current/membership differs".into(),
            ));
        }
        current_witnesses.push((doc.doc_id, current_ref, membership_ref));
    }
    Ok(Some(PreparedNativeCompaction {
        selected,
        current_witnesses,
        docs,
    }))
}

#[derive(Clone, Copy, Default)]
struct PostingPageStats {
    pages: u64,
    oversized_pages: u64,
    oversized_bytes: u64,
}

impl PostingPageStats {
    fn add(&mut self, other: Self) -> Result<(), CoreError> {
        self.pages = self
            .pages
            .checked_add(other.pages)
            .ok_or_else(page_count_overflow)?;
        self.oversized_pages = self
            .oversized_pages
            .checked_add(other.oversized_pages)
            .ok_or_else(page_count_overflow)?;
        self.oversized_bytes = self
            .oversized_bytes
            .checked_add(other.oversized_bytes)
            .ok_or_else(page_count_overflow)?;
        Ok(())
    }
}

fn page_count_overflow() -> CoreError {
    CoreError::InvalidContract("lexical: Native posting page accounting overflow".into())
}

pub(crate) fn read_current(
    store: &ObjectStore,
    root: &NativeRoot,
    id: u64,
) -> Result<Option<CurrentRecord>, CoreError> {
    Ok(read_current_with_ref(store, root, id)?.map(|(_, record)| record))
}

/// Return the authenticated current map value alongside its decoded record.
/// Seal proofs use the same lookup to compare current and membership references.
pub(crate) fn read_current_with_ref(
    store: &ObjectStore,
    root: &NativeRoot,
    id: u64,
) -> Result<Option<(BlobRef, CurrentRecord)>, CoreError> {
    read_current_with_ref_budget(store, root, id, None)
}

fn read_current_with_ref_budget(
    store: &ObjectStore,
    root: &NativeRoot,
    id: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<Option<(BlobRef, CurrentRecord)>, CoreError> {
    let Some(reference) = store.map_get(&root.current, &current_key(id), budget)? else {
        return Ok(None);
    };
    if reference.root.kind != ObjectKind::Record {
        return Err(CoreError::InvalidContract(
            "lexical: Native current value kind differs".into(),
        ));
    }
    let record =
        CurrentRecord::decode(&store.read_blob(&reference, MAX_CURRENT_RECORD_BYTES, budget)?)?;
    if record.id != id {
        return Err(CoreError::InvalidContract(
            "lexical: Native current key/value ID differs".into(),
        ));
    }
    Ok(Some((reference, record)))
}

fn apply_edits(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: &MapRoot,
    edits: BTreeMap<Vec<u8>, Option<BlobRef>>,
) -> Result<MapRoot, CoreError> {
    store.map_apply(
        lease,
        base,
        edits
            .into_iter()
            .map(|(key, value)| Ok(MapEdit { key, value })),
    )
}

fn checked_insert(
    edits: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    key: Vec<u8>,
    value: Option<BlobRef>,
) -> Result<(), CoreError> {
    if edits.insert(key, value).is_some() {
        return Err(CoreError::InvalidContract(
            "lexical: duplicate Native map edit key".into(),
        ));
    }
    Ok(())
}

/// Publish one complete effective change set. The caller owns the durable
/// allocation reservation and supplies the exact retired IDs before mutating
/// Tantivy.
///
/// Logical IDs are never selected from the live maximum here.
#[expect(
    clippy::too_many_arguments,
    reason = "one publish call binds the complete declared delta and durable allocation scope"
)]
pub(crate) fn publish_delta(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&NativeRoot>,
    source_edits: &[NativeSourceEdit],
    retired: &[u64],
    added: &[AddedTextDoc],
    clear_all: bool,
    target_generation: ManifestGeneration,
    allocation_scope_sha256: [u8; 32],
) -> Result<(NativeRoot, TextAuthorityWriteReceipt), CoreError> {
    publish_delta_with_option(
        store,
        lease,
        base,
        source_edits,
        retired,
        added,
        clear_all,
        target_generation,
        allocation_scope_sha256,
        None,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "one publish call binds the complete declared delta and optional pure preparation"
)]
pub(crate) fn publish_delta_prepared(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&NativeRoot>,
    source_edits: &[NativeSourceEdit],
    retired: &[u64],
    added: &[AddedTextDoc],
    clear_all: bool,
    target_generation: ManifestGeneration,
    allocation_scope_sha256: [u8; 32],
    prepared: Option<PreparedNativeCompaction>,
) -> Result<(NativeRoot, TextAuthorityWriteReceipt), CoreError> {
    if prepared.is_none() {
        return publish_delta(
            store,
            lease,
            base,
            source_edits,
            retired,
            added,
            clear_all,
            target_generation,
            allocation_scope_sha256,
        );
    }
    publish_delta_with_option(
        store,
        lease,
        base,
        source_edits,
        retired,
        added,
        clear_all,
        target_generation,
        allocation_scope_sha256,
        prepared,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "one publication binds one complete delta and its optional prepared input"
)]
fn publish_delta_with_option(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&NativeRoot>,
    source_edits: &[NativeSourceEdit],
    retired: &[u64],
    added: &[AddedTextDoc],
    clear_all: bool,
    target_generation: ManifestGeneration,
    allocation_scope_sha256: [u8; 32],
    prepared: Option<PreparedNativeCompaction>,
) -> Result<(NativeRoot, TextAuthorityWriteReceipt), CoreError> {
    store.with_adoption_batch(lease, |batched_store| {
        // Preserve an explicit caller's physical-read meter. Ordinary callers
        // receive a bounded producer cache; seal proofs create fresh scopes.
        let producer_store = if batched_store.has_read_meter() {
            batched_store.clone()
        } else {
            batched_store.metered_clone()
        };
        publish_delta_inner(
            &producer_store,
            lease,
            base,
            source_edits,
            retired,
            added,
            clear_all,
            target_generation,
            allocation_scope_sha256,
            prepared,
        )
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "one publish call binds the complete declared delta and durable allocation scope"
)]
fn publish_delta_inner(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: Option<&NativeRoot>,
    source_edits: &[NativeSourceEdit],
    retired: &[u64],
    added: &[AddedTextDoc],
    clear_all: bool,
    target_generation: ManifestGeneration,
    allocation_scope_sha256: [u8; 32],
    prepared: Option<PreparedNativeCompaction>,
) -> Result<(NativeRoot, TextAuthorityWriteReceipt), CoreError> {
    let inherited = base
        .cloned()
        .unwrap_or_else(|| NativeRoot::empty(allocation_scope_sha256));
    inherited.validate()?;
    if inherited.allocation_scope_sha256 != allocation_scope_sha256 {
        return Err(CoreError::InvalidContract(
            "lexical: Native allocation scope changed".into(),
        ));
    }
    let mut next = if clear_all {
        let mut empty = NativeRoot::empty(allocation_scope_sha256);
        empty.next_segment_id = inherited.next_segment_id;
        empty
    } else {
        inherited.clone()
    };
    if added.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(a, b)| a.doc_id >= b.doc_id)
    }) || retired
        .windows(2)
        .any(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a, b)| a >= b))
        || added.iter().any(|doc| doc.doc_id == 0)
    {
        return Err(CoreError::InvalidContract(
            "lexical: Native delta IDs are not canonical".into(),
        ));
    }
    let mut source_map = BTreeMap::new();
    let mut current_map = BTreeMap::new();
    let mut membership_map = BTreeMap::new();
    let mut segment_map = BTreeMap::new();
    let mut gram_map = BTreeMap::new();
    let mut term_map = BTreeMap::new();
    let mut due_map = BTreeMap::new();
    let mut retired_segments = BTreeMap::<u64, SegmentDescriptor>::new();
    let mut changed_sources = BTreeSet::<[u8; 32]>::new();
    let mut upserted_sources = BTreeSet::<[u8; 32]>::new();
    for edit in source_edits {
        let digest = source_digest(edit.key())?;
        if !changed_sources.insert(digest) {
            return Err(CoreError::InvalidContract(
                "lexical: duplicate Native source edit".into(),
            ));
        }
        if !clear_all && let Some(prior) = store.map_get(&inherited.source, &digest, None)? {
            let prior = decode_source(&store.read_blob(&prior, MAX_SOURCE_RECORD_BYTES, None)?)?;
            if prior.source.file != *edit.key() {
                return Err(CoreError::InvalidContract(
                    "lexical: Native source digest collision".into(),
                ));
            }
        }
        let value = match edit {
            NativeSourceEdit::Upsert(coverage) => {
                let _new = upserted_sources.insert(digest);
                let bytes = encode_source(coverage)?;
                Some(store.put_blob(lease, ObjectKind::Record, &[], &bytes)?)
            }
            NativeSourceEdit::Delete(_) => None,
        };
        checked_insert(&mut source_map, digest.to_vec(), value)?;
    }
    if clear_all && !retired.is_empty() {
        return Err(CoreError::InvalidContract(
            "lexical: Native clear-all cannot also name individual retirements".into(),
        ));
    }
    if !clear_all {
        let mut expected_retired = Vec::new();
        for edit in source_edits {
            expected_retired.extend(source_ids(store, &inherited, edit.key())?);
        }
        expected_retired.sort_unstable();
        if expected_retired != retired {
            return Err(CoreError::InvalidContract(
                "lexical: Native source bundle retirement is incomplete or foreign".into(),
            ));
        }
        for id in retired {
            let record = read_current(store, &inherited, *id)?.ok_or_else(|| {
                CoreError::InvalidContract(format!("lexical: retire missing Native doc {id}"))
            })?;
            let digest = source_digest(&record.source)?;
            let member_key = membership_key(&digest, *id);
            let member = store
                .map_get(&inherited.membership, &member_key, None)?
                .ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical: Native current doc lacks source membership".into(),
                    )
                })?;
            let member_record = CurrentRecord::decode(&store.read_blob(
                &member,
                MAX_CURRENT_RECORD_BYTES,
                None,
            )?)?;
            if member_record.id != *id || member_record.source != record.source {
                return Err(CoreError::InvalidContract(
                    "lexical: Native source membership differs from current".into(),
                ));
            }
            if let std::collections::btree_map::Entry::Vacant(entry) =
                retired_segments.entry(record.segment_key)
            {
                let descriptor = read_segment(store, &inherited, record.segment_key)?;
                let _prior = entry.insert(descriptor);
            }
            let descriptor = retired_segments
                .get_mut(&record.segment_key)
                .ok_or_else(|| {
                    CoreError::InvalidContract("lexical: Native retired segment vanished".into())
                })?;
            if descriptor
                .logical_ids
                .get(usize::try_from(record.local_id).map_err(|error| {
                    CoreError::InvalidContract(format!("lexical: Native local ID width: {error}"))
                })?)
                != Some(id)
            {
                return Err(CoreError::InvalidContract(
                    "lexical: Native retired current locator differs from segment".into(),
                ));
            }
            let position = descriptor
                .live_locals
                .binary_search(&record.local_id)
                .map_err(|_error| {
                    CoreError::InvalidContract(
                        "lexical: Native retired doc is not live in segment".into(),
                    )
                })?;
            let _removed = descriptor.live_locals.remove(position);
            checked_insert(&mut current_map, current_key(*id).to_vec(), None)?;
            checked_insert(&mut membership_map, member_key, None)?;
        }
    }
    for (id, descriptor) in retired_segments {
        if descriptor.live_locals.is_empty() {
            for route in read_route_inventory(store, &descriptor)? {
                let target = match route.first().copied() {
                    Some(GRAM_RAW | GRAM_FOLDED) => &mut gram_map,
                    Some(TERM_RAW | TERM_FOLDED) => &mut term_map,
                    _ => {
                        return Err(CoreError::InvalidContract(
                            "lexical: Native route inventory has unknown surface".into(),
                        ));
                    }
                };
                checked_insert(target, route, None)?;
            }
            checked_insert(&mut segment_map, segment_key(id).to_vec(), None)?;
            if store
                .map_get(&inherited.compaction_due, &segment_key(id), None)?
                .is_some()
            {
                checked_insert(&mut due_map, segment_key(id).to_vec(), None)?;
            }
            continue;
        }
        let bytes = descriptor.encode()?;
        let inventory = descriptor.route_inventory.as_ref().ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: retired Native segment lacks route inventory".into(),
            )
        })?;
        let reference = store.put_blob(
            lease,
            ObjectKind::Segment,
            std::slice::from_ref(&inventory.root),
            &bytes,
        )?;
        checked_insert(
            &mut segment_map,
            segment_key(id).to_vec(),
            Some(reference.clone()),
        )?;
        checked_insert(&mut due_map, segment_key(id).to_vec(), Some(reference))?;
    }
    for doc in added {
        let digest = source_digest(&doc.source_file)?;
        if !upserted_sources.contains(&digest) {
            return Err(CoreError::InvalidContract(
                "lexical: Native added document has no upserted source binding".into(),
            ));
        }
    }
    // Retirements are one bounded delta. Publish them into the private root
    // before writing additions so route/current edit maps never accumulate
    // across a full bootstrap corpus.
    next.source = apply_edits(store, lease, &next.source, source_map)?;
    next.current = apply_edits(store, lease, &next.current, current_map)?;
    next.membership = apply_edits(store, lease, &next.membership, membership_map)?;
    next.segments = apply_edits(store, lease, &next.segments, segment_map)?;
    next.gram_routes = apply_edits(store, lease, &next.gram_routes, gram_map)?;
    next.term_routes = apply_edits(store, lease, &next.term_routes, term_map)?;
    next.compaction_due = apply_edits(store, lease, &next.compaction_due, due_map)?;
    let (consumed, mut posting_stats, compaction_rederived_docs, compaction_rederived_text_bytes) =
        compact_due_segments(store, lease, &mut next, target_generation, added, prepared)?;
    let mut bootstrap = if consumed == 0
        && !added.is_empty()
        && next.current.entries == 0
        && next.membership.entries == 0
        && next.segments.entries == 0
        && next.gram_routes.entries == 0
        && next.term_routes.entries == 0
        && next.compaction_due.entries == 0
    {
        store.flush_adoption_batch()?;
        Some(NativeBootstrapMaps::new()?)
    } else {
        None
    };
    for group in segment_groups(added.get(consumed..).ok_or_else(|| {
        CoreError::InvalidContract("lexical: Native consumed beyond added documents".into())
    })?) {
        let group = group?;
        // Tokenization is bounded by one segment cohort; release the write
        // gate while building its postings, then reacquire on the next write.
        store.flush_adoption_batch()?;
        let segment = BuiltSegment::build(next.next_segment_id, target_generation, group)?;
        let mut current_map = BTreeMap::new();
        let mut membership_map = BTreeMap::new();
        let mut segment_map = BTreeMap::new();
        let mut gram_map = BTreeMap::new();
        let mut term_map = BTreeMap::new();
        posting_stats.add(write_segment(
            store,
            lease,
            &next,
            &segment,
            group,
            &mut current_map,
            &mut membership_map,
            &mut segment_map,
            &mut gram_map,
            &mut term_map,
            None,
        )?)?;
        if let Some(scratch) = &mut bootstrap {
            // Keep private SQLite work outside the durable adoption gate.
            store.flush_adoption_batch()?;
            scratch.stage(BootstrapMap::Current, current_map)?;
            scratch.stage(BootstrapMap::Membership, membership_map)?;
            scratch.stage(BootstrapMap::Segments, segment_map)?;
            scratch.stage(BootstrapMap::GramRoutes, gram_map)?;
            scratch.stage(BootstrapMap::TermRoutes, term_map)?;
        } else {
            next.current = apply_edits(store, lease, &next.current, current_map)?;
            next.membership = apply_edits(store, lease, &next.membership, membership_map)?;
            next.segments = apply_edits(store, lease, &next.segments, segment_map)?;
            next.gram_routes = apply_edits(store, lease, &next.gram_routes, gram_map)?;
            next.term_routes = apply_edits(store, lease, &next.term_routes, term_map)?;
        }
        next.next_segment_id = next.next_segment_id.checked_add(1).ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native segment ID overflow".into())
        })?;
    }
    if let Some(scratch) = bootstrap {
        store.flush_adoption_batch()?;
        let roots = scratch.finish(store, lease)?;
        next.current = roots.current;
        next.membership = roots.membership;
        next.segments = roots.segments;
        next.gram_routes = roots.gram_routes;
        next.term_routes = roots.term_routes;
    }
    next.live_docs = next.current.entries;
    next.validate()?;
    let shards_written = next
        .next_segment_id
        .saturating_sub(inherited.next_segment_id);
    let receipt = TextAuthorityWriteReceipt {
        docs_derived: u64::try_from(added.len()).map_err(|error| {
            CoreError::InvalidContract(format!("lexical: Native added count width: {error}"))
        })?,
        docs_retired: if clear_all {
            inherited.live_docs
        } else {
            u64::try_from(retired.len()).map_err(|error| {
                CoreError::InvalidContract(format!("lexical: Native retired count width: {error}"))
            })?
        },
        compaction_rederived_docs,
        compaction_rederived_text_bytes,
        shards_written,
        shards_inherited: next.segments.entries.saturating_sub(shards_written),
        posting_pages_written: posting_stats.pages,
        oversized_posting_pages: posting_stats.oversized_pages,
        oversized_posting_bytes: posting_stats.oversized_bytes,
    };
    Ok((next, receipt))
}

pub(crate) fn read_route_inventory(
    store: &ObjectStore,
    descriptor: &SegmentDescriptor,
) -> Result<Vec<Vec<u8>>, CoreError> {
    read_route_inventory_budget(store, descriptor, None)
}

pub(crate) fn read_route_inventory_budget(
    store: &ObjectStore,
    descriptor: &SegmentDescriptor,
    budget: Option<&RequestBudgetV1>,
) -> Result<Vec<Vec<u8>>, CoreError> {
    let reference = descriptor.route_inventory.as_ref().ok_or_else(|| {
        CoreError::InvalidContract("lexical: Native segment lacks route inventory".into())
    })?;
    if reference.root.kind != ObjectKind::Record {
        return Err(CoreError::InvalidContract(
            "lexical: Native route inventory kind differs".into(),
        ));
    }
    let bytes = store.read_blob(reference, 128 * 1024 * 1024, budget)?;
    let (version, routes): (u16, Vec<Vec<u8>>) = crate::channel_payloads::decode_cbor_exact(&bytes)
        .map_err(|error| {
            CoreError::Storage(format!("lexical: decode Native route inventory: {error}"))
        })?;
    if version != 1
        || routes
            .windows(2)
            .any(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a, b)| a >= b))
        || routes
            .iter()
            .any(|route| route.len() < 9 || !route.ends_with(&descriptor.id.to_be_bytes()))
    {
        return Err(CoreError::InvalidContract(
            "lexical: Native route inventory is not canonical".into(),
        ));
    }
    Ok(routes)
}

pub(crate) fn read_segment(
    store: &ObjectStore,
    root: &NativeRoot,
    id: u64,
) -> Result<SegmentDescriptor, CoreError> {
    read_segment_budget(store, root, id, None)
}

fn read_segment_budget(
    store: &ObjectStore,
    root: &NativeRoot,
    id: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<SegmentDescriptor, CoreError> {
    let reference = store
        .map_get(&root.segments, &segment_key(id), budget)?
        .ok_or_else(|| CoreError::InvalidContract("lexical: Native segment is missing".into()))?;
    if reference.root.kind != ObjectKind::Segment {
        return Err(CoreError::InvalidContract(
            "lexical: Native segment kind differs".into(),
        ));
    }
    let bytes = store.read_blob(&reference, 64 * 1024, budget)?;
    let descriptor = SegmentDescriptor::decode(&bytes)?;
    if descriptor.id != id {
        return Err(CoreError::InvalidContract(
            "lexical: Native segment key/value differs".into(),
        ));
    }
    Ok(descriptor)
}

pub(crate) fn source_ids(
    store: &ObjectStore,
    root: &NativeRoot,
    source: &SourceFileKey,
) -> Result<Vec<u64>, CoreError> {
    let digest = source_digest(source)?;
    let mut ids = Vec::new();
    store.map_scan(
        &root.membership,
        &digest,
        prefix_upper(&digest).as_deref(),
        None,
        &mut |key, reference| {
            if key.len() != 40 || !key.starts_with(&digest) {
                return Err(CoreError::InvalidContract(
                    "lexical: Native membership key differs".into(),
                ));
            }
            let id = u64::from_be_bytes(
                key.get(32..40)
                    .ok_or_else(|| {
                        CoreError::InvalidContract(
                            "lexical: Native membership ID suffix missing".into(),
                        )
                    })?
                    .try_into()
                    .map_err(|error| {
                        CoreError::InvalidContract(format!(
                            "lexical: Native membership ID width: {error}"
                        ))
                    })?,
            );
            let row = CurrentRecord::decode(&store.read_blob(
                reference,
                MAX_CURRENT_RECORD_BYTES,
                None,
            )?)?;
            if row.id != id || row.source != *source || ids.last().is_some_and(|prior| *prior >= id)
            {
                return Err(CoreError::InvalidContract(
                    "lexical: Native membership record differs from source".into(),
                ));
            }
            ids.push(id);
            Ok(())
        },
    )?;
    Ok(ids)
}

fn prepared_matches_target(
    store: &ObjectStore,
    root: &NativeRoot,
    packet: &PreparedNativeCompaction,
) -> Result<bool, CoreError> {
    let due = store.map_scan_limit(
        &root.compaction_due,
        &[0],
        None,
        packet.selected.len(),
        None,
    )?;
    if due.len() != packet.selected.len() || packet.current_witnesses.len() != packet.docs.len() {
        return Ok(false);
    }
    for ((key, reference), (id, expected, descriptor)) in due.iter().zip(&packet.selected) {
        if key.as_slice() != segment_key(*id).as_slice()
            || reference != expected
            || store.map_get(&root.segments, key, None)?.as_ref() != Some(expected)
            || read_segment(store, root, *id)?.encode()? != descriptor.encode()?
        {
            return Ok(false);
        }
    }
    for ((id, current_ref, membership_ref), doc) in
        packet.current_witnesses.iter().zip(&packet.docs)
    {
        if *id != doc.doc_id
            || store
                .map_get(&root.current, &current_key(*id), None)?
                .as_ref()
                != Some(current_ref)
            || store
                .map_get(
                    &root.membership,
                    &membership_key(&source_digest(&doc.source_file)?, *id),
                    None,
                )?
                .as_ref()
                != Some(membership_ref)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Pay at most two queued segment debts on one publication.
///
/// Every selected live document is re-derived from its committed body and current locator;
/// the replacement is published in the same immutable-root transaction as
/// removal of the old route inventory.
fn compact_due_segments(
    store: &ObjectStore,
    lease: &AttemptLease,
    root: &mut NativeRoot,
    generation: ManifestGeneration,
    added: &[AddedTextDoc],
    prepared: Option<PreparedNativeCompaction>,
) -> Result<(usize, PostingPageStats, u64, u64), CoreError> {
    // The prior map edits are durable before reading and re-deriving old
    // bodies; this bounded compaction preparation does not hold the gate.
    store.flush_adoption_batch()?;
    let due = store.map_scan_limit(&root.compaction_due, &[0], None, 2, None)?;
    if due.is_empty() {
        return Ok((0, PostingPageStats::default(), 0, 0));
    }
    let prepared = match prepared {
        Some(packet) if prepared_matches_target(store, root, &packet)? => Some(packet),
        _ => None,
    };
    let (selected, mut docs) = if let Some(packet) = prepared {
        let selected = packet
            .selected
            .into_iter()
            .map(|(_, _, descriptor)| descriptor)
            .collect::<Vec<_>>();
        (selected, packet.docs)
    } else {
        let mut selected = Vec::new();
        let mut docs = Vec::new();
        for (key, reference) in due {
            let id = u64::from_be_bytes(key.as_slice().try_into().map_err(|_error| {
                CoreError::InvalidContract("lexical: Native compaction key width differs".into())
            })?);
            let segment_ref = store.map_get(&root.segments, &key, None)?.ok_or_else(|| {
                CoreError::InvalidContract("lexical: due segment vanished".into())
            })?;
            if segment_ref != reference {
                return Err(CoreError::InvalidContract(
                    "lexical: due segment version differs from committed descriptor".into(),
                ));
            }
            let descriptor = read_segment(store, root, id)?;
            let segment_docs = live_segment_docs(store, root, &descriptor)?;
            let added_bytes = segment_docs
                .iter()
                .try_fold(0_usize, |sum, doc| sum.checked_add(doc.text.len()))
                .ok_or_else(|| {
                    CoreError::InvalidContract("lexical: compacted byte count overflow".into())
                })?;
            let prior_bytes = docs
                .iter()
                .try_fold(0_usize, |sum: usize, doc: &AddedTextDoc| {
                    sum.checked_add(doc.text.len())
                })
                .ok_or_else(|| {
                    CoreError::InvalidContract("lexical: compacted byte count overflow".into())
                })?;
            if !selected.is_empty()
                && (docs.len().saturating_add(segment_docs.len()) > SEGMENT_DOCS
                    || prior_bytes.saturating_add(added_bytes) > SEGMENT_TARGET_TEXT_BYTES)
            {
                break;
            }
            docs.extend(segment_docs);
            selected.push(descriptor);
        }
        (selected, docs)
    };
    if selected.is_empty() || docs.is_empty() {
        return Err(CoreError::InvalidContract(
            "lexical: Native compaction selected no live segment".into(),
        ));
    }
    let compaction_rederived_docs = u64::try_from(docs.len()).map_err(|error| {
        CoreError::InvalidContract(format!("lexical: compaction document count width: {error}"))
    })?;
    let compaction_rederived_text_bytes = docs.iter().try_fold(0_u64, |sum, doc| {
        let bytes = u64::try_from(doc.text.len()).map_err(|error| {
            CoreError::InvalidContract(format!("lexical: compaction text width: {error}"))
        })?;
        sum.checked_add(bytes).ok_or_else(|| {
            CoreError::InvalidContract("lexical: compaction text byte count overflow".into())
        })
    })?;
    let compacted_segments = u64::try_from(selected.len()).map_err(|error| {
        CoreError::InvalidContract(format!("lexical: compaction segment count width: {error}"))
    })?;
    store.record_compaction(
        compacted_segments,
        compaction_rederived_docs,
        compaction_rederived_text_bytes,
    );
    let existing_ids = docs.iter().map(|doc| doc.doc_id).collect::<BTreeSet<_>>();
    let mut consumed = 0_usize;
    let mut text_bytes = docs.iter().try_fold(0_usize, |sum, doc| {
        sum.checked_add(doc.text.len()).ok_or_else(|| {
            CoreError::InvalidContract("lexical: compacted byte count overflow".into())
        })
    })?;
    while let Some(doc) = added.get(consumed) {
        let next_bytes = text_bytes.checked_add(doc.text.len()).ok_or_else(|| {
            CoreError::InvalidContract("lexical: compacted byte count overflow".into())
        })?;
        if docs.len() >= SEGMENT_DOCS || next_bytes > SEGMENT_TARGET_TEXT_BYTES {
            break;
        }
        docs.push(doc.clone());
        text_bytes = next_bytes;
        consumed = consumed.saturating_add(1);
    }
    docs.sort_unstable_by_key(|doc| doc.doc_id);
    if docs.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(a, b)| a.doc_id == b.doc_id)
    }) {
        return Err(CoreError::InvalidContract(
            "lexical: compaction source segments share a live logical ID".into(),
        ));
    }
    store.flush_adoption_batch()?;
    let replacement = BuiltSegment::build(root.next_segment_id, generation, &docs)?;
    let mut current_map = BTreeMap::new();
    let mut membership_map = BTreeMap::new();
    let mut segment_map = BTreeMap::new();
    let mut gram_map = BTreeMap::new();
    let mut term_map = BTreeMap::new();
    let mut due_map = BTreeMap::new();
    let posting_stats = write_segment(
        store,
        lease,
        root,
        &replacement,
        &docs,
        &mut current_map,
        &mut membership_map,
        &mut segment_map,
        &mut gram_map,
        &mut term_map,
        Some(&existing_ids),
    )?;
    for descriptor in selected {
        for route in read_route_inventory(store, &descriptor)? {
            let target = match route.first().copied() {
                Some(GRAM_RAW | GRAM_FOLDED) => &mut gram_map,
                Some(TERM_RAW | TERM_FOLDED) => &mut term_map,
                _ => {
                    return Err(CoreError::InvalidContract(
                        "lexical: compacted route has unknown surface".into(),
                    ));
                }
            };
            checked_insert(target, route, None)?;
        }
        checked_insert(&mut segment_map, segment_key(descriptor.id).to_vec(), None)?;
        checked_insert(&mut due_map, segment_key(descriptor.id).to_vec(), None)?;
    }
    root.current = apply_edits(store, lease, &root.current, current_map)?;
    root.membership = apply_edits(store, lease, &root.membership, membership_map)?;
    root.segments = apply_edits(store, lease, &root.segments, segment_map)?;
    root.gram_routes = apply_edits(store, lease, &root.gram_routes, gram_map)?;
    root.term_routes = apply_edits(store, lease, &root.term_routes, term_map)?;
    root.compaction_due = apply_edits(store, lease, &root.compaction_due, due_map)?;
    root.next_segment_id = root.next_segment_id.checked_add(1).ok_or_else(|| {
        CoreError::InvalidContract("lexical: Native compaction segment ID overflow".into())
    })?;
    root.live_docs = root.current.entries;
    root.validate()?;
    Ok((
        consumed,
        posting_stats,
        compaction_rederived_docs,
        compaction_rederived_text_bytes,
    ))
}

fn live_segment_docs(
    store: &ObjectStore,
    root: &NativeRoot,
    descriptor: &SegmentDescriptor,
) -> Result<Vec<AddedTextDoc>, CoreError> {
    live_segment_docs_admitted(store, root, descriptor, None, None)
}

fn live_segment_docs_admitted(
    store: &ObjectStore,
    root: &NativeRoot,
    descriptor: &SegmentDescriptor,
    mut admission: Option<&mut ScratchAdmission>,
    budget: Option<&RequestBudgetV1>,
) -> Result<Vec<AddedTextDoc>, CoreError> {
    if let Some(admission) = admission.as_deref_mut() {
        admission.charge(
            u64::try_from(descriptor.live_locals.len())
                .map_err(|_error| compaction_scratch_limit())?
                .checked_mul(
                    u64::try_from(std::mem::size_of::<AddedTextDoc>())
                        .map_err(|_error| compaction_scratch_limit())?,
                )
                .ok_or_else(compaction_scratch_limit)?,
        )?;
    }
    let mut docs = Vec::new();
    docs.try_reserve_exact(descriptor.live_locals.len())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: reserve Native compaction documents: {error}"
            ))
        })?;
    for local in &descriptor.live_locals {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:native:prepare:body")?;
        }
        let id = *descriptor
            .logical_ids
            .get(usize::try_from(*local).map_err(|_error| {
                CoreError::InvalidContract("lexical: compaction local ID width differs".into())
            })?)
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: compaction local ID absent".into())
            })?;
        let (current_ref, current) = read_current_with_ref_budget(store, root, id, budget)?
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: compaction live current ID absent".into())
            })?;
        if let Some(admission) = admission.as_deref_mut() {
            admission.charge(current_ref.bytes)?;
        }
        if current.segment_key != descriptor.id || current.local_id != *local {
            return Err(CoreError::InvalidContract(
                "lexical: compaction current locator differs".into(),
            ));
        }
        if current.body.root.kind != ObjectKind::NativeBody {
            return Err(CoreError::InvalidContract(
                "lexical: compaction body kind differs".into(),
            ));
        }
        if let Some(admission) = admission.as_deref_mut() {
            // The raw logical blob and the decoded candidate/indexed strings
            // each require at most the committed logical byte count. Charge
            // both before either body buffer is allocated.
            admission.charge(
                current
                    .body
                    .bytes
                    .checked_mul(2)
                    .ok_or_else(compaction_scratch_limit)?,
            )?;
        }
        let bytes = store.read_blob(&current.body, MAX_BODY_BYTES, budget)?;
        let (candidate_id, indexed, _folded) = match admission.as_deref() {
            Some(admission) => decode_body_for_preparation(
                &bytes,
                usize::try_from(admission.charged).map_err(|_error| compaction_scratch_limit())?,
                usize::try_from(admission.limit).map_err(|_error| compaction_scratch_limit())?,
            )?,
            None => decode_body(&bytes)?,
        };
        let indexed_sha256: [u8; 32] = Sha256::digest(indexed.as_bytes()).into();
        if candidate_id != current.candidate_id || indexed_sha256 != current.indexed_sha256 {
            return Err(CoreError::InvalidContract(
                "lexical: compaction body differs from current identity".into(),
            ));
        }
        let digest = source_digest(&current.source)?;
        let member = store
            .map_get(&root.membership, &membership_key(&digest, id), budget)?
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: compacted membership absent".into())
            })?;
        let member_record =
            CurrentRecord::decode(&store.read_blob(&member, MAX_CURRENT_RECORD_BYTES, budget)?)?;
        if member_record.id != id
            || member_record.segment_key != descriptor.id
            || member_record.local_id != *local
            || member_record.source != current.source
            || member_record.candidate_id != current.candidate_id
            || member_record.indexed_sha256 != current.indexed_sha256
        {
            return Err(CoreError::InvalidContract(
                "lexical: compacted membership differs from current".into(),
            ));
        }
        docs.push(AddedTextDoc {
            doc_id: id,
            source_file: current.source,
            candidate_id,
            text: indexed,
        });
    }
    Ok(docs)
}

fn segment_groups(
    docs: &[AddedTextDoc],
) -> impl Iterator<Item = Result<&[AddedTextDoc], CoreError>> {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start >= docs.len() {
            return None;
        }
        let mut end = start;
        let mut bytes = 0_usize;
        while end < docs.len() && end.saturating_sub(start) < SEGMENT_DOCS {
            let next = bytes.saturating_add(docs.get(end)?.text.len());
            if end > start && next > SEGMENT_TARGET_TEXT_BYTES {
                break;
            }
            bytes = next;
            end = end.saturating_add(1);
        }
        let group_start = start;
        start = end;
        Some(docs.get(group_start..end).ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: Native segment group outside added documents".into(),
            )
        }))
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "one segment write updates its coupled current, membership, segment and posting maps atomically"
)]
fn write_segment(
    store: &ObjectStore,
    lease: &AttemptLease,
    base: &NativeRoot,
    segment: &BuiltSegment,
    docs: &[AddedTextDoc],
    current_map: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    membership_map: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    segment_map: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    gram_map: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    term_map: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
    replace_existing: Option<&BTreeSet<u64>>,
) -> Result<PostingPageStats, CoreError> {
    let mut inventory = Vec::new();
    inventory
        .try_reserve(segment.grams.len().saturating_add(segment.terms.len()))
        .map_err(|error| {
            CoreError::Storage(format!("lexical: reserve Native route inventory: {error}"))
        })?;
    for posting in &segment.grams {
        let gram: [u8; 3] = posting.key.as_slice().try_into().map_err(|_error| {
            CoreError::InvalidContract("lexical: Native gram key width differs".into())
        })?;
        inventory.push(route_key(
            &gram_prefix(posting.surface, gram),
            segment.descriptor.id,
        ));
    }
    for posting in &segment.terms {
        let term = std::str::from_utf8(&posting.key).map_err(|error| {
            CoreError::InvalidContract(format!("lexical: Native term UTF-8: {error}"))
        })?;
        inventory.push(route_key(
            &term_prefix(posting.surface, term)?,
            segment.descriptor.id,
        ));
    }
    let inventory_count = inventory.len();
    inventory.sort_unstable();
    inventory.dedup();
    if inventory.len() != inventory_count {
        return Err(CoreError::InvalidContract(
            "lexical: duplicate Native segment route".into(),
        ));
    }
    let inventory_bytes =
        crate::channel_payloads::encode_cbor(&(1_u16, &inventory), "Native route inventory")?;
    let inventory_ref = store.put_blob(lease, ObjectKind::Record, &[], &inventory_bytes)?;
    let mut descriptor = segment.descriptor.clone();
    descriptor.route_inventory = Some(inventory_ref.clone());
    let descriptor_bytes = descriptor.encode()?;
    let descriptor_ref = store.put_blob(
        lease,
        ObjectKind::Segment,
        &[inventory_ref.root],
        &descriptor_bytes,
    )?;
    checked_insert(
        segment_map,
        segment_key(segment.descriptor.id).to_vec(),
        Some(descriptor_ref),
    )?;
    for body in &segment.bodies {
        let input = docs
            .get(usize::try_from(body.local_id).map_err(|error| {
                CoreError::InvalidContract(format!("lexical: Native body local width: {error}"))
            })?)
            .ok_or_else(|| {
                CoreError::InvalidContract("lexical: Native body local missing".into())
            })?;
        let expected_logical_id = input.doc_id;
        let expected_candidate_id = &input.candidate_id;
        let logical_id_matches = expected_logical_id == body.logical_id;
        let candidate_id_matches = expected_candidate_id == &body.candidate_id;
        if !logical_id_matches || !candidate_id_matches {
            return Err(CoreError::InvalidContract(
                "lexical: Native body and input differ".into(),
            ));
        }
        let existing = read_current(store, base, body.logical_id)?;
        match (replace_existing, existing) {
            (None, None) => {}
            (None, Some(_)) => {
                return Err(CoreError::InvalidContract(
                    "lexical: Native allocation reuses an existing logical ID".into(),
                ));
            }
            (Some(expected), Some(prior)) if expected.contains(&body.logical_id) => {
                let source_matches = prior.source == input.source_file;
                let candidate_matches = prior.candidate_id == body.candidate_id;
                if !source_matches
                    || !candidate_matches
                    || prior.indexed_sha256 != body.indexed_sha256
                {
                    return Err(CoreError::InvalidContract(
                        "lexical: Native compaction input differs from current ID".into(),
                    ));
                }
            }
            (Some(expected), None) if !expected.contains(&body.logical_id) => {}
            (Some(_), _) => {
                return Err(CoreError::InvalidContract(
                    "lexical: Native compaction current membership differs".into(),
                ));
            }
        }
        let source = source_digest(&input.source_file)?;
        let body_ref = store.put_blob(lease, ObjectKind::NativeBody, &[], &body.bytes)?;
        let record = CurrentRecord {
            id: body.logical_id,
            source: input.source_file.clone(),
            candidate_id: body.candidate_id.clone(),
            indexed_sha256: body.indexed_sha256,
            body: body_ref.clone(),
            segment_key: segment.descriptor.id,
            local_id: body.local_id,
        };
        let record_ref = store.put_blob(
            lease,
            ObjectKind::Record,
            &[body_ref.root],
            &record.encode()?,
        )?;
        checked_insert(
            current_map,
            current_key(body.logical_id).to_vec(),
            Some(record_ref.clone()),
        )?;
        checked_insert(
            membership_map,
            membership_key(&source, body.logical_id),
            Some(record_ref),
        )?;
    }
    let mut posting_stats = write_posting_pages(
        store,
        lease,
        segment.descriptor.id,
        &segment.grams,
        ObjectKind::NativeTrigrams,
        gram_map,
    )?;
    posting_stats.add(write_posting_pages(
        store,
        lease,
        segment.descriptor.id,
        &segment.terms,
        ObjectKind::NativePositions,
        term_map,
    )?)?;
    Ok(posting_stats)
}

fn write_posting_pages(
    store: &ObjectStore,
    lease: &AttemptLease,
    segment_key: u64,
    postings: &[BuiltPosting],
    kind: ObjectKind,
    edits: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
) -> Result<PostingPageStats, CoreError> {
    let mut page = Vec::<&BuiltPosting>::new();
    let mut page_bytes = HEADER_BYTES;
    let mut stats = PostingPageStats::default();
    for posting in postings {
        let row_bytes = PostingPage::encoded_len_for(posting)?;
        let next_bytes = page_bytes.checked_add(row_bytes).ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native posting page length overflow".into())
        })?;
        if u64::try_from(HEADER_BYTES.saturating_add(row_bytes))
            .map_or(true, |len| len > MAX_POSTING_BYTES)
        {
            return Err(CoreError::InvalidContract(
                "lexical: Native posting exceeds admitted bound".into(),
            ));
        }
        if !page.is_empty() && next_bytes > TARGET_BYTES {
            stats.add(flush_posting_page(
                store,
                lease,
                segment_key,
                kind,
                &page,
                edits,
            )?)?;
            page.clear();
            page_bytes = HEADER_BYTES;
        }
        page_bytes = page_bytes.checked_add(row_bytes).ok_or_else(|| {
            CoreError::InvalidContract("lexical: Native posting page length overflow".into())
        })?;
        page.try_reserve(1).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: reserve Native posting page rows: {error}"
            ))
        })?;
        page.push(posting);
    }
    if !page.is_empty() {
        stats.add(flush_posting_page(
            store,
            lease,
            segment_key,
            kind,
            &page,
            edits,
        )?)?;
    }
    Ok(stats)
}

fn flush_posting_page(
    store: &ObjectStore,
    lease: &AttemptLease,
    segment_key: u64,
    kind: ObjectKind,
    page: &[&BuiltPosting],
    edits: &mut BTreeMap<Vec<u8>, Option<BlobRef>>,
) -> Result<PostingPageStats, CoreError> {
    let bytes = PostingPage::encode(segment_key, page)?;
    if u64::try_from(bytes.len()).map_or(true, |len| len > MAX_POSTING_BYTES) {
        return Err(CoreError::InvalidContract(
            "lexical: Native posting page exceeds admitted bound".into(),
        ));
    }
    // An oversized singleton remains one bounded logical blob; the object
    // store splits its physical representation into fixed-size blocks.
    let reference = store.put_blob(lease, kind, &[], &bytes)?;
    for posting in page {
        let prefix = match kind {
            ObjectKind::NativeTrigrams => {
                if !matches!(posting.surface, GRAM_RAW | GRAM_FOLDED) {
                    return Err(CoreError::InvalidContract(
                        "lexical: Native gram surface differs".into(),
                    ));
                }
                let gram: [u8; 3] = posting.key.as_slice().try_into().map_err(|_error| {
                    CoreError::InvalidContract("lexical: Native gram key width differs".into())
                })?;
                gram_prefix(posting.surface, gram).to_vec()
            }
            ObjectKind::NativePositions => {
                if !matches!(posting.surface, TERM_RAW | TERM_FOLDED) {
                    return Err(CoreError::InvalidContract(
                        "lexical: Native term surface differs".into(),
                    ));
                }
                let term = std::str::from_utf8(&posting.key).map_err(|error| {
                    CoreError::InvalidContract(format!("lexical: Native term UTF-8: {error}"))
                })?;
                term_prefix(posting.surface, term)?
            }
            ObjectKind::FileBody
            | ObjectKind::FilePostings
            | ObjectKind::NativeBody
            | ObjectKind::Segment
            | ObjectKind::Record
            | ObjectKind::MapPage
            | ObjectKind::BlobBlock
            | ObjectKind::BlobNode
            | ObjectKind::AttemptBatch => {
                return Err(CoreError::InvalidContract(
                    "lexical: Native posting object kind differs".into(),
                ));
            }
        };
        checked_insert(
            edits,
            route_key(&prefix, segment_key),
            Some(reference.clone()),
        )?;
    }
    let oversized = bytes.len() > TARGET_BYTES;
    Ok(PostingPageStats {
        pages: 1,
        oversized_pages: u64::from(oversized),
        oversized_bytes: if oversized {
            u64::try_from(bytes.len()).map_err(|error| {
                CoreError::InvalidContract(format!(
                    "lexical: Native oversized page byte width: {error}"
                ))
            })?
        } else {
            0
        },
    })
}
