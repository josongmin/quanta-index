//! Bind compact metadata to the complete original CAS stream before publication.

use super::{
    MapEntries, PreparedRows,
    entry_edits::{Edits, Stamp},
    dense_posting::{wire as dense_wire, write as dense_write},
    entry_map::{BlockDraft, EntryMap, EntryMapUse, INLINE_ENTRIES, SmallMap, directory},
    posting_witness::OccurrenceDigestWitness,
};
use crate::native::text_inventory_descriptor::decode;
use crate::native::text_occurrence::block::{self as occurrence_block, CheckedBlock};
use crate::native::text_spool::{
    DensePostingPrepassProof, MAX_TERM_KEY_BYTES, PostingCursor, ReadyInventoryReadLease,
};
use quanta_index_core::index::{
    AttemptContext, AttemptRef, Digest, IndexError, MapPageEntry, MapRoot, MapValue,
    MemoryLifetime, ObjectRef, OrdinalMapEdit, StagedObjectBatch, WorkBudget, WorkUsage,
    ports::{
        ImmutableObjectPort, MapEditCursor, PersistentMapPort, RootPackingMode, RootPackingPlan,
    },
};

pub(super) type PostingRefResolver<'a> = dyn FnMut(usize, &[u8], OccurrenceDigestWitness, &mut WorkBudget) -> Result<ObjectRef, IndexError>
    + 'a;

pub(super) use super::entry_map::occurrence::{
    COLD_ALIAS_MAX_TERMS, COLD_ALIAS_MIN_TERMS, COLD_INLINE_ANCHOR_KEY_BYTES, ColdDescriptorSeal,
    ColdInventoryKeySeal,
};

/// The alias layout is fixed by its source key and controls the persisted
/// root shape; anchor count alone cannot identify a one-block directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ColdAliasLayout {
    Inline,
    Directory,
}

/// A packed block is an inventory alias only after persisted readback proves
/// the complete key set and the independently sealed anchor edit agrees.
#[derive(Clone, Copy)]
pub(super) struct ColdInventoryAlias {
    pub(super) reference: ObjectRef,
    pub(super) terms: u64,
    pub(super) key_digest: Digest,
    pub(super) anchors: u64,
    pub(super) descriptor_digest: Digest,
    pub(super) layout: ColdAliasLayout,
    pub(super) attempt: AttemptRef,
}

/// Exact descriptor count produced with this attempt's directory root.
/// The term-row count in `Plan::stamp` is a different unit.
#[derive(Clone, Copy)]
pub(super) struct DirectoryReceipt {
    pub(super) root: ObjectRef,
    pub(super) descriptors: u64,
    pub(super) attempt: AttemptRef,
    pub(super) slot: u32,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the fixed inline anchor stays in a bounded owner-local stack value; boxing would add uncharged heap backing"
)]
pub(super) enum ColdInventoryDiff {
    Inline {
        alias: ColdInventoryAlias,
        anchor: EntryMap,
    },
    Directory(ColdInventoryAlias),
}

impl ColdInventoryAlias {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the fixed Copy root is forwarded to the validated reader and returned unchanged"
    )]
    pub(super) fn visit_inline(
        self,
        root: EntryMap,
        anchor: &EntryMap,
        objects: &dyn ImmutableObjectPort,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
        visit: impl FnMut(&[u8], ObjectRef, u64, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(EntryMap, CheckedBlock), IndexError> {
        if self.layout != ColdAliasLayout::Inline || self.anchors != 1 || self.attempt != attempt {
            return Err(IndexError::FenceLost);
        }
        if self.descriptor_digest != ColdDescriptorSeal::new().finish() {
            return Err(IndexError::CorruptObject(
                "cold inline inventory descriptor seal differs".into(),
            ));
        }
        super::entry_map::occurrence::visit_cold_inline(
            root,
            self.reference,
            self.terms,
            self.key_digest,
            anchor,
            objects,
            budget,
            visit,
        )
    }

    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the fixed Copy root is forwarded to the validated reader and returned unchanged"
    )]
    pub(super) fn visit_directory(
        self,
        root: EntryMap,
        objects: &dyn ImmutableObjectPort,
        maps: &dyn PersistentMapPort,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
        visit: impl FnMut(&[u8], ObjectRef, u64, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<EntryMap, IndexError> {
        if self.layout != ColdAliasLayout::Directory || self.attempt != attempt {
            return Err(IndexError::FenceLost);
        }
        super::entry_map::occurrence::visit_cold_directory(
            root,
            self.terms,
            self.anchors,
            self.reference,
            self.key_digest,
            self.descriptor_digest,
            objects,
            maps,
            budget,
            visit,
        )
    }

    #[cfg(test)]
    pub(super) fn verify_directory(
        self,
        root: EntryMap,
        objects: &dyn ImmutableObjectPort,
        maps: &dyn PersistentMapPort,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<EntryMap, IndexError> {
        self.visit_directory(root, objects, maps, attempt, budget, |_, _, _, _| Ok(()))
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
        if edit.root() != 0
            || self
                .previous
                .as_ref()
                .is_some_and(|prior| prior.key() >= edit.key())
        {
            return Err(IndexError::CorruptObject(
                "entry-map root delta is foreign or unordered".into(),
            ));
        }
        self.digest.push(&edit, budget)?;
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
    InventoryDirectory(ObjectRef),
}

struct Plan {
    shape: Shape,
    small: SmallMap,
    stamp: Option<Stamp>,
    aliased_directory: bool,
    block_slot: Option<usize>,
    dense: Option<dense_write::Draft>,
}

impl Plan {
    fn directory(reference: ObjectRef, count: u64, digest: Digest, anchored: bool) -> Self {
        Self {
            shape: Shape::InventoryDirectory(reference),
            small: SmallMap::default(),
            stamp: Some(Stamp { count, digest }),
            aliased_directory: anchored,
            block_slot: None,
            dense: None,
        }
    }
}

#[expect(
    clippy::integer_division,
    reason = "The fixed MAX_ROWS is divisible by four; preserve the exact 3/4 repack threshold."
)]
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
        bytes: u64::try_from(edit.key().len())
            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    tree_edit(
        edit.root(),
        edit.key(),
        edit.expected()?.map(MapValue::object_ref).transpose()?,
        edit.next()?.map(MapValue::object_ref).transpose()?,
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
            slot.checked_add(1) == Some(directory.len()) || edit.key() <= fence.last.as_slice()
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
                (Some(row), Some(edit)) => row.owner.as_slice().cmp(edit.key()),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => break,
            };
            if order == std::cmp::Ordering::Less {
                let row = old.ok_or_else(|| {
                    IndexError::BackendViolation("packed page lost base row".into())
                })?;
                draft.push(row, context)?;
                row_slot = row_slot.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                continue;
            }
            let change = change
                .ok_or_else(|| IndexError::BackendViolation("packed page lost edit".into()))?;
            let owner: [u8; 32] = change.key().try_into().map_err(|_width_error| {
                IndexError::CorruptObject("packed posting owner key width".into())
            })?;
            let expected = if order == std::cmp::Ordering::Equal {
                row_slot = row_slot.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                old.map(|row| PriorValue::Witness(OccurrenceDigestWitness::from_bytes(row.witness)))
            } else {
                None
            };
            require_expected(expected, change.expected()?)?;
            if let Some(next) = change.next()?.map(MapValue::object_ref).transpose()? {
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
        aliased_directory: false,
        block_slot: None,
        dense: Some(draft),
    }))
}

struct BoundedMapDraft {
    usage: EntryMapUse,
    small: Option<SmallMap>,
    block: Option<BlockDraft>,
}

impl BoundedMapDraft {
    fn new(usage: EntryMapUse, count: u64, budget: &WorkBudget) -> Result<Self, IndexError> {
        let block = if usage.is_inventory()
            && count
                > u64::try_from(INLINE_ENTRIES).map_err(|_error| IndexError::ResourceLimited)?
        {
            Some(BlockDraft::new(budget)?)
        } else {
            None
        };
        Ok(Self {
            usage,
            small: Some(SmallMap::default()),
            block,
        })
    }

    fn push(
        &mut self,
        key: &[u8],
        value: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if let Some(map) = self.small.as_mut()
            && !map.push(key, value)?
        {
            // Preserve Small's original funding envelope. A short map
            // needs a block only when a key actually exceeds Small's shape.
            if self.usage.is_inventory() && self.block.is_none() && key.len() <= 1024 {
                let mut draft = BlockDraft::new(budget)?;
                for slot in 0..map.len() {
                    let (prior_key, prior_value) = map.entry(slot)?.ok_or_else(|| {
                        IndexError::BackendViolation("bounded entry seed missing".into())
                    })?;
                    if !draft.push(prior_key, prior_value, budget)? {
                        return Err(IndexError::BackendViolation(
                            "Small entries exceed bounded block seed".into(),
                        ));
                    }
                }
                self.block = Some(draft);
            }
            self.small = None;
        }
        if let Some(draft) = self.block.as_mut()
            && !draft.push(key, value, budget)?
        {
            self.block = None;
        }
        Ok(())
    }

    fn finish(self, stamp: Stamp, context: &mut AttemptContext<'_>) -> Result<Plan, IndexError> {
        let shape = if self.small.is_some() {
            Shape::Small
        } else if let Some(block) = self.block {
            if self.usage == EntryMapUse::ColdInventory
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
            small: self.small.unwrap_or_default(),
            stamp: Some(stamp),
            aliased_directory: false,
            block_slot: None,
            dense: None,
        })
    }
}

fn resolve_frozen_bounded_inventory(
    inventory: &ReadyInventoryReadLease<'_>,
    previous_key: &mut [u8],
    slot: usize,
    count: u64,
    context: &mut AttemptContext<'_>,
) -> Result<Plan, IndexError> {
    let mut draft = BoundedMapDraft::new(EntryMapUse::ColdInventory, count, context.budget)?;
    let mut digest = Edits::new();
    let mut actual = 0_u64;
    inventory.visit_root_edits(slot, previous_key, context.budget, |row, budget| {
        let [expected, next] =
            digest.push_object_wires(row.key, [row.expected, row.next], budget)?;
        require_expected(None, expected.map(Into::into))?;
        if let Some(reference) = next {
            actual = actual.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            if actual > count {
                return Err(IndexError::CorruptObject(
                    "entry-map final cardinality exceeds declared value".into(),
                ));
            }
            draft.push(row.key, reference, budget)?;
        }
        Ok(())
    })?;
    if actual != count {
        return Err(IndexError::CorruptObject(
            "entry-map final cardinality differs".into(),
        ));
    }
    let plan = draft.finish(digest.finish(), context)?;
    inventory.check_epoch(context.budget)?;
    Ok(plan)
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
            EntryMap::Small(_)
            | EntryMap::Block(_)
            | EntryMap::Tree(_)
            | EntryMap::OccurrenceBlock(_)
            | EntryMap::InventoryDirectory(_) => None,
        };
        Some(dense_write::Draft::new(prior, count, context.budget)?)
    } else {
        None
    };
    let mut compact = (!pack)
        .then(|| BoundedMapDraft::new(usage, count, context.budget))
        .transpose()?;
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
            (Some((key, _)), Some(edit)) => key.cmp(edit.key()),
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
                require_expected(expected, edit.expected()?)?;
                (
                    edit.key(),
                    edit.next()?
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
            if let Some(draft) = dense.as_mut() {
                let owner = key.try_into().map_err(|_width_error| {
                    IndexError::CorruptObject("packed posting owner key width".into())
                })?;
                draft.push(
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
                compact.as_mut().ok_or(IndexError::ResourceLimited)?.push(
                    key,
                    value,
                    context.budget,
                )?;
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
    let stamp = input.digest.finish();
    if let Some(mut draft) = dense {
        draft.finish(context)?;
        return Ok(Plan {
            shape: Shape::PostingPages,
            small: SmallMap::default(),
            stamp: Some(stamp),
            aliased_directory: false,
            block_slot: None,
            dense: Some(draft),
        });
    }
    compact
        .ok_or(IndexError::ResourceLimited)?
        .finish(stamp, context)
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
        if usize::try_from(edit.root()).map_err(|_error| IndexError::ResourceLimited)?
            >= self.priors.len()
            || self.previous.as_ref().is_some_and(|prior| {
                prior.root() > edit.root()
                    || (prior.root() == edit.root() && prior.key() >= edit.key())
            })
        {
            return Err(IndexError::CorruptObject(
                "global entry-map edits are foreign or unordered".into(),
            ));
        }
        for value in [edit.expected()?, edit.next()?].into_iter().flatten() {
            if self
                .plans
                .get(usize::try_from(edit.root()).map_err(|_error| IndexError::ResourceLimited)?)
                .is_some_and(|plan| plan.aliased_directory)
            {
                let _descriptor = decode(value)?;
            } else {
                let _reference = value.object_ref()?;
            }
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
            if self
                .plans
                .get(self.root)
                .is_some_and(|plan| plan.aliased_directory)
            {
                self.digest.push_descriptor(&edit, budget)?;
            } else {
                self.digest.push(&edit, budget)?;
            }
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

    fn close_compact(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if let Some(directory) = self.old_cursor.as_mut()
            && directory.next(budget)?.is_some()
        {
            return Err(IndexError::CorruptObject(
                "inventory directory has trailing rows".into(),
            ));
        }
        self.close()
    }

    fn verify_frozen_compacts(
        &mut self,
        inventory: &ReadyInventoryReadLease<'_>,
        previous_key: &mut [u8],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        inventory.visit_edits(self.plans.len(), previous_key, budget, |row, budget| {
            let root = usize::try_from(row.root).map_err(|_width| IndexError::ResourceLimited)?;
            while self.root < root {
                self.close_compact(budget)?;
            }
            let plan = self
                .plans
                .get(self.root)
                .ok_or_else(|| IndexError::BackendViolation("entry plan missing".into()))?;
            if plan.aliased_directory {
                return self
                    .digest
                    .push_descriptor_wire(row.key, row.expected, row.next, budget);
            }
            let [expected, next] =
                self.digest
                    .push_object_wires(row.key, [row.expected, row.next], budget)?;
            if let Shape::InventoryDirectory(reference) = plan.shape {
                let directory = self.old_cursor.get_or_insert_with(|| {
                    MapEntries::new(
                        self.objects,
                        self.maps,
                        EntryMap::InventoryDirectory(reference),
                    )
                });
                let actual = directory.next(budget)?.ok_or_else(|| {
                    IndexError::CorruptObject(
                        "inventory directory has fewer rows than original stream".into(),
                    )
                })?;
                let (key, reference) = actual.object_parts()?;
                if key != row.key || expected.is_some() || next != Some(*reference) {
                    return Err(IndexError::CorruptObject(
                        "inventory directory readback differs from original stream".into(),
                    ));
                }
            }
            Ok(())
        })?;
        while self.root < self.plans.len() {
            self.close_compact(budget)?;
        }
        inventory.check_epoch(budget)?;
        self.finished = true;
        Ok(())
    }
}
impl MapEditCursor for MapInput<'_> {
    fn subtree_packing(&self) -> Result<RootPackingPlan, IndexError> {
        let mut packing = RootPackingPlan::default();
        if self.usage == EntryMapUse::ColdInventory {
            for (slot, (prior, plan)) in self.priors.iter().zip(self.plans).enumerate() {
                if *prior == EntryMap::default() && matches!(&plan.shape, Shape::Tree) {
                    packing.select(
                        u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?,
                        RootPackingMode::Nested,
                    )?;
                }
            }
        }
        Ok(packing)
    }

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
            let has_delta = self.head.as_ref().is_some_and(|edit| edit.root() == root);
            if let Some(Shape::InventoryDirectory(reference)) =
                self.plans.get(self.root).map(|plan| &plan.shape)
            {
                if self
                    .plans
                    .get(self.root)
                    .is_some_and(|plan| plan.aliased_directory)
                {
                    if has_delta {
                        let edit = self.take(budget)?;
                        if edit.expected()?.is_some() || edit.next()?.is_none() {
                            return Err(IndexError::CorruptObject(
                                "cold alias anchor CAS differs".into(),
                            ));
                        }
                        continue;
                    }
                    self.close()?;
                    continue;
                }
                if self.old_cursor.is_none() {
                    self.old_cursor = Some(MapEntries::new(
                        self.objects,
                        self.maps,
                        EntryMap::InventoryDirectory(*reference),
                    ));
                }
                if has_delta {
                    let edit = self.take(budget)?;
                    let actual = self
                        .old_cursor
                        .as_mut()
                        .ok_or(IndexError::ResourceLimited)?
                        .next(budget)?
                        .ok_or_else(|| {
                            IndexError::CorruptObject(
                                "inventory directory has fewer rows than original stream".into(),
                            )
                        })?;
                    let (key, reference) = actual.object_parts()?;
                    if key != edit.key()
                        || edit.expected()?.is_some()
                        || edit.next()?.map(MapValue::object_ref).transpose()? != Some(*reference)
                    {
                        return Err(IndexError::CorruptObject(
                            "inventory directory readback differs from original stream".into(),
                        ));
                    }
                    continue;
                }
                self.close_compact(budget)?;
                continue;
            }
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
                EntryMap::Block(_)
                    | EntryMap::OccurrenceBlock(_)
                    | EntryMap::PostingPages(_)
                    | EntryMap::InventoryDirectory(_)
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
                (Some((key, _)), Some(edit)) => key.cmp(edit.key()),
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
            require_expected(expected, edit.expected()?)?;
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
                .next()?
                .map(quanta_index_core::index::MapValue::object_ref)
                .transpose()?
            {
                // For an empty inventory base this is already the original,
                // ordered object-valued edit. Keep its admitted key and grant
                // instead of copying them into a second ordinal edit.
                if self.usage.is_inventory() && prior == EntryMap::default() {
                    return Ok(Some(edit));
                }
                let result = tree_edit(root, edit.key(), None, Some(value), self.usage, budget)?;
                return Ok(Some(result));
            }
        }
    }
}

enum OriginalInput<'a, 'spool> {
    Cursor(&'a mut dyn MapEditCursor),
    Inventory(&'a ReadyInventoryReadLease<'spool>),
}

pub(super) fn apply<C: MapEditCursor>(
    usage: EntryMapUse,
    priors: &[EntryMap],
    counts: &[u64],
    input: &mut dyn MapEditCursor,
    root_cursor: impl FnMut(usize) -> Result<C, IndexError>,
    context: &mut AttemptContext<'_>,
    posting_refs: Option<&mut PostingRefResolver<'_>>,
) -> Result<PreparedRows<EntryMap>, IndexError> {
    apply_inner(
        usage,
        priors,
        counts,
        None,
        None,
        OriginalInput::Cursor(input),
        root_cursor,
        context,
        posting_refs,
    )
}

/// Cold bounded postings are planned from one authenticated original stream.
/// All page outputs are complete before any packed object is installed.
pub(super) fn apply_cold_bounded_postings(
    priors: &[EntryMap],
    counts: &[u64],
    input: &mut PostingCursor<'_>,
    proof: &DensePostingPrepassProof,
    mut confirmed: Option<&mut dense_write::ConfirmedColdRoots<'_>>,
    context: &mut AttemptContext<'_>,
) -> Result<PreparedRows<EntryMap>, IndexError> {
    if priors.is_empty()
        || priors.len() > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
        || counts.len() != priors.len()
        || priors.iter().any(|prior| *prior != EntryMap::default())
        || counts.iter().any(|count| *count > dense_wire::MAX_ROWS)
    {
        return Err(IndexError::InvalidInput(
            "cold bounded posting page differs from its declared roots".into(),
        ));
    }
    input.check_cardinalities(proof, context.budget)?;
    let mut result = PreparedRows::with_capacity(priors.len(), context.budget)?;
    let mut drafts = PreparedRows::with_capacity(priors.len(), context.budget)?;
    let mut next = input.next_edit(context.budget)?;
    for (slot, count) in counts.iter().copied().enumerate() {
        let root = u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?;
        let mut small = SmallMap::default();
        let mut draft = if count >= dense_wire::MIN_ROWS {
            Some(dense_write::Draft::new(None, count, context.budget)?)
        } else {
            None
        };
        while next.as_ref().is_some_and(|edit| edit.root() == root) {
            let edit = next.take().ok_or(IndexError::ResourceLimited)?;
            if edit.key().len() != 32 {
                return Err(IndexError::CorruptObject(
                    "cold bounded posting owner key width".into(),
                ));
            }
            require_expected(None, edit.expected()?)?;
            if let Some(reference) = edit.next()?.map(MapValue::object_ref).transpose()? {
                if let Some(draft) = draft.as_mut() {
                    let owner: [u8; 32] = edit.key().try_into().map_err(|_error| {
                        IndexError::CorruptObject("cold bounded posting owner key width".into())
                    })?;
                    draft.push(
                        dense_wire::PostingRow {
                            owner,
                            witness: OccurrenceDigestWitness::from_reference(reference).bytes(),
                        },
                        context,
                    )?;
                } else if !small.push(edit.key(), reference)? {
                    return Err(IndexError::CorruptObject(
                        "cold small posting exceeds inline shape".into(),
                    ));
                }
            }
            next = input.next_edit(context.budget)?;
        }
        if draft.is_none()
            && u64::try_from(small.len()).map_err(|_error| IndexError::ResourceLimited)? != count
        {
            return Err(IndexError::CorruptObject(
                "cold bounded posting cardinality differs".into(),
            ));
        }
        if let Some(draft) = draft.as_mut() {
            draft.finish(context)?;
        }
        result.push(EntryMap::inline(&small), context.budget)?;
        drafts.push(draft, context.budget)?;
    }
    if next.is_some() {
        return Err(IndexError::CorruptObject(
            "cold bounded posting stream has trailing root".into(),
        ));
    }
    input.check_cardinalities(proof, context.budget)?;
    // Hints are consulted only after the entire original stream authenticated.
    // No leaf from a partially validated page can seed a later page.
    if let Some(confirmed) = confirmed.as_deref_mut() {
        confirmed.reuse(drafts.as_mut_slice(), result.as_mut_slice(), context)?;
    }
    if drafts.as_slice().iter().any(Option::is_some) {
        dense_write::install(drafts.as_mut_slice(), result.as_mut_slice(), context)?;
    }
    if let Some(confirmed) = confirmed {
        confirmed.remember_installed(result.as_slice(), context)?;
    }
    Ok(result)
}

#[cfg(test)]
pub(super) fn apply_with_cold_aliases<C: MapEditCursor>(
    usage: EntryMapUse,
    priors: &[EntryMap],
    counts: &[u64],
    aliases: &[Option<ColdInventoryAlias>],
    input: &mut dyn MapEditCursor,
    root_cursor: impl FnMut(usize) -> Result<C, IndexError>,
    context: &mut AttemptContext<'_>,
    posting_refs: Option<&mut PostingRefResolver<'_>>,
) -> Result<PreparedRows<EntryMap>, IndexError> {
    apply_inner(
        usage,
        priors,
        counts,
        Some(aliases),
        None,
        OriginalInput::Cursor(input),
        root_cursor,
        context,
        posting_refs,
    )
}

pub(super) fn apply_frozen_inventory(
    usage: EntryMapUse,
    priors: &[EntryMap],
    counts: &[u64],
    aliases: &[Option<ColdInventoryAlias>],
    directory_receipts: &mut [Option<DirectoryReceipt>],
    inventory: &ReadyInventoryReadLease<'_>,
    context: &mut AttemptContext<'_>,
) -> Result<PreparedRows<EntryMap>, IndexError> {
    apply_inner(
        usage,
        priors,
        counts,
        Some(aliases),
        Some(directory_receipts),
        OriginalInput::Inventory(inventory),
        |slot| inventory.root_cursor(slot),
        context,
        None,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "private join keeps independent root, receipt, cursor and posting authorities explicit"
)]
fn apply_inner<C: MapEditCursor>(
    usage: EntryMapUse,
    priors: &[EntryMap],
    counts: &[u64],
    aliases: Option<&[Option<ColdInventoryAlias>]>,
    mut directory_receipts: Option<&mut [Option<DirectoryReceipt>]>,
    input: OriginalInput<'_, '_>,
    mut root_cursor: impl FnMut(usize) -> Result<C, IndexError>,
    context: &mut AttemptContext<'_>,
    mut posting_refs: Option<&mut PostingRefResolver<'_>>,
) -> Result<PreparedRows<EntryMap>, IndexError> {
    if aliases.is_some_and(|aliases| aliases.len() != priors.len()) {
        return Err(IndexError::InvalidInput(
            "cold alias plan count differs".into(),
        ));
    }
    if directory_receipts
        .as_ref()
        .is_some_and(|receipts| receipts.len() != priors.len())
    {
        return Err(IndexError::InvalidInput(
            "cold directory receipt count differs".into(),
        ));
    }
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
    let mut directories = PreparedRows::new(priors.len())?;
    let frozen = match &input {
        OriginalInput::Cursor(_) => None,
        OriginalInput::Inventory(inventory) => Some(*inventory),
    };
    let diagnostic = if usage.is_inventory() {
        context.budget.request().diagnostic_context().cloned()
    } else {
        None
    };
    // Admit every retained slot before reading a base or creating any map.
    for prior in priors {
        prior.validate_for(usage)?;
        plans.push(
            Plan {
                shape: Shape::Tree,
                small: SmallMap::default(),
                stamp: None,
                aliased_directory: false,
                block_slot: None,
                dense: None,
            },
            context.budget,
        )?;
        bases.push(prior.tree_base(), context.budget)?;
        result.push(EntryMap::default(), context.budget)?;
    }
    let planning_memory = if usage == EntryMapUse::ColdInventory && frozen.is_some() {
        context
            .budget
            .try_reserve_memory_for_optimization(MAX_TERM_KEY_BYTES, MemoryLifetime::Temporary)?
    } else {
        None
    };
    let mut planning_key = [0; 1024];
    for (slot, (prior, count)) in priors.iter().zip(counts).enumerate() {
        let alias = aliases
            .and_then(|aliases| aliases.get(slot))
            .copied()
            .flatten();
        if let Some(alias) = alias
            && (usage != EntryMapUse::ColdInventory
                || *prior != EntryMap::default()
                || alias.anchors != *count
                || alias.attempt != context.attempt)
        {
            return Err(IndexError::InvalidInput(
                "cold alias plan differs from root".into(),
            ));
        }
        let bound = match usage {
            EntryMapUse::Inventory | EntryMapUse::ColdInventory => 512,
            EntryMapUse::PackedPosting if matches!(prior, EntryMap::Tree(_)) => REPACK_TREE_ROWS,
            EntryMapUse::PackedPosting => dense_wire::MAX_ROWS,
            EntryMapUse::Posting => {
                u64::try_from(INLINE_ENTRIES).map_err(|_error| IndexError::ResourceLimited)?
            }
        };
        let anchored = alias.is_some_and(|alias| alias.layout == ColdAliasLayout::Directory);
        if usage == EntryMapUse::ColdInventory
            && *prior == EntryMap::default()
            && (*count > bound || anchored)
        {
            if frozen.is_some() {
                directories.push(
                    directory::BuildRequest {
                        slot,
                        count: *count,
                        anchored,
                    },
                    context.budget,
                )?;
                continue;
            }
            let (reference, receipt) = super::observe_text_step(
                diagnostic.as_ref(),
                quanta_index_core::DiagnosticStageV1::NativeTextInventoryMapApply,
                40,
                context,
                |context| {
                    if anchored {
                        directory::build_anchors(&mut root_cursor(slot)?, *count, context)
                    } else {
                        directory::build(&mut root_cursor(slot)?, *count, context)
                    }
                },
            )?;
            *plans
                .as_mut_slice()
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)? =
                Plan::directory(reference, *count, receipt.digest, anchored);
            if !anchored && let Some(receipts) = directory_receipts.as_deref_mut() {
                *receipts.get_mut(slot).ok_or(IndexError::ResourceLimited)? =
                    Some(DirectoryReceipt {
                        root: reference,
                        descriptors: receipt.descriptors,
                        attempt: context.attempt,
                        slot: u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?,
                    });
            }
        } else if *count <= bound {
            let local = if let (Some(inventory), Some(_memory)) = (frozen, &planning_memory) {
                Some(resolve_frozen_bounded_inventory(
                    inventory,
                    &mut planning_key,
                    slot,
                    *count,
                    context,
                )?)
            } else if usage == EntryMapUse::PackedPosting
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
    drop(planning_memory);
    if let Some(inventory) = frozen
        && !directories.as_slice().is_empty()
    {
        let requests = directories.as_slice();
        let (roots, produced) = super::observe_text_step(
            diagnostic.as_ref(),
            quanta_index_core::DiagnosticStageV1::NativeTextInventoryMapApply,
            40,
            context,
            |context| directory::build_batch(requests, inventory, context),
        )?;
        for (index, (request, receipt)) in requests.iter().zip(produced.as_slice()).enumerate() {
            let root = roots.get(index).and_then(|root| root.0).ok_or_else(|| {
                IndexError::CorruptObject("directory batch map root absent".into())
            })?;
            *plans
                .as_mut_slice()
                .get_mut(request.slot)
                .ok_or(IndexError::ResourceLimited)? =
                Plan::directory(root, request.count, receipt.digest, request.anchored);
            if !request.anchored
                && let Some(receipts) = directory_receipts.as_deref_mut()
            {
                *receipts
                    .get_mut(request.slot)
                    .ok_or(IndexError::ResourceLimited)? = Some(DirectoryReceipt {
                    root,
                    descriptors: receipt.descriptors,
                    attempt: context.attempt,
                    slot: u32::try_from(request.slot)
                        .map_err(|_error| IndexError::ResourceLimited)?,
                });
            }
        }
    }
    drop(directories);
    let needs_tree = plans
        .as_slice()
        .iter()
        .any(|plan| matches!(&plan.shape, Shape::Tree));
    let mut inventory_cursor;
    let input: &mut dyn MapEditCursor = match input {
        OriginalInput::Cursor(input) => input,
        OriginalInput::Inventory(inventory) => {
            inventory_cursor = inventory.cursor(priors.len())?;
            &mut inventory_cursor
        }
    };
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
    let roots = super::observe_text_step(
        diagnostic.as_ref(),
        quanta_index_core::DiagnosticStageV1::NativeTextInventoryMapApply,
        41,
        context,
        |context| {
            if usage == EntryMapUse::ColdInventory
                && !needs_tree
                && let Some(inventory) = frozen
                && let Some(_key_memory) = context.budget.try_reserve_memory_for_optimization(
                    MAX_TERM_KEY_BYTES,
                    MemoryLifetime::Temporary,
                )?
            {
                let mut previous_key = [0; 1024];
                checked.verify_frozen_compacts(inventory, &mut previous_key, context.budget)?;
                return Ok(None);
            }
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
            Ok(roots)
        },
    )?;
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
            Shape::InventoryDirectory(reference) => EntryMap::InventoryDirectory(*reference),
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
    if let Some(receipts) = directory_receipts.as_deref() {
        for (slot, receipt) in receipts.iter().enumerate() {
            if let Some(receipt) = receipt
                && (receipt.attempt != context.attempt
                    || !usize::try_from(receipt.slot).is_ok_and(|index| index == slot)
                    || result.as_slice().get(slot)
                        != Some(&EntryMap::InventoryDirectory(receipt.root)))
            {
                return Err(IndexError::CorruptObject(
                    "cold directory receipt differs from installed root".into(),
                ));
            }
        }
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
