//! Bind compact metadata to the complete original CAS stream before publication.

use super::{
    MapEntries, PreparedRows,
    dense_posting::{wire as dense_wire, write as dense_write},
    entry_map::{BlockDraft, EntryMap, EntryMapUse, INLINE_ENTRIES, SmallMap},
    posting_witness::OccurrenceDigestWitness,
};
use crate::native::text_occurrence::block::{self as occurrence_block, CheckedBlock};
use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MapPageEntry, MapRoot, MapValue, ObjectRef, OrdinalMapEdit,
    StagedObjectBatch, WorkBudget, WorkUsage,
    ports::{ImmutableObjectPort, MapEditCursor, PersistentMapPort},
};
use sha2::{Digest as _, Sha256};

pub(super) type PostingRefResolver<'a> = dyn FnMut(usize, &[u8], OccurrenceDigestWitness, &mut WorkBudget) -> Result<ObjectRef, IndexError>
    + 'a;

pub(super) const COLD_ALIAS_MIN_TERMS: u64 = 9;
pub(super) const COLD_ALIAS_MAX_TERMS: u64 = 512;
pub(super) const COLD_ALIAS_MAX_KEY_BYTES: usize = 32;

/// A producer-side commitment to one complete, ordered cold inventory.
/// This is a candidate only; the verifier authenticates the persisted block
/// and the ordinary entry-map path independently seals its original edit.
pub(super) struct ColdInventoryKeySeal {
    hash: Sha256,
}

impl ColdInventoryKeySeal {
    pub(super) fn new() -> Self {
        let mut hash = Sha256::new();
        hash.update(b"qidx.native.text.cold-inventory-keys.v1\0");
        Self { hash }
    }

    pub(super) fn push(&mut self, key: &[u8], budget: &mut WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if key.len() > COLD_ALIAS_MAX_KEY_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        let len = u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: len.checked_add(8).ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.hash.update(len.to_be_bytes());
        self.hash.update(key);
        Ok(())
    }

    pub(super) fn finish(self) -> Digest {
        Digest(self.hash.finalize().into())
    }
}

/// A packed block is an inventory alias only after persisted readback proves
/// the complete key set and the independently sealed anchor edit agrees.
#[derive(Clone, Copy)]
pub(super) struct ColdInventoryAlias {
    pub(super) reference: ObjectRef,
    pub(super) terms: u64,
    pub(super) key_digest: Digest,
}

impl ColdInventoryAlias {
    pub(super) fn verify(
        self,
        anchor: &EntryMap,
        context: &mut AttemptContext<'_>,
    ) -> Result<EntryMap, IndexError> {
        context.budget.checkpoint()?;
        if !(COLD_ALIAS_MIN_TERMS..=COLD_ALIAS_MAX_TERMS).contains(&self.terms) {
            return Err(IndexError::InvalidInput(
                "cold inventory alias term count is out of bounds".into(),
            ));
        }
        let EntryMap::Small(anchor) = anchor else {
            return Err(IndexError::CorruptObject(
                "cold inventory alias anchor is not inline".into(),
            ));
        };
        anchor.validate()?;
        if anchor.len() != 1 {
            return Err(IndexError::CorruptObject(
                "cold inventory alias anchor has wrong cardinality".into(),
            ));
        }
        let (anchor_key, anchor_ref) = anchor.entry(0)?.ok_or_else(|| {
            IndexError::CorruptObject("cold inventory alias anchor is missing".into())
        })?;
        if anchor_ref != self.reference {
            return Err(IndexError::CorruptObject(
                "cold inventory alias anchor reference differs".into(),
            ));
        }
        let stored = context.objects.get(self.reference, context.budget)?;
        if stored.registration.reference != self.reference {
            return Err(IndexError::CorruptObject(
                "cold inventory alias stored identity differs".into(),
            ));
        }
        let checked = CheckedBlock::parse(stored, context.budget)?;
        if u64::try_from(checked.term_count()).map_err(|_error| IndexError::ResourceLimited)?
            != self.terms
        {
            return Err(IndexError::CorruptObject(
                "cold inventory alias term count differs".into(),
            ));
        }
        if checked.key(0)? != anchor_key {
            return Err(IndexError::CorruptObject(
                "cold inventory alias first key differs".into(),
            ));
        }
        let mut actual = ColdInventoryKeySeal::new();
        for slot in 0..checked.term_count() {
            let key = checked.key(slot)?;
            if key.len() > COLD_ALIAS_MAX_KEY_BYTES {
                return Err(IndexError::CorruptObject(
                    "cold inventory alias key exceeds inline bound".into(),
                ));
            }
            actual.push(key, context.budget)?;
        }
        if actual.finish() != self.key_digest {
            return Err(IndexError::CorruptObject(
                "cold inventory alias key seal differs".into(),
            ));
        }
        context.budget.checkpoint()?;
        Ok(EntryMap::OccurrenceBlock(self.reference))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Stamp {
    count: u64,
    digest: Digest,
}
struct Edits {
    hash: Sha256,
    count: u64,
}
impl Edits {
    fn new() -> Self {
        let mut hash = Sha256::new();
        hash.update(b"qidx.native.text.entry-edits.v1\0");
        Self { hash, count: 0 }
    }
    fn push(&mut self, edit: &OrdinalMapEdit, budget: &mut WorkBudget) -> Result<(), IndexError> {
        let len =
            u64::try_from(edit.edit.key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: len.checked_add(74).ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.hash.update(len.to_be_bytes());
        self.hash.update(&edit.edit.key);
        for value in [edit.edit.expected, edit.edit.next] {
            self.hash.update([u8::from(value.is_some())]);
            if let Some(value) = value {
                self.hash.update(value.object_ref()?.0.0);
            }
        }
        self.count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }
    fn finish(&self) -> Stamp {
        Stamp {
            count: self.count,
            digest: Digest(self.hash.clone().finalize().into()),
        }
    }
}

struct RootInput<'a> {
    cursor: &'a mut dyn MapEditCursor,
    previous: Option<OrdinalMapEdit>,
    digest: Edits,
}
impl RootInput<'_> {
    fn next(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        let Some(edit) = self.cursor.next_edit(budget)? else {
            return Ok(None);
        };
        if edit.root != 0
            || self
                .previous
                .as_ref()
                .is_some_and(|prior| prior.edit.key >= edit.edit.key)
        {
            return Err(IndexError::CorruptObject(
                "entry-map root delta is foreign or unordered".into(),
            ));
        }
        self.digest.push(&edit, budget)?;
        for value in [edit.edit.expected, edit.edit.next].into_iter().flatten() {
            let _reference = value.object_ref()?;
        }
        self.previous = Some(edit.clone());
        Ok(Some(edit))
    }
}

enum Shape {
    Small,
    Block(Option<BlockDraft>),
    OccurrenceBlock(ObjectRef),
    PostingPages,
    Tree,
}

struct Plan {
    shape: Shape,
    small: SmallMap,
    stamp: Option<Stamp>,
    block_slot: Option<usize>,
    dense: Option<dense_write::Draft>,
}

const REPACK_TREE_ROWS: u64 = dense_wire::MAX_ROWS * 3 / 4;

#[derive(Clone, Copy)]
enum PriorValue {
    Reference(ObjectRef),
    Witness(OccurrenceDigestWitness),
}

impl PriorValue {
    fn matches(self, reference: ObjectRef) -> bool {
        match self {
            Self::Reference(prior) => prior == reference,
            Self::Witness(prior) => prior.matches(reference),
        }
    }

    fn witness(self) -> OccurrenceDigestWitness {
        match self {
            Self::Reference(reference) => OccurrenceDigestWitness::from_reference(reference),
            Self::Witness(witness) => witness,
        }
    }
}

fn prior_row(entry: &MapPageEntry, witness_only: bool) -> Result<(&[u8], PriorValue), IndexError> {
    let (key, value) = entry.get()?;
    let value = if witness_only {
        PriorValue::Witness(OccurrenceDigestWitness::from_value(*value)?)
    } else {
        PriorValue::Reference(value.object_ref()?)
    };
    Ok((key.as_slice(), value))
}

fn require_expected(
    expected: Option<PriorValue>,
    supplied: Option<MapValue>,
) -> Result<(), IndexError> {
    let supplied = supplied.map(MapValue::object_ref).transpose()?;
    if supplied.is_some() != expected.is_some()
        || expected
            .zip(supplied)
            .is_some_and(|(prior, actual)| !prior.matches(actual))
    {
        return Err(IndexError::Conflict);
    }
    Ok(())
}

fn tree_edit(
    root: u32,
    key: &[u8],
    expected: Option<ObjectRef>,
    next: Option<ObjectRef>,
    usage: EntryMapUse,
    budget: &WorkBudget,
) -> Result<OrdinalMapEdit, IndexError> {
    if !usage.is_posting() {
        return OrdinalMapEdit::copy_object_key(root, key, expected, next, budget);
    }
    let witness = |reference| OccurrenceDigestWitness::from_reference(reference).into_value();
    OrdinalMapEdit::copy_key(
        root,
        key,
        expected.map(witness).transpose()?,
        next.map(witness).transpose()?,
        budget,
    )
}

fn posting_tree_edit(
    edit: OrdinalMapEdit,
    usage: EntryMapUse,
    budget: &mut WorkBudget,
) -> Result<OrdinalMapEdit, IndexError> {
    if !usage.is_posting() {
        return Ok(edit);
    }
    // The original shared edit remains immutable for ordering and stream
    // binding. This extra physical key copy has its own work and memory cost.
    budget.charge(WorkUsage {
        bytes: u64::try_from(edit.edit.key.len())
            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    tree_edit(
        edit.root,
        &edit.edit.key,
        edit.edit.expected.map(MapValue::object_ref).transpose()?,
        edit.edit.next.map(MapValue::object_ref).transpose()?,
        usage,
        budget,
    )
}

fn cold_occurrence_alias(
    draft: &BlockDraft,
    context: &mut AttemptContext<'_>,
) -> Result<Option<ObjectRef>, IndexError> {
    let Some(reference) = draft.single_child() else {
        return Ok(None);
    };
    // A single child is only a candidate. Authenticate the complete packed
    // occurrence and compare every key before omitting the inventory object.
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "cold inventory candidate identity differs".into(),
        ));
    }
    if !occurrence_block::is_packed(&stored.payload) {
        return Ok(None);
    }
    let checked = CheckedBlock::parse(stored, context.budget)?;
    if draft.matches_occurrence_keys(&checked, context.budget)? {
        Ok(Some(reference))
    } else {
        Ok(None)
    }
}

fn resolve_packed_warm(
    prior: ObjectRef,
    count: u64,
    edits: &mut dyn MapEditCursor,
    context: &mut AttemptContext<'_>,
) -> Result<Option<Plan>, IndexError> {
    let directory = dense_wire::decode_directory(
        context.objects.get(prior, context.budget)?,
        prior,
        context.budget,
    )?;
    let mut input = RootInput {
        cursor: edits,
        previous: None,
        digest: Edits::new(),
    };
    let mut edit = input.next(context.budget)?;
    let mut draft = dense_write::Draft::new_local(directory.clone(), count, context.budget)?;
    for slot in 0..directory.len() {
        context.budget.checkpoint()?;
        let (fence, reference) = directory.descriptor(slot)?;
        let in_range = |edit: &OrdinalMapEdit| {
            slot + 1 == directory.len() || edit.edit.key.as_slice() <= fence.last.as_slice()
        };
        if !edit.as_ref().is_some_and(in_range) {
            if !draft.keep_page(fence, reference, context.budget)? {
                return Ok(None);
            }
            continue;
        }
        let page = dense_wire::decode_page(
            context.objects.get(reference, context.budget)?,
            reference,
            fence,
            context.budget,
        )?;
        let start = draft.row_offset();
        let mut row_slot = 0;
        while row_slot < page.len() || edit.as_ref().is_some_and(in_range) {
            context.budget.checkpoint()?;
            let old = (row_slot < page.len())
                .then(|| page.row(row_slot))
                .transpose()?;
            let change = edit.as_ref().filter(|edit| in_range(edit));
            let order = match (old, change) {
                (Some(row), Some(edit)) => row.owner.as_slice().cmp(edit.edit.key.as_slice()),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => break,
            };
            if order == std::cmp::Ordering::Less {
                let row = old.ok_or_else(|| {
                    IndexError::BackendViolation("packed page lost base row".into())
                })?;
                draft.push(row, context)?;
                row_slot += 1;
                continue;
            }
            let change = change
                .ok_or_else(|| IndexError::BackendViolation("packed page lost edit".into()))?;
            let owner: [u8; 32] =
                change.edit.key.as_slice().try_into().map_err(|_| {
                    IndexError::CorruptObject("packed posting owner key width".into())
                })?;
            let expected = if order == std::cmp::Ordering::Equal {
                row_slot += 1;
                old.map(|row| PriorValue::Witness(OccurrenceDigestWitness::from_bytes(row.witness)))
            } else {
                None
            };
            require_expected(expected, change.edit.expected)?;
            if let Some(next) = change.edit.next.map(MapValue::object_ref).transpose()? {
                draft.push(
                    dense_wire::PostingRow {
                        owner,
                        witness: next.0.0,
                    },
                    context,
                )?;
            }
            edit = input.next(context.budget)?;
        }
        if !draft.finish_range(start, context.budget)? {
            return Ok(None);
        }
    }
    if edit.is_some() {
        return Err(IndexError::CorruptObject(
            "packed posting has trailing edits".into(),
        ));
    }
    draft.finish_local(context.budget)?;
    Ok(Some(Plan {
        shape: Shape::PostingPages,
        small: SmallMap::default(),
        stamp: Some(input.digest.finish()),
        block_slot: None,
        dense: Some(draft),
    }))
}

fn resolve_bounded(
    usage: EntryMapUse,
    slot: usize,
    prior: &EntryMap,
    count: u64,
    edits: &mut dyn MapEditCursor,
    context: &mut AttemptContext<'_>,
    posting_refs: &mut Option<&mut PostingRefResolver<'_>>,
) -> Result<Plan, IndexError> {
    let mut input = RootInput {
        cursor: edits,
        previous: None,
        digest: Edits::new(),
    };
    let mut before = MapEntries::new(context.objects, context.maps, prior);
    let mut old = before.next(context.budget)?;
    let mut delta = input.next(context.budget)?;
    let pack = usage == EntryMapUse::PackedPosting
        && (dense_wire::MIN_ROWS..=dense_wire::MAX_ROWS).contains(&count);
    let mut dense = if pack {
        let prior = match prior {
            EntryMap::PostingPages(reference) => Some(dense_wire::decode_directory(
                context.objects.get(*reference, context.budget)?,
                *reference,
                context.budget,
            )?),
            _ => None,
        };
        Some(dense_write::Draft::new(prior, count, context.budget)?)
    } else {
        None
    };
    let mut small = (!pack).then(SmallMap::default);
    let mut block = if usage.is_inventory()
        && count > u64::try_from(INLINE_ENTRIES).map_err(|_error| IndexError::ResourceLimited)?
    {
        Some(BlockDraft::new(context.budget)?)
    } else {
        None
    };
    let mut actual = 0_u64;
    while old.is_some() || delta.is_some() {
        context.budget.checkpoint()?;
        let old_row = old
            .as_ref()
            .map(|entry| {
                prior_row(
                    entry,
                    usage.is_posting()
                        && matches!(prior, EntryMap::Tree(_) | EntryMap::PostingPages(_)),
                )
            })
            .transpose()?;
        let ordering = match (old_row, delta.as_ref()) {
            (Some((key, _)), Some(edit)) => key.cmp(&edit.edit.key),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => break,
        };
        let (key, value) = match ordering {
            std::cmp::Ordering::Less => {
                let (key, value) = old_row
                    .ok_or_else(|| IndexError::BackendViolation("entry merge lost base".into()))?;
                (key, Some(value))
            }
            std::cmp::Ordering::Equal | std::cmp::Ordering::Greater => {
                let edit = delta
                    .as_ref()
                    .ok_or_else(|| IndexError::BackendViolation("entry merge lost delta".into()))?;
                let expected = if ordering == std::cmp::Ordering::Equal {
                    old_row.map(|(_, value)| value)
                } else {
                    None
                };
                require_expected(expected, edit.edit.expected)?;
                (
                    edit.edit.key.as_slice(),
                    edit.edit
                        .next
                        .map(quanta_index_core::index::MapValue::object_ref)
                        .transpose()?
                        .map(PriorValue::Reference),
                )
            }
        };
        if let Some(value) = value {
            actual = actual.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            if actual > count {
                return Err(IndexError::CorruptObject(
                    "entry-map final cardinality exceeds declared value".into(),
                ));
            }
            if pack {
                let owner = key.try_into().map_err(|_| {
                    IndexError::CorruptObject("packed posting owner key width".into())
                })?;
                dense.as_mut().ok_or(IndexError::ResourceLimited)?.push(
                    dense_wire::PostingRow {
                        owner,
                        witness: value.witness().bytes(),
                    },
                    context,
                )?;
            } else {
                let value = match value {
                    PriorValue::Reference(reference) => reference,
                    PriorValue::Witness(witness) => {
                        let resolve = posting_refs.as_deref_mut().ok_or_else(|| {
                            IndexError::BackendViolation(
                                "posting witness resolver is missing".into(),
                            )
                        })?;
                        let reference = resolve(slot, key, witness, context.budget)?;
                        if !witness.matches(reference) {
                            return Err(IndexError::CorruptObject(
                                "posting witness differs from owner inventory".into(),
                            ));
                        }
                        reference
                    }
                };
                if let Some(map) = small.as_mut()
                    && !map.push(key, value)?
                {
                    // Preserve Small's original funding envelope. A short map
                    // needs a block only when a key actually exceeds Small's shape.
                    if usage.is_inventory() && block.is_none() && key.len() <= 1024 {
                        let mut draft = BlockDraft::new(context.budget)?;
                        for slot in 0..map.len() {
                            let (prior_key, prior_value) = map.entry(slot)?.ok_or_else(|| {
                                IndexError::BackendViolation("bounded entry seed missing".into())
                            })?;
                            if !draft.push(prior_key, prior_value, context.budget)? {
                                return Err(IndexError::BackendViolation(
                                    "Small entries exceed bounded block seed".into(),
                                ));
                            }
                        }
                        block = Some(draft);
                    }
                    small = None;
                }
                if let Some(draft) = block.as_mut()
                    && !draft.push(key, value, context.budget)?
                {
                    block = None;
                }
            }
        }
        if ordering != std::cmp::Ordering::Greater {
            old = before.next(context.budget)?;
        }
        if ordering != std::cmp::Ordering::Less {
            delta = input.next(context.budget)?;
        }
    }
    if actual != count {
        return Err(IndexError::CorruptObject(
            "entry-map final cardinality differs".into(),
        ));
    }
    if let Some(draft) = dense.as_mut() {
        draft.finish(context)?;
    }
    let shape = if dense.is_some() {
        Shape::PostingPages
    } else if small.is_some() {
        Shape::Small
    } else if let Some(block) = block {
        if usage == EntryMapUse::ColdInventory
            && let Some(reference) = cold_occurrence_alias(&block, context)?
        {
            Shape::OccurrenceBlock(reference)
        } else {
            Shape::Block(Some(block))
        }
    } else {
        Shape::Tree
    };
    Ok(Plan {
        shape,
        small: small.unwrap_or_default(),
        stamp: Some(input.digest.finish()),
        block_slot: None,
        dense,
    })
}

/// Revalidate the complete original stream before publishing compact metadata.
///
/// Input is consumed by the map producer or drained here. Per-root digests forbid using a
/// compact result prepared from another/changed/incomplete input stream.
struct MapInput<'a> {
    input: &'a mut dyn MapEditCursor,
    usage: EntryMapUse,
    priors: &'a [EntryMap],
    plans: &'a [Plan],
    objects: &'a dyn ImmutableObjectPort,
    maps: &'a dyn PersistentMapPort,
    root: usize,
    old_slot: usize,
    old_cursor: Option<MapEntries<'a>>,
    old: Option<MapPageEntry>,
    head: Option<OrdinalMapEdit>,
    previous: Option<OrdinalMapEdit>,
    digest: Edits,
    eof: bool,
    finished: bool,
}
impl MapInput<'_> {
    fn fill(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if self.head.is_some() || self.eof {
            return Ok(());
        }
        let Some(edit) = self.input.next_edit(budget)? else {
            self.eof = true;
            return Ok(());
        };
        if usize::try_from(edit.root).map_err(|_error| IndexError::ResourceLimited)?
            >= self.priors.len()
            || self.previous.as_ref().is_some_and(|prior| {
                prior.root > edit.root
                    || (prior.root == edit.root && prior.edit.key >= edit.edit.key)
            })
        {
            return Err(IndexError::CorruptObject(
                "global entry-map edits are foreign or unordered".into(),
            ));
        }
        for value in [edit.edit.expected, edit.edit.next].into_iter().flatten() {
            let _reference = value.object_ref()?;
        }
        self.previous = Some(edit.clone());
        self.head = Some(edit);
        Ok(())
    }
    fn take(&mut self, budget: &mut WorkBudget) -> Result<OrdinalMapEdit, IndexError> {
        let edit = self
            .head
            .take()
            .ok_or_else(|| IndexError::BackendViolation("entry-map head missing".into()))?;
        if self
            .plans
            .get(self.root)
            .is_some_and(|plan| plan.stamp.is_some())
        {
            self.digest.push(&edit, budget)?;
        }
        Ok(edit)
    }
    fn close(&mut self) -> Result<(), IndexError> {
        let plan = self
            .plans
            .get(self.root)
            .ok_or_else(|| IndexError::BackendViolation("entry plan missing".into()))?;
        if plan
            .stamp
            .is_some_and(|expected| expected != self.digest.finish())
        {
            return Err(IndexError::CorruptObject(
                "entry-map original edit stream changed".into(),
            ));
        }
        self.root = self
            .root
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.old_slot = 0;
        self.old_cursor = None;
        self.old = None;
        self.digest = Edits::new();
        Ok(())
    }
}
impl MapEditCursor for MapInput<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        loop {
            budget.checkpoint()?;
            self.fill(budget)?;
            let Some(prior) = self.priors.get(self.root).copied() else {
                if self.head.is_some() || !self.eof {
                    return Err(IndexError::CorruptObject("entry-map trailing edits".into()));
                }
                self.finished = true;
                return Ok(None);
            };
            let root = u32::try_from(self.root).map_err(|_error| IndexError::ResourceLimited)?;
            let has_delta = self.head.as_ref().is_some_and(|edit| edit.root == root);
            if self
                .plans
                .get(self.root)
                .is_some_and(|plan| !matches!(&plan.shape, Shape::Tree))
            {
                if has_delta {
                    let _edit = self.take(budget)?;
                    continue;
                }
                self.close()?;
                continue;
            }
            if matches!(prior, EntryMap::Tree(_)) {
                if has_delta {
                    return posting_tree_edit(self.take(budget)?, self.usage, budget).map(Some);
                }
                self.close()?;
                continue;
            }
            if matches!(
                prior,
                EntryMap::Block(_) | EntryMap::OccurrenceBlock(_) | EntryMap::PostingPages(_)
            ) && self.old_cursor.is_none()
            {
                self.old_cursor = Some(MapEntries::new(self.objects, self.maps, prior));
            }
            if let Some(cursor) = self.old_cursor.as_mut()
                && self.old.is_none()
            {
                self.old = cursor.next(budget)?;
            }
            let small = prior.inline_map();
            let old = if let Some(small) = small.as_ref() {
                small
                    .entry(self.old_slot)?
                    .map(|(key, reference)| (key, PriorValue::Reference(reference)))
            } else {
                self.old
                    .as_ref()
                    .map(|entry| prior_row(entry, matches!(prior, EntryMap::PostingPages(_))))
                    .transpose()?
            };
            let order = match (old, if has_delta { self.head.as_ref() } else { None }) {
                (Some((key, _)), Some(edit)) => key.cmp(&edit.edit.key),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => {
                    self.close()?;
                    continue;
                }
            };
            if order == std::cmp::Ordering::Less {
                let (key, value) = old.ok_or_else(|| {
                    IndexError::BackendViolation("bounded base entry missing".into())
                })?;
                let result = match value {
                    PriorValue::Reference(reference) => {
                        tree_edit(root, key, None, Some(reference), self.usage, budget)?
                    }
                    PriorValue::Witness(witness) => OrdinalMapEdit::copy_key(
                        root,
                        key,
                        None,
                        Some(witness.into_value()?),
                        budget,
                    )?,
                };
                if small.is_some() {
                    self.old_slot = self
                        .old_slot
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                } else {
                    self.old = None;
                }
                return Ok(Some(result));
            }
            let expected = if order == std::cmp::Ordering::Equal {
                old.map(|(_, value)| value)
            } else {
                None
            };
            let edit = self.take(budget)?;
            require_expected(expected, edit.edit.expected)?;
            if order == std::cmp::Ordering::Equal {
                if small.is_some() {
                    self.old_slot = self
                        .old_slot
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                } else {
                    self.old = None;
                }
            }
            if let Some(value) = edit
                .edit
                .next
                .map(quanta_index_core::index::MapValue::object_ref)
                .transpose()?
            {
                let result =
                    tree_edit(root, &edit.edit.key, None, Some(value), self.usage, budget)?;
                return Ok(Some(result));
            }
        }
    }
}

pub(super) fn apply<C: MapEditCursor>(
    usage: EntryMapUse,
    priors: &[EntryMap],
    counts: &[u64],
    input: &mut dyn MapEditCursor,
    mut root_cursor: impl FnMut(usize) -> Result<C, IndexError>,
    context: &mut AttemptContext<'_>,
    mut posting_refs: Option<&mut PostingRefResolver<'_>>,
) -> Result<PreparedRows<EntryMap>, IndexError> {
    let empty_priors = priors.iter().all(|prior| *prior == EntryMap::default());
    let cold_inventory = usage.is_inventory() && empty_priors;
    let usage = if usage == EntryMapUse::ColdInventory && !empty_priors {
        EntryMapUse::Inventory
    } else {
        usage
    };
    let maximum = if cold_inventory {
        quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH
    } else {
        quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
    };
    if priors.is_empty() || priors.len() > maximum || counts.len() != priors.len() {
        return Err(IndexError::InvalidInput(
            "entry-map batch count differs".into(),
        ));
    }
    let mut plans = PreparedRows::new(priors.len())?;
    let mut bases = PreparedRows::new(priors.len())?;
    let mut result = PreparedRows::new(priors.len())?;
    // Admit every retained slot before reading a base or creating any map.
    for prior in priors {
        prior.validate_for(usage)?;
        plans.push(
            Plan {
                shape: Shape::Tree,
                small: SmallMap::default(),
                stamp: None,
                block_slot: None,
                dense: None,
            },
            context.budget,
        )?;
        bases.push(prior.tree_base(), context.budget)?;
        result.push(EntryMap::default(), context.budget)?;
    }
    for (slot, (prior, count)) in priors.iter().zip(counts).enumerate() {
        let bound = match usage {
            EntryMapUse::Inventory | EntryMapUse::ColdInventory => 512,
            EntryMapUse::PackedPosting if matches!(prior, EntryMap::Tree(_)) => REPACK_TREE_ROWS,
            EntryMapUse::PackedPosting => dense_wire::MAX_ROWS,
            EntryMapUse::Posting => {
                u64::try_from(INLINE_ENTRIES).map_err(|_| IndexError::ResourceLimited)?
            }
        };
        if *count <= bound {
            let local = if usage == EntryMapUse::PackedPosting
                && *count >= dense_wire::MIN_ROWS
                && let EntryMap::PostingPages(reference) = prior
            {
                resolve_packed_warm(*reference, *count, &mut root_cursor(slot)?, context)?
            } else {
                None
            };
            let plan = match local {
                Some(plan) => plan,
                None => resolve_bounded(
                    usage,
                    slot,
                    prior,
                    *count,
                    &mut root_cursor(slot)?,
                    context,
                    &mut posting_refs,
                )?,
            };
            if !matches!(&plan.shape, Shape::Tree) {
                *bases
                    .as_mut_slice()
                    .get_mut(slot)
                    .ok_or(IndexError::ResourceLimited)? = MapRoot(None);
            }
            *plans
                .as_mut_slice()
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)? = plan;
        }
    }
    let needs_tree = plans
        .as_slice()
        .iter()
        .any(|plan| matches!(&plan.shape, Shape::Tree));
    let mut checked = MapInput {
        input,
        usage,
        priors,
        plans: plans.as_slice(),
        objects: context.objects,
        maps: context.maps,
        root: 0,
        old_slot: 0,
        old_cursor: None,
        old: None,
        head: None,
        previous: None,
        digest: Edits::new(),
        eof: false,
        finished: false,
    };
    let roots = if needs_tree {
        Some(context.apply_stream(bases.as_slice(), &mut checked)?)
    } else {
        if checked.next_edit(context.budget)?.is_some() {
            return Err(IndexError::BackendViolation(
                "inline batch produced a map edit".into(),
            ));
        }
        None
    };
    if !checked.finished {
        return Err(IndexError::BackendViolation(
            "map consumer did not complete original entry edits".into(),
        ));
    }
    drop(checked);
    let block_count = plans
        .as_slice()
        .iter()
        .filter(|plan| matches!(&plan.shape, Shape::Block(_)))
        .count();
    let mut staged = if block_count == 0 {
        None
    } else if cold_inventory {
        Some(StagedObjectBatch::for_install_packet(
            block_count,
            context.budget,
        )?)
    } else {
        Some(StagedObjectBatch::new(block_count, context.budget)?)
    };
    for plan in plans.as_mut_slice() {
        if let Shape::Block(draft) = &mut plan.shape {
            let batch = staged
                .as_mut()
                .ok_or_else(|| IndexError::BackendViolation("entry block batch missing".into()))?;
            let draft = draft
                .take()
                .ok_or_else(|| IndexError::BackendViolation("entry block draft missing".into()))?;
            plan.block_slot = Some(draft.stage(batch, context.budget)?);
        }
    }
    let mut expected = if block_count == 0 {
        None
    } else {
        Some(PreparedRows::<ObjectRef>::new(block_count)?)
    };
    if let (Some(batch), Some(expected)) = (staged.as_ref(), expected.as_mut()) {
        for input in batch.inputs() {
            let registration =
                context
                    .objects
                    .identify(&input.payload, &input.children, context.budget)?;
            expected.push(registration.reference, context.budget)?;
        }
    }
    let installed = if let Some(batch) = staged.as_ref() {
        let installed = context.put_batch(batch.inputs())?;
        let expected = expected
            .as_ref()
            .ok_or_else(|| IndexError::BackendViolation("entry block identities missing".into()))?;
        if installed.len() != expected.len()
            || installed
                .iter()
                .zip(expected.as_slice())
                .any(|(actual, expected)| actual != expected)
        {
            return Err(IndexError::BackendViolation(
                "entry block identity changed during installation".into(),
            ));
        }
        Some(installed)
    } else {
        None
    };
    for (slot, ((plan, count), output)) in plans
        .as_slice()
        .iter()
        .zip(counts)
        .zip(result.as_mut_slice())
        .enumerate()
    {
        *output = match &plan.shape {
            Shape::Small => EntryMap::inline(&plan.small),
            Shape::Block(_) => {
                let reference = installed
                    .as_ref()
                    .and_then(|refs| plan.block_slot.and_then(|slot| refs.get(slot)))
                    .copied()
                    .ok_or_else(|| {
                        IndexError::BackendViolation("entry block root missing".into())
                    })?;
                EntryMap::Block(reference)
            }
            Shape::OccurrenceBlock(reference) => EntryMap::OccurrenceBlock(*reference),
            Shape::PostingPages => EntryMap::default(),
            Shape::Tree => {
                let root = roots
                    .as_ref()
                    .and_then(|roots| roots.get(slot))
                    .copied()
                    .ok_or_else(|| {
                        IndexError::BackendViolation("entry-map batch lost a root".into())
                    })?;
                if (*count == 0) != root.0.is_none() {
                    return Err(IndexError::CorruptObject(
                        "entry-map cardinality and root disagree".into(),
                    ));
                }
                EntryMap::from(root)
            }
        };
    }
    if plans.as_slice().iter().any(|plan| plan.dense.is_some()) {
        let mut drafts = PreparedRows::new(priors.len())?;
        for plan in plans.as_mut_slice() {
            drafts.push(plan.dense.take(), context.budget)?;
        }
        dense_write::install(drafts.as_mut_slice(), result.as_mut_slice(), context)?;
    }
    Ok(result)
}
