//! Incremental cold-owner validation for the bounded term-major producer.

#[cfg(test)]
use super::PostingTermKey;
use super::{
    AttemptContext, Contribution, EntryMap, IndexError, ObjectRef, entry_map, entry_patch,
};
use crate::native::text_occurrence::{OccurrenceReader, block::CheckedBlock};
use quanta_index_core::index::{
    AttemptRef, MapPage, MapPageEntry, MemoryCreditLimits, StoredObject, WorkBudget, WorkUsage,
    ports::{ImmutableObjectPort, PersistentMapPort},
};

enum Input<'a> {
    Small {
        map: entry_map::SmallMap,
        slot: usize,
    },
    MapBlock(entry_map::block::BlockCursor<'a>),
    Block {
        reference: ObjectRef,
        checked: Option<CheckedBlock>,
        slot: usize,
    },
    Directory(entry_map::directory::Cursor<'a>),
    Finished,
}

/// Only the current attempt's Draft producer can assert canonical source
/// layout. Ready inventories have no such provenance and keep the old path.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ColdInventoryProvenance {
    PreparedCanonical,
    Unsealed,
}

pub(super) struct ColdInventoryCursor<'a> {
    attempt: AttemptRef,
    contribution: Contribution,
    provenance: ColdInventoryProvenance,
    alias: Option<entry_patch::ColdInventoryAlias>,
    directory_receipt: Option<entry_patch::DirectoryReceipt>,
    inline_anchor: Option<EntryMap>,
    input: Input<'a>,
    // One owner-local checked DATA window. PreparedCanonical Small/MapBlock
    // selectors derive counts from the authenticated block after its first
    // read; unsealed and prior routes retain their original port behavior.
    reader: OccurrenceReader,
    keys: Option<entry_patch::ColdInventoryKeySeal>,
    descriptors: Option<entry_patch::ColdDescriptorSeal>,
    terms: u64,
    anchors: u64,
    tokens: u64,
    active: bool,
    failed: bool,
}

fn corrupt(message: &'static str) -> IndexError {
    IndexError::CorruptObject(message.into())
}

impl<'a> ColdInventoryCursor<'a> {
    pub(super) fn open(
        next: &Contribution,
        alias: Option<&(entry_patch::ColdInventoryAlias, Option<EntryMap>)>,
        directory_receipt: Option<&entry_patch::DirectoryReceipt>,
        provenance: ColdInventoryProvenance,
        objects: &'a dyn ImmutableObjectPort,
        maps: &'a dyn PersistentMapPort,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        next.inventory
            .validate_for(entry_map::EntryMapUse::ColdInventory)?;
        let sealed = alias.map(|(alias, _)| *alias);
        if sealed.is_some_and(|alias| alias.attempt != attempt) {
            return Err(IndexError::FenceLost);
        }
        if let Some(receipt) = directory_receipt {
            if receipt.attempt != attempt {
                return Err(IndexError::FenceLost);
            }
            if provenance != ColdInventoryProvenance::PreparedCanonical
                || alias.is_some()
                || next.inventory != EntryMap::InventoryDirectory(receipt.root)
                || receipt.descriptors == 0
            {
                return Err(corrupt("cold directory receipt differs from prepared root"));
            }
        }
        let (input, inline_anchor) = match (next.inventory, alias) {
            (EntryMap::Small(map), None)
                if provenance == ColdInventoryProvenance::PreparedCanonical =>
            {
                (Input::Small { map, slot: 0 }, None)
            }
            (EntryMap::Block(reference), None)
                if provenance == ColdInventoryProvenance::PreparedCanonical =>
            {
                (
                    Input::MapBlock(entry_map::block::BlockCursor::open(
                        objects, reference, budget,
                    )?),
                    None,
                )
            }
            (EntryMap::OccurrenceBlock(reference), Some((alias, Some(anchor)))) => {
                if alias.layout != entry_patch::ColdAliasLayout::Inline
                    || alias.reference != reference
                    || alias.anchors != 1
                    || alias.descriptor_digest != entry_patch::ColdDescriptorSeal::new().finish()
                {
                    return Err(corrupt("cold inline cursor envelope differs"));
                }
                (
                    Input::Block {
                        reference,
                        checked: None,
                        slot: 0,
                    },
                    Some(*anchor),
                )
            }
            (EntryMap::OccurrenceBlock(reference), None) => (
                Input::Block {
                    reference,
                    checked: None,
                    slot: 0,
                },
                None,
            ),
            (EntryMap::InventoryDirectory(reference), alias) => {
                if alias.is_none() && provenance != ColdInventoryProvenance::PreparedCanonical {
                    return Err(corrupt("unsealed cold directory cursor"));
                }
                if let Some((alias, anchor)) = alias
                    && (alias.layout != entry_patch::ColdAliasLayout::Directory
                        || anchor.is_some()
                        || alias.anchors == 0
                        || alias.terms < entry_patch::COLD_ALIAS_MIN_TERMS
                        || (alias.anchors == 1 && alias.terms > entry_patch::COLD_ALIAS_MAX_TERMS)
                        || (alias.anchors > 1 && alias.terms <= entry_patch::COLD_ALIAS_MAX_TERMS))
                {
                    return Err(corrupt("cold directory cursor envelope differs"));
                }
                let bound = if let Some(alias) = sealed {
                    alias.anchors
                } else if let Some(receipt) = directory_receipt {
                    receipt.descriptors
                } else {
                    next.indexed_tokens
                        .checked_mul(2)
                        .ok_or(IndexError::ResourceLimited)?
                };
                let mut cursor = entry_map::directory::Cursor::open(objects, maps, reference);
                cursor.prefetch_descriptors(
                    usize::try_from(bound).map_err(|_error| IndexError::ResourceLimited)?,
                    budget,
                )?;
                (Input::Directory(cursor), None)
            }
            _ => return Err(corrupt("cold cursor layout differs from alias")),
        };
        Ok(Self {
            attempt,
            contribution: *next,
            provenance,
            alias: sealed,
            directory_receipt: directory_receipt.copied(),
            inline_anchor,
            input,
            reader: OccurrenceReader::new(2)?,
            keys: sealed.map(|_| entry_patch::ColdInventoryKeySeal::new()),
            descriptors: sealed.map(|_| entry_patch::ColdDescriptorSeal::new()),
            terms: 0,
            anchors: 0,
            tokens: 0,
            active: false,
            failed: false,
        })
    }

    /// Only a current-attempt canonical producer bounds JSON overflow leaves.
    /// Unsealed legacy layouts keep their original streaming path; ordinary
    /// packed occurrence blocks have an independently bounded binary format.
    pub(super) fn credit_limits(
        next: &Contribution,
        alias: Option<&entry_patch::ColdInventoryAlias>,
        directory_receipt: Option<&entry_patch::DirectoryReceipt>,
        provenance: ColdInventoryProvenance,
        maps: &dyn PersistentMapPort,
    ) -> Result<Option<MemoryCreditLimits>, IndexError> {
        if let Some(receipt) = directory_receipt
            && (provenance != ColdInventoryProvenance::PreparedCanonical
                || alias.is_some()
                || next.inventory != EntryMap::InventoryDirectory(receipt.root)
                || receipt.descriptors == 0)
        {
            return Err(corrupt("cold directory receipt differs from prepared root"));
        }
        let directory = match (next.inventory, alias) {
            (EntryMap::OccurrenceBlock(_), _) => None,
            (EntryMap::InventoryDirectory(_), Some(alias)) => Some(alias.anchors),
            (EntryMap::InventoryDirectory(root), None)
                if provenance == ColdInventoryProvenance::PreparedCanonical =>
            {
                let Some(receipt) = directory_receipt else {
                    return Ok(None);
                };
                if root != receipt.root || receipt.descriptors == 0 {
                    return Err(corrupt("cold directory receipt root or count differs"));
                }
                Some(receipt.descriptors)
            }
            (EntryMap::Small(_) | EntryMap::Block(_), None)
                if provenance == ColdInventoryProvenance::PreparedCanonical =>
            {
                None
            }
            _ => return Ok(None),
        };
        let packed_bytes = u64::try_from(crate::native::text_occurrence::block::MAX_BLOCK_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let payload = if directory.is_some()
            || matches!(next.inventory, EntryMap::Small(_) | EntryMap::Block(_))
        {
            // Canonical serde JSON: each u32 needs at most ten decimal digits
            // and one separator, plus the fixed object/array field envelope.
            next.indexed_tokens
                .min(super::MAX_POSITIONS_PER_TERM)
                .checked_mul(11)
                .and_then(|n| n.checked_add(16))
                .ok_or(IndexError::ResourceLimited)?
                .max(packed_bytes)
        } else {
            packed_bytes
        };
        let occurrence_object = StoredObject::allocation_bytes(
            usize::try_from(payload).map_err(|_error| IndexError::ResourceLimited)?,
            0,
        )?;
        let map_object = if matches!(next.inventory, EntryMap::Block(_)) {
            StoredObject::allocation_bytes(
                entry_map::block::MAX_PAYLOAD_BYTES,
                entry_map::block::MAX_ROWS,
            )?
        } else {
            0
        };
        let object = occurrence_object.max(map_object);
        let layout = u64::try_from(std::mem::size_of::<CheckedBlock>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let page = MapPage::allocation_bytes(1)?;
        let page_memory = if directory.is_some() {
            page.checked_mul(2).ok_or(IndexError::ResourceLimited)?
        } else {
            0
        };
        // Object wire and checked layout/preflight may coexist. Only a
        // directory retains map pages; a block borrows its checked key.
        let temporary = payload
            .checked_mul(8)
            .and_then(|n| n.checked_add(layout.checked_mul(2)?))
            .and_then(|n| n.checked_add(page_memory))
            .and_then(|n| n.checked_add(super::MAX_TERM_KEY_BYTES.checked_mul(8)?))
            .ok_or(IndexError::ResourceLimited)?;
        let retained = object
            .checked_mul(3)
            .and_then(|n| n.checked_add(page_memory))
            .and_then(|n| n.checked_add(super::MAX_TERM_KEY_BYTES.checked_mul(4)?))
            .ok_or(IndexError::ResourceLimited)?;
        let mut limits = MemoryCreditLimits {
            temporary_data_bytes: temporary,
            retained_data_bytes: retained,
            max_live_temporary_leases: 32,
            max_live_retained_leases: 32,
        };
        if let Some(anchors) = directory {
            let anchors = usize::try_from(anchors).map_err(|_error| IndexError::ResourceLimited)?;
            if anchors == 0 {
                return Err(corrupt("cold directory has no declared anchors"));
            }
            let Some(backend) = maps.cold_scan_memory_credit_limits(anchors)? else {
                return Ok(None);
            };
            let descriptor_backing = anchors
                .checked_mul(std::mem::size_of::<MapPageEntry>())
                .ok_or(IndexError::ResourceLimited)?;
            let anchors_u64 =
                u64::try_from(anchors).map_err(|_error| IndexError::ResourceLimited)?;
            let retained_pages = page
                .checked_add(super::MAX_TERM_KEY_BYTES)
                .and_then(|n| n.checked_mul(anchors_u64))
                .ok_or(IndexError::ResourceLimited)?;
            let anchors_u32 =
                u32::try_from(anchors).map_err(|_error| IndexError::ResourceLimited)?;
            let retained_leases = anchors_u32
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?;
            limits = limits
                .checked_add(MemoryCreditLimits {
                    temporary_data_bytes: u64::try_from(descriptor_backing)
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    retained_data_bytes: retained_pages,
                    max_live_temporary_leases: 1,
                    max_live_retained_leases: retained_leases,
                })?
                .checked_add(backend)?;
        }
        Ok(Some(limits))
    }

    pub(super) fn advance(&mut self, context: &mut AttemptContext<'_>) -> Result<bool, IndexError> {
        if context.attempt != self.attempt {
            self.failed = true;
            return Err(IndexError::FenceLost);
        }
        context.budget.checkpoint()?;
        if self.failed {
            return Err(IndexError::BackendViolation(
                "failed cold cursor reused".into(),
            ));
        }
        if matches!(self.input, Input::Finished) {
            return Ok(false);
        }
        self.active = false;
        let result = self.advance_inner(context);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn advance_inner(&mut self, context: &mut AttemptContext<'_>) -> Result<bool, IndexError> {
        let alias = self.alias;
        let anchor = self.inline_anchor.as_ref();
        let reader = &mut self.reader;
        let row = match &mut self.input {
            Input::Small { map, slot } => {
                if let Some((key, reference)) = map.entry(*slot)? {
                    context.budget.charge(WorkUsage {
                        bytes: u64::try_from(key.len())
                            .map_err(|_error| IndexError::ResourceLimited)?,
                        ..WorkUsage::default()
                    })?;
                    let count = match reader.validated_count(reference, key, context.budget)? {
                        Some(count) => count,
                        None => reader.count(reference, key, context.objects, context.budget)?,
                    };
                    if count.value == 0 {
                        return Err(corrupt("cold inline occurrence is empty"));
                    }
                    *slot = slot.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    Some(count.value)
                } else {
                    None
                }
            }
            Input::MapBlock(cursor) => {
                cursor.visit_next(context.budget, |key, reference, budget| {
                    let count = match reader.validated_count(reference, key, budget)? {
                        Some(count) => count,
                        None => reader.count(reference, key, context.objects, budget)?,
                    };
                    if count.value == 0 {
                        return Err(corrupt("cold block occurrence is empty"));
                    }
                    Ok(count.value)
                })?
            }
            Input::Block {
                reference,
                checked,
                slot,
            } => {
                if checked.is_none() {
                    *checked = Some(if let Some(anchor) = anchor {
                        let alias = alias.ok_or_else(|| corrupt("cold inline alias absent"))?;
                        entry_map::occurrence::open_cold_inline(
                            *reference,
                            alias.terms,
                            anchor,
                            context.objects,
                            context.budget,
                        )?
                    } else {
                        entry_map::occurrence::read(context.objects, *reference, context.budget)?
                    });
                }
                let block = checked.as_ref().ok_or(IndexError::ResourceLimited)?;
                if *slot == block.term_count() {
                    None
                } else {
                    let key = block.key(*slot)?;
                    if anchor.is_some() {
                        context.budget.charge(WorkUsage {
                            bytes: u64::try_from(key.len())
                                .map_err(|_error| IndexError::ResourceLimited)?,
                            ..WorkUsage::default()
                        })?;
                    }
                    // A current-attempt producer has already authenticated
                    // and parsed this exact DATA above. Unsealed Ready input
                    // keeps its independent count-read boundary.
                    if *slot == 0
                        && self.provenance != ColdInventoryProvenance::PreparedCanonical
                        && context
                            .objects
                            .read_retained(block.object(), context.budget)?
                            != *block.object()
                    {
                        return Err(corrupt("retained occurrence count data changed"));
                    }
                    let count = block
                        .position_count_if_at(*slot, key, context.budget)?
                        .ok_or_else(|| corrupt("cold inline count key differs"))?;
                    *slot = slot.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    Some(count)
                }
            }
            Input::Directory(cursor) => {
                let anchors = &mut self.anchors;
                let descriptors = &mut self.descriptors;
                cursor.visit_next(
                    context.budget,
                    |fence, kind, terms, reference, budget| {
                        if let Some(alias) = alias {
                            if *anchors == 0 && reference != alias.reference {
                                return Err(corrupt("cold alias first child differs"));
                            }
                            if *anchors >= alias.anchors {
                                return Err(corrupt("cold alias directory has excess descriptors"));
                            }
                        }
                        *anchors = anchors.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                        if let Some(seal) = descriptors {
                            seal.push(fence, kind, terms, reference, budget)?;
                        }
                        Ok(())
                    },
                    |_key, _reference, count, _budget| Ok(count),
                )?
            }
            Input::Finished => None,
        };
        let Some(count) = row else {
            self.finish()?;
            self.input = Input::Finished;
            return Ok(false);
        };
        self.active = true;
        let (key, _reference) =
            Self::view_input(&self.input)?.ok_or_else(|| corrupt("advanced cold row absent"))?;
        self.terms = self
            .terms
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if let Some(alias) = self.alias
            && self.terms > alias.terms
        {
            return Err(corrupt("cold inventory has excess terms"));
        }
        if let Some(seal) = &mut self.keys {
            seal.push(key, context.budget)?;
        }
        if key.first() == Some(&0) {
            self.tokens = self
                .tokens
                .checked_add(count)
                .ok_or(IndexError::ResourceLimited)?;
        }
        Ok(true)
    }

    fn view_input<'b>(input: &'b Input<'_>) -> Result<Option<(&'b [u8], ObjectRef)>, IndexError> {
        match input {
            Input::Small { map, slot } => {
                let Some(prior) = slot.checked_sub(1) else {
                    return Ok(None);
                };
                map.entry(prior)
            }
            Input::MapBlock(cursor) => cursor.current(),
            Input::Block {
                reference,
                checked,
                slot,
            } => {
                let Some(prior) = slot.checked_sub(1) else {
                    return Ok(None);
                };
                let block = checked
                    .as_ref()
                    .ok_or_else(|| corrupt("visited block absent"))?;
                Ok(Some((block.key(prior)?, *reference)))
            }
            Input::Directory(cursor) => cursor.current(),
            Input::Finished => Ok(None),
        }
    }

    pub(super) fn current(&self) -> Result<Option<(&[u8], ObjectRef)>, IndexError> {
        if self.failed {
            return Err(IndexError::BackendViolation(
                "failed cold cursor reused".into(),
            ));
        }
        if !self.active {
            return Ok(None);
        }
        Self::view_input(&self.input)?
            .map(Some)
            .ok_or_else(|| corrupt("current cold row absent"))
    }

    #[cfg(test)]
    pub(super) fn next(
        &mut self,
        context: &mut AttemptContext<'_>,
    ) -> Result<Option<(PostingTermKey, ObjectRef)>, IndexError> {
        if !self.advance(context)? {
            return Ok(None);
        }
        let (key, reference) = self
            .current()?
            .ok_or_else(|| corrupt("current cold row absent"))?;
        Ok(Some((
            PostingTermKey::copy(key, context.budget)?,
            reference,
        )))
    }

    fn finish(&mut self) -> Result<(), IndexError> {
        if self.tokens != self.contribution.indexed_tokens {
            return Err(corrupt(
                "native text contribution token count differs from inventory",
            ));
        }
        if let Some(alias) = self.alias {
            if self.terms != alias.terms
                || self
                    .keys
                    .take()
                    .map(entry_patch::ColdInventoryKeySeal::finish)
                    != Some(alias.key_digest)
            {
                return Err(corrupt("cold inventory alias key seal differs"));
            }
            if alias.layout == entry_patch::ColdAliasLayout::Directory
                && (self.anchors != alias.anchors
                    || self
                        .descriptors
                        .take()
                        .map(entry_patch::ColdDescriptorSeal::finish)
                        != Some(alias.descriptor_digest))
            {
                return Err(corrupt(
                    "cold alias persisted directory differs from source",
                ));
            }
        }
        if let Some(receipt) = self.directory_receipt
            && self.anchors != receipt.descriptors
        {
            return Err(corrupt(
                "cold directory descriptor count differs from producer",
            ));
        }
        Ok(())
    }
}
