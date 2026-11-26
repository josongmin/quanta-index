//! Cold inventory: an immutable map of upper term fences to occurrence blocks.
//! Only the original ordered SQL edit stream supplies directory entries.
use super::super::entry_edits::Edits;
pub(in crate::native::text) use crate::native::text_inventory_descriptor::descriptor;
use crate::native::text_inventory_descriptor::{LEAF, PACKED, corrupt, decode};
use crate::native::text_memory::{PreparedKey, PreparedRows};
use crate::native::text_occurrence::{
    OccurrenceReader,
    block::{self, CheckedBlock, MAX_BLOCK_TERMS},
};
use crate::native::text_spool::{
    ReadyInventoryCursor, ReadyInventoryReadLease, RootCursor, check_term_key,
};
use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MapPage, MapPageEntry, MapRoot, MapValue, MemoryLease,
    MemoryLifetime, ObjectRef, OrdinalMapEdit, StagedMapRoots, WorkBudget, WorkUsage,
    ports::{
        ImmutableObjectPort, MapEditCursor, PersistentMapPort, RootPackingMode, RootPackingPlan,
    },
};

fn packed(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    terms: usize,
    fence: &[u8],
    budget: &mut WorkBudget,
) -> Result<CheckedBlock, IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.registration.reference != reference {
        return Err(corrupt("packed identity differs"));
    }
    let checked = CheckedBlock::parse(stored, budget)?;
    let last = terms.checked_sub(1).ok_or(IndexError::ResourceLimited)?;
    if checked.term_count() != terms || checked.key(last)? != fence {
        return Err(corrupt("packed count or upper fence differs"));
    }
    Ok(checked)
}

enum IntervalAnchor<'a> {
    First,
    Previous(&'a [u8]),
    Random,
}

/// A descriptor's first key must follow its predecessor fence.
///
/// Sequential readers already retain that fence; random readers prove it with
/// two bounded point/seek reads, including equality at the first key.
fn checked_packed_interval(
    maps: &dyn PersistentMapPort,
    root: ObjectRef,
    fence: &[u8],
    value: MapValue,
    checked: &CheckedBlock,
    anchor: &IntervalAnchor<'_>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let first = checked.key(0)?;
    match anchor {
        IntervalAnchor::First => return Ok(()),
        IntervalAnchor::Previous(previous) => {
            if first <= *previous {
                return Err(corrupt("packed first key crosses previous fence"));
            }
            return Ok(());
        }
        IntervalAnchor::Random => {}
    }
    if maps.lookup(MapRoot(Some(root)), first, budget)?.is_some() {
        return Err(corrupt("packed first key is an existing upper fence"));
    }
    let page = ordered_descriptor_page(
        maps.scan(MapRoot(Some(root)), Some(first), 1, budget)?,
        Some(first),
        false,
    )?;
    if let Some((selected, actual)) = page.entries.first() {
        let _decoded = decode(*actual)?;
        if selected == fence && *actual == value {
            return Ok(());
        }
    }
    Err(corrupt("packed first key selects a different descriptor"))
}

fn check_next_interval(
    objects: &dyn ImmutableObjectPort,
    maps: &dyn PersistentMapPort,
    root: ObjectRef,
    fence: &[u8],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let successor = ordered_descriptor_page(
        maps.scan(MapRoot(Some(root)), Some(fence), 1, budget)?,
        Some(fence),
        false,
    )?;
    if let Some((next_fence, next_value)) = successor.entries.first() {
        let (next_kind, next_terms, next_reference) = decode(*next_value)?;
        if next_kind == PACKED {
            let next = packed(objects, next_reference, next_terms, next_fence, budget)?;
            checked_packed_interval(
                maps,
                root,
                next_fence,
                *next_value,
                &next,
                &IntervalAnchor::Random,
                budget,
            )?;
        }
    }
    Ok(())
}
fn leaf(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    leaf_position_count(objects, reference, key, budget).map(|_| ())
}

fn leaf_position_count(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<u64, IndexError> {
    let mut reader = OccurrenceReader::uncached()?;
    let count = reader.count(reference, key, objects, budget)?;
    if !count.leaf || count.value == 0 {
        return Err(corrupt("overflow child is not a valid leaf"));
    }
    Ok(count.value)
}

fn ordered_descriptor_page(
    page: MapPage,
    after: Option<&[u8]>,
    promised: bool,
) -> Result<MapPage, IndexError> {
    if page.entries.len() > 1 || (page.entries.is_empty() && (page.has_more || promised)) {
        return Err(IndexError::BackendViolation(
            "inventory directory map page exceeded bound or claimed empty continuation".into(),
        ));
    }
    if let Some((key, _value)) = page.entries.first()
        && after.is_some_and(|prior| key.as_slice() <= prior)
    {
        return Err(IndexError::BackendViolation(
            "inventory directory map page did not advance".into(),
        ));
    }
    Ok(page)
}

struct ProducerState {
    ordinal: u32,
    previous: Option<OrdinalMapEdit>,
    digest: Edits,
    rows: u64,
    descriptors: u64,
    expected_rows: u64,
    anchored: bool,
    eof: bool,
}
impl ProducerState {
    fn new(ordinal: u32, expected_rows: u64, anchored: bool) -> Self {
        Self {
            ordinal,
            previous: None,
            digest: Edits::new(),
            rows: 0,
            descriptors: 0,
            expected_rows,
            anchored,
            eof: false,
        }
    }

    fn original(
        &mut self,
        input: &mut dyn MapEditCursor,
        budget: &mut WorkBudget,
    ) -> Result<Option<OrdinalMapEdit>, IndexError> {
        let Some(edit) = input.next_edit(budget)? else {
            self.eof = true;
            return Ok(None);
        };
        if edit.root() != self.ordinal
            || self
                .previous
                .as_ref()
                .is_some_and(|prior| prior.key() >= edit.key())
            || edit.expected_wire().is_some()
            || edit.next_wire().is_none()
        {
            return Err(corrupt("original CAS, ordinal or order differs"));
        }
        check_term_key(edit.key())?;
        if self.anchored {
            self.digest.push_descriptor(&edit, budget)?;
        } else {
            self.digest.push(&edit, budget)?;
        }
        self.rows = self
            .rows
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if self.rows > self.expected_rows {
            return Err(corrupt("original count exceeds declared count"));
        }
        self.previous = Some(edit.clone());
        Ok(Some(edit))
    }
    fn next(
        &mut self,
        input: &mut dyn MapEditCursor,
        objects: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        if self.eof {
            return Ok(None);
        }
        let Some(first) = self.original(input, budget)? else {
            if self.rows != self.expected_rows {
                return Err(corrupt("original count or EOF differs"));
            }
            return Ok(None);
        };
        if self.anchored {
            // The source-owned descriptor is disposable DATA. Authentication
            // of every child remains in the independent post-map visitor.
            self.descriptors = self
                .descriptors
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            return Ok(Some(first));
        }
        let reference = first
            .next()?
            .ok_or_else(|| corrupt("next absent"))?
            .object_ref()?;
        let stored = objects.get(reference, budget)?;
        if stored.registration.reference != reference || !stored.registration.children.is_empty() {
            return Err(corrupt("occurrence child identity or edges differ"));
        }
        let (kind, terms, fence) = if block::is_packed(&stored.payload) {
            let checked = CheckedBlock::parse(stored, budget)?;
            let terms = checked.term_count();
            if first.key() != checked.key(0)? {
                return Err(corrupt("packed first key differs"));
            }
            let mut last = first;
            for slot in 1..terms {
                let edit = self
                    .original(input, budget)?
                    .ok_or_else(|| corrupt("packed group truncated"))?;
                if edit.key() != checked.key(slot)?
                    || edit
                        .next()?
                        .ok_or_else(|| corrupt("packed next absent"))?
                        .object_ref()?
                        != reference
                {
                    return Err(corrupt("packed key or reference differs"));
                }
                last = edit;
            }
            (PACKED, terms, last)
        } else {
            let _count =
                crate::native::text_occurrence::stored_leaf_count(&stored, reference, budget)?;
            (LEAF, 1, first)
        };
        self.descriptors = self
            .descriptors
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        OrdinalMapEdit::copy_key(
            self.ordinal,
            fence.key(),
            None,
            Some(descriptor(kind, terms, reference)?),
            budget,
        )
        .map(Some)
    }

    fn finish(&self) -> Result<ProducerReceipt, IndexError> {
        if !self.eof || self.rows != self.expected_rows || self.descriptors == 0 {
            return Err(corrupt("directory producer did not reach complete EOF"));
        }
        Ok(ProducerReceipt {
            digest: self.digest.finish().digest,
            descriptors: self.descriptors,
        })
    }
}

#[derive(Clone, Copy)]
pub(in crate::native::text) struct ProducerReceipt {
    pub digest: Digest,
    pub descriptors: u64,
}

struct Producer<'a> {
    input: &'a mut dyn MapEditCursor,
    objects: &'a dyn ImmutableObjectPort,
    state: ProducerState,
}

impl MapEditCursor for Producer<'_> {
    fn subtree_packing(&self) -> Result<RootPackingPlan, IndexError> {
        let mut packing = RootPackingPlan::default();
        packing.select(0, RootPackingMode::Nested)?;
        Ok(packing)
    }

    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        self.state.next(self.input, self.objects, budget)
    }
}

pub(in crate::native::text) fn build(
    input: &mut dyn MapEditCursor,
    count: u64,
    context: &mut AttemptContext<'_>,
) -> Result<(ObjectRef, ProducerReceipt), IndexError> {
    build_inner(input, count, false, context)
}

pub(in crate::native::text) fn build_anchors(
    input: &mut dyn MapEditCursor,
    count: u64,
    context: &mut AttemptContext<'_>,
) -> Result<(ObjectRef, ProducerReceipt), IndexError> {
    build_inner(input, count, true, context)
}

fn build_inner(
    input: &mut dyn MapEditCursor,
    count: u64,
    anchored: bool,
    context: &mut AttemptContext<'_>,
) -> Result<(ObjectRef, ProducerReceipt), IndexError> {
    let objects = context.objects;
    let mut producer = Producer {
        input,
        objects,
        state: ProducerState::new(0, count, anchored),
    };
    let roots = context.apply_stream(&[MapRoot(None)], &mut producer)?;
    let digest = producer.state.finish()?;
    let root = roots
        .first()
        .and_then(|root| root.0)
        .ok_or_else(|| corrupt("directory map root absent"))?;
    Ok((root, digest))
}

pub(in crate::native::text) struct BuildRequest {
    pub slot: usize,
    pub count: u64,
    pub anchored: bool,
}

struct BatchProducer<'a, 'spool> {
    requests: &'a [BuildRequest],
    inventory: &'a ReadyInventoryReadLease<'spool>,
    objects: &'a dyn ImmutableObjectPort,
    active: Option<(
        ReadyInventoryCursor<'a, 'spool, RootCursor<'spool>>,
        ProducerState,
    )>,
    digests: PreparedRows<ProducerReceipt>,
}

impl MapEditCursor for BatchProducer<'_, '_> {
    fn subtree_packing(&self) -> Result<RootPackingPlan, IndexError> {
        let mut packing = RootPackingPlan::default();
        for index in 0..self.requests.len() {
            let ordinal = u32::try_from(index).map_err(|_width| IndexError::ResourceLimited)?;
            packing.select(ordinal, RootPackingMode::Nested)?;
        }
        Ok(packing)
    }

    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        loop {
            budget.checkpoint()?;
            let index = self.digests.len();
            let Some(request) = self.requests.get(index) else {
                return Ok(None);
            };
            if self.active.is_none() {
                let ordinal = u32::try_from(index).map_err(|_width| IndexError::ResourceLimited)?;
                self.active = Some((
                    self.inventory.root_cursor_for(request.slot, ordinal)?,
                    ProducerState::new(ordinal, request.count, request.anchored),
                ));
            }
            let (input, state) = self.active.as_mut().ok_or(IndexError::ResourceLimited)?;
            if let Some(edit) = state.next(input, self.objects, budget)? {
                return Ok(Some(edit));
            }
            self.digests.push(state.finish()?, budget)?;
            self.active = None;
        }
    }
}

pub(in crate::native::text) fn build_batch(
    requests: &[BuildRequest],
    inventory: &ReadyInventoryReadLease<'_>,
    context: &mut AttemptContext<'_>,
) -> Result<(StagedMapRoots, PreparedRows<ProducerReceipt>), IndexError> {
    let mut bases = PreparedRows::with_capacity(requests.len(), context.budget)?;
    for _request in requests {
        bases.push(MapRoot(None), context.budget)?;
    }
    let mut producer = BatchProducer {
        requests,
        inventory,
        objects: context.objects,
        active: None,
        digests: PreparedRows::with_capacity(requests.len(), context.budget)?,
    };
    let roots = context.apply_stream(bases.as_slice(), &mut producer)?;
    if producer.digests.len() != requests.len() {
        return Err(corrupt("directory batch did not reach complete EOF"));
    }
    inventory.check_epoch(context.budget)?;
    Ok((roots, producer.digests))
}

pub(super) fn lookup(
    objects: &dyn ImmutableObjectPort,
    maps: &dyn PersistentMapPort,
    root: ObjectRef,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<Option<ObjectRef>, IndexError> {
    budget.checkpoint()?;
    if let Some(value) = maps.lookup(MapRoot(Some(root)), key, budget)? {
        let (kind, terms, reference) = decode(value)?;
        if kind == LEAF {
            leaf(objects, reference, key, budget)?;
            check_next_interval(objects, maps, root, key, budget)?;
            return Ok(Some(reference));
        }
        let checked = packed(objects, reference, terms, key, budget)?;
        checked_packed_interval(
            maps,
            root,
            key,
            value,
            &checked,
            &IntervalAnchor::Random,
            budget,
        )?;
        check_next_interval(objects, maps, root, key, budget)?;
        let last = terms.checked_sub(1).ok_or(IndexError::ResourceLimited)?;
        return Ok((checked.key(last)? == key).then_some(reference));
    }
    let page = ordered_descriptor_page(
        maps.scan(MapRoot(Some(root)), Some(key), 1, budget)?,
        Some(key),
        false,
    )?;
    let Some((fence, value)) = page.entries.first() else {
        return Ok(None);
    };
    let (kind, terms, reference) = decode(*value)?;
    if kind == LEAF {
        leaf(objects, reference, fence, budget)?;
        check_next_interval(objects, maps, root, fence, budget)?;
        return Ok(None);
    }
    let checked = packed(objects, reference, terms, fence, budget)?;
    checked_packed_interval(
        maps,
        root,
        fence,
        *value,
        &checked,
        &IntervalAnchor::Random,
        budget,
    )?;
    let slot = checked.lower_bound(key, budget)?;
    if slot < terms && checked.key(slot)? == key {
        return Ok(Some(reference));
    }
    // A miss inside this descriptor could be a key incorrectly hidden in the
    // next overlapping block. Check that one successor's interval before
    // reporting absence; a leaf successor cannot overlap an earlier fence.
    check_next_interval(objects, maps, root, fence, budget)?;
    Ok(None)
}

pub(super) fn scan(
    objects: &dyn ImmutableObjectPort,
    maps: &dyn PersistentMapPort,
    root: ObjectRef,
    after: Option<&[u8]>,
    limit: u32,
    budget: &mut WorkBudget,
) -> Result<MapPage, IndexError> {
    budget.checkpoint()?;
    if limit == 0 || limit > 4096 {
        return Err(IndexError::InvalidInput("inventory directory limit".into()));
    }
    let capacity = usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?;
    let allocation = budget.reserve_memory(
        MapPage::allocation_bytes(capacity)?,
        MemoryLifetime::Retained,
    )?;
    let mut entries = Vec::new();
    let mut leases: Vec<MemoryLease> = Vec::new();
    entries
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    leases
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut cursor_after = after
        .map(|key| PreparedKey::copy(key, budget))
        .transpose()?;
    let mut crossed_descriptor = false;
    let mut more = false;
    let mut promised = false;
    loop {
        budget.checkpoint()?;
        let page = ordered_descriptor_page(
            maps.scan(
                MapRoot(Some(root)),
                cursor_after.as_ref().map(PreparedKey::as_slice),
                1,
                budget,
            )?,
            cursor_after.as_ref().map(PreparedKey::as_slice),
            promised,
        )?;
        let Some((fence, value)) = page.entries.first() else {
            // A resume exactly at the final fence selects no successor. Check
            // that fence's own interval before returning an empty page.
            if !crossed_descriptor
                && entries.is_empty()
                && let Some(resume) = after
                && let Some(value) = maps.lookup(MapRoot(Some(root)), resume, budget)?
            {
                let (kind, terms, reference) = decode(value)?;
                if kind == PACKED {
                    let checked = packed(objects, reference, terms, resume, budget)?;
                    checked_packed_interval(
                        maps,
                        root,
                        resume,
                        value,
                        &checked,
                        &IntervalAnchor::Random,
                        budget,
                    )?;
                } else {
                    leaf(objects, reference, resume, budget)?;
                }
            }
            break;
        };
        let (kind, terms, reference) = decode(*value)?;
        if kind == PACKED {
            let checked = packed(objects, reference, terms, fence, budget)?;
            let anchor = if crossed_descriptor {
                IntervalAnchor::Previous(
                    cursor_after
                        .as_ref()
                        .ok_or_else(|| corrupt("prior fence missing"))?
                        .as_slice(),
                )
            } else if after.is_none() {
                IntervalAnchor::First
            } else {
                IntervalAnchor::Random
            };
            checked_packed_interval(maps, root, fence, *value, &checked, &anchor, budget)?;
            for slot in 0..terms {
                let key = checked.key(slot)?;
                if after.is_some_and(|prior| key <= prior) {
                    continue;
                }
                if entries.len() == capacity {
                    more = true;
                    break;
                }
                let bytes =
                    u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
                budget.charge(WorkUsage {
                    bytes,
                    ..WorkUsage::default()
                })?;
                let memory = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
                let mut owned = Vec::new();
                owned
                    .try_reserve_exact(key.len())
                    .map_err(|_error| IndexError::ResourceLimited)?;
                owned.extend_from_slice(key);
                entries.push((owned, MapValue::Object(reference)));
                leases.extend(memory);
                if entries.len() == capacity
                    && (slot.checked_add(1).ok_or(IndexError::ResourceLimited)? < terms
                        || page.has_more)
                {
                    more = true;
                    break;
                }
            }
        } else {
            leaf(objects, reference, fence, budget)?;
            if after.is_none_or(|prior| fence.as_slice() > prior) {
                if entries.len() == capacity {
                    more = true;
                } else {
                    let bytes =
                        u64::try_from(fence.len()).map_err(|_error| IndexError::ResourceLimited)?;
                    budget.charge(WorkUsage {
                        bytes,
                        ..WorkUsage::default()
                    })?;
                    let memory = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
                    let mut owned = Vec::new();
                    owned
                        .try_reserve_exact(fence.len())
                        .map_err(|_error| IndexError::ResourceLimited)?;
                    owned.extend_from_slice(fence);
                    entries.push((owned, MapValue::Object(reference)));
                    leases.extend(memory);
                    if entries.len() == capacity && page.has_more {
                        more = true;
                    }
                }
            }
        }
        if more || !page.has_more || entries.len() == capacity {
            break;
        }
        promised = true;
        cursor_after = Some(PreparedKey::copy(fence, budget)?);
        crossed_descriptor = true;
    }
    Ok(MapPage::new(entries, more, leases, allocation))
}

/// Retains one authenticated packed block across bounded `MapEntries` pages.
///
/// Every continuation crosses the original retained-read and cancellation path.
pub(in crate::native::text) struct Cursor<'a> {
    objects: &'a dyn ImmutableObjectPort,
    maps: &'a dyn PersistentMapPort,
    root: ObjectRef,
    descriptor: Option<quanta_index_core::index::MapPageEntry>,
    checked: Option<CheckedBlock>,
    leaf_count: Option<u64>,
    term_slot: usize,
    terms: usize,
    kind: u8,
    reference: Option<ObjectRef>,
    future_descriptors: bool,
    finished: bool,
    prefetched: Option<PreparedRows<MapPageEntry>>,
    prefetched_slot: usize,
}

/// Keys and counts from one authenticated directory descriptor.
pub(super) struct DescriptorRows<'a> {
    pub(super) fence: &'a [u8],
    pub(super) kind: u8,
    pub(super) terms: usize,
    pub(super) reference: ObjectRef,
    checked: Option<&'a CheckedBlock>,
    leaf_count: Option<u64>,
}

impl DescriptorRows<'_> {
    pub(super) fn key(&self, slot: usize) -> Result<&[u8], IndexError> {
        if let Some(checked) = self.checked {
            return checked.key(slot);
        }
        if slot != 0 {
            return Err(corrupt("leaf term slot exceeds descriptor"));
        }
        Ok(self.fence)
    }

    pub(super) fn position_count(
        &self,
        slot: usize,
        key: &[u8],
        budget: &WorkBudget,
    ) -> Result<u64, IndexError> {
        budget.checkpoint()?;
        if let Some(checked) = self.checked {
            return checked
                .position_count_if_at(slot, key, budget)?
                .ok_or_else(|| corrupt("position count key differs from descriptor"));
        }
        if slot != 0 || self.fence != key {
            return Err(corrupt("leaf position count key differs from descriptor"));
        }
        self.leaf_count
            .ok_or_else(|| corrupt("leaf position count absent"))
    }
}

/// Use the storage-owned continuation when available. A cold visitor may only
/// stage disposable effects, and never retries an already visited descriptor.
pub(super) fn visit_descriptors(
    objects: &dyn ImmutableObjectPort,
    maps: &dyn PersistentMapPort,
    root: ObjectRef,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(DescriptorRows<'_>, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let mut cursor = Cursor::open(objects, maps, root);
    let mut started = false;
    let mut finished = false;
    let usage = budget.usage();
    let supported =
        maps.try_visit_scan_pages(MapRoot(Some(root)), budget, &mut |page, budget| {
            if finished {
                return Err(IndexError::BackendViolation(
                    "inventory directory visit continued past EOF".into(),
                ));
            }
            started = true;
            let has_more = page.has_more;
            if cursor.advance_with(budget, |_, _| Ok(page))? {
                visit(cursor.descriptor_rows()?, budget)?;
                // Keep the fence but release the decoded child before Storage
                // allocates or reads the following map page.
                drop(cursor.checked.take());
            }
            finished = !has_more;
            Ok(())
        })?;
    if supported {
        if !finished {
            return Err(IndexError::BackendViolation(
                "inventory directory visit omitted EOF".into(),
            ));
        }
        return Ok(());
    }
    if started || budget.usage() != usage {
        return Err(IndexError::BackendViolation(
            "inventory directory visit declined after starting".into(),
        ));
    }
    while let Some(rows) = cursor.next_descriptor(budget)? {
        visit(rows, budget)?;
    }
    Ok(())
}

impl<'a> Cursor<'a> {
    pub(in crate::native::text) fn open(
        objects: &'a dyn ImmutableObjectPort,
        maps: &'a dyn PersistentMapPort,
        root: ObjectRef,
    ) -> Self {
        Self {
            objects,
            maps,
            root,
            descriptor: None,
            checked: None,
            leaf_count: None,
            term_slot: 0,
            terms: 0,
            kind: 0,
            reference: None,
            future_descriptors: false,
            finished: false,
            prefetched: None,
            prefetched_slot: 0,
        }
    }

    /// Retain only bounded, authenticated directory descriptor pages. Child
    /// DATA is opened later by `visit_next` and never retained by this pass.
    pub(in crate::native::text) fn prefetch_descriptors(
        &mut self,
        bound: usize,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if bound == 0 || self.prefetched.is_some() || self.descriptor.is_some() || self.finished {
            return Err(IndexError::InvalidInput(
                "inventory descriptor prefetch state or bound invalid".into(),
            ));
        }
        let mut entries = PreparedRows::with_capacity(bound, budget)?;
        let mut started = false;
        let mut finished = false;
        let mut promised = false;
        let usage = budget.usage();
        let root = MapRoot(Some(self.root));
        let supported = self
            .maps
            .try_visit_scan_pages(root, budget, &mut |page, budget| {
                if finished {
                    return Err(IndexError::BackendViolation(
                        "inventory directory prefetch continued past EOF".into(),
                    ));
                }
                started = true;
                let more = page.has_more;
                Self::stage_descriptor_page(page, &mut entries, bound, promised, budget)?;
                promised = more;
                finished = !more;
                Ok(())
            })?;
        if supported {
            if !finished {
                return Err(IndexError::BackendViolation(
                    "inventory directory prefetch omitted EOF".into(),
                ));
            }
        } else {
            if started || budget.usage() != usage {
                return Err(IndexError::BackendViolation(
                    "inventory directory prefetch declined after starting".into(),
                ));
            }
            loop {
                let after = entries
                    .as_slice()
                    .last()
                    .map(MapPageEntry::key)
                    .transpose()?;
                let page = self.maps.scan(root, after, 1, budget)?;
                let more = page.has_more;
                Self::stage_descriptor_page(page, &mut entries, bound, promised, budget)?;
                if !more {
                    break;
                }
                promised = true;
            }
        }
        self.prefetched = Some(entries);
        self.prefetched_slot = 0;
        Ok(())
    }

    fn stage_descriptor_page(
        page: MapPage,
        entries: &mut PreparedRows<MapPageEntry>,
        bound: usize,
        promised: bool,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        let after = entries
            .as_slice()
            .last()
            .map(MapPageEntry::key)
            .transpose()?;
        let page = ordered_descriptor_page(page, after, promised)?;
        let Some(entry) = page.cursor().next() else {
            return Ok(());
        };
        if entries.len() >= bound {
            return Err(corrupt("inventory descriptor prefetch exceeds bound"));
        }
        entries.push(entry, budget)
    }

    fn advance(&mut self, budget: &mut WorkBudget) -> Result<bool, IndexError> {
        let maps = self.maps;
        let root = MapRoot(Some(self.root));
        self.advance_with(budget, |after, budget| maps.scan(root, after, 1, budget))
    }

    fn advance_with(
        &mut self,
        budget: &mut WorkBudget,
        read_page: impl FnOnce(Option<&[u8]>, &mut WorkBudget) -> Result<MapPage, IndexError>,
    ) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        // The previous descriptor already owns its retained map page and key.
        // Keep it alive until the next descriptor and child are authenticated.
        let after = self.descriptor.as_ref().map(|row| row.key()).transpose()?;
        if after.is_some_and(<[u8]>::is_empty) {
            return Err(IndexError::CorruptObject(
                "empty native preparation key".into(),
            ));
        }
        let page = ordered_descriptor_page(
            read_page(after, budget)?,
            after,
            after.is_some() && self.future_descriptors,
        )?;
        let future_descriptors = page.has_more;
        let descriptor = page.cursor().next();
        let Some(row) = descriptor.as_ref() else {
            return Ok(false);
        };
        self.accept_descriptor(row.clone(), future_descriptors, budget)
    }

    fn accept_descriptor(
        &mut self,
        descriptor: MapPageEntry,
        future_descriptors: bool,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        let after = self
            .descriptor
            .as_ref()
            .map(MapPageEntry::key)
            .transpose()?;
        let (key, value) = descriptor.get()?;
        let (kind, terms, reference) = decode(*value)?;
        let (checked, leaf_count) = if kind == PACKED {
            let checked = packed(self.objects, reference, terms, key, budget)?;
            let anchor = after.map_or(IntervalAnchor::First, IntervalAnchor::Previous);
            checked_packed_interval(self.maps, self.root, key, *value, &checked, &anchor, budget)?;
            (Some(checked), None)
        } else {
            (
                None,
                Some(leaf_position_count(self.objects, reference, key, budget)?),
            )
        };
        self.future_descriptors = future_descriptors;
        self.descriptor = Some(descriptor);
        self.checked = checked;
        self.leaf_count = leaf_count;
        self.term_slot = 0;
        self.kind = kind;
        self.terms = terms;
        self.reference = Some(reference);
        Ok(true)
    }

    fn advance_prefetched(&mut self, budget: &mut WorkBudget) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        let entries = self
            .prefetched
            .as_ref()
            .ok_or(IndexError::ResourceLimited)?;
        let Some(descriptor) = entries.as_slice().get(self.prefetched_slot).cloned() else {
            return Ok(false);
        };
        let future = self
            .prefetched_slot
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?
            < entries.len();
        let accepted = self.accept_descriptor(descriptor, future, budget)?;
        self.prefetched_slot = self
            .prefetched_slot
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(accepted)
    }

    /// Lend the complete next descriptor to the disposable cold alias visitor.
    fn next_descriptor(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<DescriptorRows<'_>>, IndexError> {
        budget.checkpoint()?;
        if self.term_slot != self.terms {
            return Err(corrupt("cannot borrow a partly consumed descriptor"));
        }
        // The visitor consumed the previous child in full. Only its fence
        // participates in validation of the next descriptor.
        drop(self.checked.take());
        if self.finished
            || (self.descriptor.is_some() && !self.future_descriptors)
            || !self.advance(budget)?
        {
            self.finished = true;
            return Ok(None);
        }
        self.descriptor_rows().map(Some)
    }

    fn descriptor_rows(&mut self) -> Result<DescriptorRows<'_>, IndexError> {
        self.term_slot = self.terms;
        self.rows_view()
    }

    fn rows_view(&self) -> Result<DescriptorRows<'_>, IndexError> {
        let fence = self
            .descriptor
            .as_ref()
            .ok_or_else(|| corrupt("descriptor absent"))?
            .key()?;
        let reference = self
            .reference
            .ok_or_else(|| corrupt("directory reference missing"))?;
        Ok(DescriptorRows {
            fence,
            kind: self.kind,
            terms: self.terms,
            reference,
            checked: self.checked.as_ref(),
            leaf_count: self.leaf_count,
        })
    }

    /// Lend one verified row while retaining only its current descriptor.
    /// The descriptor callback runs exactly once before any of its rows.
    pub(in crate::native::text) fn visit_next<T>(
        &mut self,
        budget: &mut WorkBudget,
        mut descriptor: impl FnMut(
            &[u8],
            u8,
            usize,
            ObjectRef,
            &mut WorkBudget,
        ) -> Result<(), IndexError>,
        row: impl FnOnce(&[u8], ObjectRef, u64, &mut WorkBudget) -> Result<T, IndexError>,
    ) -> Result<Option<T>, IndexError> {
        budget.checkpoint()?;
        if self.finished {
            return Ok(None);
        }
        if self.descriptor.is_none() || self.term_slot == self.terms {
            drop(self.checked.take());
            if self.descriptor.is_some() && !self.future_descriptors {
                self.finished = true;
                return Ok(None);
            }
            if !(if self.prefetched.is_some() {
                self.advance_prefetched(budget)?
            } else {
                self.advance(budget)?
            }) {
                self.finished = true;
                return Ok(None);
            }
            let rows = self.rows_view()?;
            descriptor(rows.fence, rows.kind, rows.terms, rows.reference, budget)?;
        }
        let rows = self.rows_view()?;
        let key = rows.key(self.term_slot)?;
        let count = rows.position_count(self.term_slot, key, budget)?;
        let value = row(key, rows.reference, count, budget)?;
        self.term_slot = self
            .term_slot
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(Some(value))
    }

    /// Borrow the last row visited from the authenticated descriptor or
    /// packed block. The next visit or EOF invalidates this view.
    pub(in crate::native::text) fn current(
        &self,
    ) -> Result<Option<(&[u8], ObjectRef)>, IndexError> {
        if self.finished || self.term_slot == 0 || self.descriptor.is_none() {
            return Ok(None);
        }
        let slot = self
            .term_slot
            .checked_sub(1)
            .ok_or(IndexError::ResourceLimited)?;
        let key = if let Some(checked) = self.checked.as_ref() {
            checked.key(slot)?
        } else {
            if slot != 0 {
                return Err(corrupt("visited leaf term slot differs"));
            }
            self.descriptor
                .as_ref()
                .ok_or_else(|| corrupt("visited descriptor absent"))?
                .key()?
        };
        let reference = self
            .reference
            .ok_or_else(|| corrupt("visited directory child absent"))?;
        Ok(Some((key, reference)))
    }

    pub(in crate::native::text) fn next_page(
        &mut self,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        budget.checkpoint()?;
        if limit == 0
            || usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?
                > MAX_BLOCK_TERMS
        {
            return Err(IndexError::InvalidInput(
                "inventory directory cursor page exceeds bound".into(),
            ));
        }
        let mut newly_loaded = false;
        let mut finished = self.finished;
        if !self.finished && (self.descriptor.is_none() || self.term_slot == self.terms) {
            if self.descriptor.is_some() && !self.future_descriptors {
                finished = true;
            } else {
                newly_loaded = self.advance(budget)?;
                if !newly_loaded {
                    finished = true;
                }
            }
        }
        // One page ends at one occurrence block or leaf. Reserve only the
        // rows this descriptor can return, even when the caller asks for 512.
        let remaining = self
            .terms
            .checked_sub(self.term_slot)
            .ok_or_else(|| corrupt("directory term slot exceeds descriptor"))?;
        let capacity = if finished {
            0
        } else {
            usize::try_from(limit)
                .map_err(|_error| IndexError::ResourceLimited)?
                .min(remaining)
        };
        let allocation = budget.reserve_memory(
            MapPage::allocation_bytes(capacity)?,
            MemoryLifetime::Retained,
        )?;
        let mut entries = Vec::new();
        let mut leases: Vec<MemoryLease> = Vec::new();
        entries
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        leases
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if !newly_loaded
            && capacity != 0
            && let Some(checked) = self.checked.as_ref()
        {
            let retained = self.objects.read_retained(checked.object(), budget)?;
            if retained != *checked.object() {
                return Err(corrupt("retained block differs from authenticated data"));
            }
        }
        let mut term_slot = self.term_slot;
        while entries.len() < capacity {
            budget.checkpoint()?;
            let reference = self
                .reference
                .ok_or_else(|| corrupt("directory reference missing"))?;
            let key = if self.kind == PACKED {
                self.checked
                    .as_ref()
                    .ok_or_else(|| corrupt("block not retained"))?
                    .key(term_slot)?
            } else {
                self.descriptor
                    .as_ref()
                    .ok_or_else(|| corrupt("leaf descriptor absent"))?
                    .key()?
            };
            let bytes = u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                bytes,
                ..WorkUsage::default()
            })?;
            let memory = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
            let mut owned = Vec::new();
            owned
                .try_reserve_exact(key.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            owned.extend_from_slice(key);
            entries.push((owned, MapValue::Object(reference)));
            leases.extend(memory);
            term_slot = term_slot
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        let has_more = !finished && (term_slot < self.terms || self.future_descriptors);
        let page = MapPage::new(entries, has_more, leases, allocation);
        self.term_slot = term_slot;
        if !has_more {
            self.finished = true;
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_wire_and_malformed_shapes_are_fixed() -> Result<(), IndexError> {
        let reference = ObjectRef(Digest([7; 32]));
        for (kind, terms) in [(PACKED, 2), (PACKED, MAX_BLOCK_TERMS), (LEAF, 1)] {
            let value = descriptor(kind, terms, reference)?;
            let encoded = value.encoded()?;
            let wire = encoded.as_slice()?;
            if wire.len() != 38
                || wire[0] != 1
                || wire[1] != 3
                || wire[2] != kind
                || wire[3..5]
                    != u16::try_from(terms)
                        .map_err(|_error| IndexError::ResourceLimited)?
                        .to_be_bytes()
                || wire[5] != 1
                || wire[6..] != [7; 32]
                || MapValue::decode(wire)? != value
                || decode(value)? != (kind, terms, reference)
            {
                return Err(corrupt("descriptor fixed wire differs"));
            }
        }
        for malformed in [
            MapValue::Object(reference),
            MapValue::inline(&[PACKED, 0, 1], Some(reference))?,
            MapValue::inline(&[LEAF, 0, 2], Some(reference))?,
            MapValue::inline(&[PACKED, 0, 2], None)?,
            MapValue::inline(&[PACKED, 0, 2, 0], Some(reference))?,
        ] {
            if decode(malformed).is_ok() {
                return Err(corrupt("malformed descriptor accepted"));
            }
        }
        Ok(())
    }
}
