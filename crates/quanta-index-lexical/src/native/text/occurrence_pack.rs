//! Initial-population owner-local occurrence packing.
//!
//! The existing occurrence batch remains the physical installer and the
//! inventory spool remains the only source of logical map edits. This module
//! changes the number of immutable payloads, not their custody protocol.

use quanta_index_core::index::{
    AttemptContext, IndexError, ObjectRef, WorkBudget, ports::MAX_OBJECT_INSTALL_BATCH,
};

use super::{
    ConfirmedOccurrences, InventoryEdits, InventoryPartition, OccurrenceEdit, OccurrenceWrites,
    PreparedKey, PreparedPositions, TextSpool, check_term_key,
    cold_tokens::ColdTokens,
    entry_patch::{
        COLD_ALIAS_MAX_KEY_BYTES, COLD_ALIAS_MAX_TERMS, COLD_ALIAS_MIN_TERMS, ColdInventoryAlias,
        ColdInventoryKeySeal,
    },
};
use crate::native::{
    text_memory::{PreparedPayload, PreparedRows},
    text_occurrence::block::{
        BlockDraft, BlockPush, BlockTerm, MAX_BLOCK_TERMS, PackedBlockView, SealedBlock,
        encode_packed,
    },
    text_spool::MAX_INVENTORY_ROOTS,
};

struct PackTarget<'a> {
    ordinal: u32,
    spool: &'a TextSpool,
    confirmed: &'a mut ConfirmedOccurrences,
    inventory_edits: &'a mut InventoryEdits,
    writes: &'a mut OccurrenceWrites,
}

enum PackedSelectors {
    Terms {
        terms: PreparedRows<BlockTerm>,
        alias_only: bool,
    },
    AliasFirst {
        first: PreparedKey,
        term_count: usize,
    },
}

impl PackedSelectors {
    fn len(&self) -> usize {
        match self {
            Self::Terms { terms, .. } => terms.len(),
            Self::AliasFirst { term_count, .. } => *term_count,
        }
    }
}

/// Prepare one new owner's sorted token inventory. The caller must establish
/// an empty prior source and empty prior inventory before choosing this path.
pub(super) fn prepare_owner_blocks(
    ordinal: u32,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    cold_tokens: Option<&ColdTokens>,
    confirmed: &mut ConfirmedOccurrences,
    inventory_edits: &mut InventoryEdits,
    writes: &mut OccurrenceWrites,
) -> Result<(u64, Option<ColdInventoryAlias>), IndexError> {
    context.budget.checkpoint()?;
    if inventory_edits.ordinal != ordinal {
        return Err(IndexError::BackendViolation(
            "native packed inventory ordinal changed during preparation".into(),
        ));
    }
    let mut target = PackTarget {
        ordinal,
        spool,
        confirmed,
        inventory_edits,
        writes,
    };
    if let Some(tokens) = cold_tokens
        && let Some(borrowed) = tokens.try_packed_alias(context.budget)?
    {
        let mut seal = ColdInventoryKeySeal::new();
        for ordinal in 0..borrowed.term_count() {
            let (key, _vector) = borrowed.term(ordinal)?;
            seal.push(key, context.budget)?;
        }
        let first = PreparedKey::copy(borrowed.first_key()?, context.budget)?;
        let payload = encode_packed(&borrowed, borrowed.encoded_len(), context.budget)?;
        let term_count = borrowed.term_count();
        let terms = u64::try_from(term_count).map_err(|_error| IndexError::ResourceLimited)?;
        drop(borrowed);
        let reference = target.stage_packed_selected(
            PreparedPayload::from_staged(payload)?,
            PackedSelectors::AliasFirst { first, term_count },
            context,
        )?;
        return Ok((
            terms,
            Some(ColdInventoryAlias {
                reference,
                terms,
                key_digest: seal.finish(),
            }),
        ));
    }
    let mut draft = BlockDraft::new()?;
    let mut term_count = 0_u64;
    // A cold alias is valid only for the entire ordered key stream of one
    // packed block. Once a term cannot fit, no prefix witness is usable.
    let mut key_seal = Some(ColdInventoryKeySeal::new());
    let attempt = context.attempt;
    let objects = context.objects;
    let maps = context.maps;
    let custody = context.custody;
    let scratch = context.scratch;
    let mut consume = |term: PreparedKey, positions: PreparedPositions, budget: &mut WorkBudget| {
        let mut group_context = AttemptContext {
            attempt,
            objects,
            maps,
            custody,
            scratch,
            budget,
        };
        term_count = term_count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        check_term_key(term.as_slice())?;
        if positions.as_slice().is_empty() {
            return Err(IndexError::BackendViolation(
                "native text token term has no positions".into(),
            ));
        }
        if let Some(seal) = key_seal.as_mut() {
            if term.as_slice().len() <= COLD_ALIAS_MAX_KEY_BYTES
                && term_count <= COLD_ALIAS_MAX_TERMS
            {
                seal.push(term.as_slice(), group_context.budget)?;
            } else {
                key_seal = None;
            }
        }
        match draft.try_push(term, positions, group_context.budget)? {
            BlockPush::Added => Ok(()),
            BlockPush::Full { term, positions } => {
                key_seal = None;
                if draft.len() != 0 {
                    let sealed = std::mem::replace(&mut draft, BlockDraft::new()?)
                        .finish(group_context.budget)?;
                    target.stage_sealed(sealed, &mut group_context)?;
                }
                // Full returns the original owned input. A term too large
                // for an empty block follows the canonical leaf path.
                match draft.try_push(term, positions, group_context.budget)? {
                    BlockPush::Added => Ok(()),
                    BlockPush::Full { term, positions } => {
                        target.stage_leaf(term, positions, &mut group_context)
                    }
                }
            }
        }
    };
    let limit = usize::try_from(super::MAX_POSITIONS_PER_TERM)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if let Some(tokens) = cold_tokens {
        tokens.visit_groups(limit, context.budget, &mut consume)?;
    } else {
        spool.visit_token_groups(limit, context.budget, &mut consume)?;
    }
    if draft.len() == 0 {
        return Ok((term_count, None));
    }
    let sealed = draft.finish(context.budget)?;
    match (key_seal, sealed) {
        (Some(seal), SealedBlock::Packed { payload, terms }) => {
            let count = u64::try_from(terms.len()).map_err(|_error| IndexError::ResourceLimited)?;
            if count != term_count {
                return Err(IndexError::BackendViolation(
                    "cold inventory key seal term count differs from packed block".into(),
                ));
            }
            if (COLD_ALIAS_MIN_TERMS..=COLD_ALIAS_MAX_TERMS).contains(&count) {
                let reference = target.stage_packed(
                    PreparedPayload::from_staged(payload)?,
                    terms,
                    context,
                    true,
                )?;
                return Ok((
                    term_count,
                    Some(ColdInventoryAlias {
                        reference,
                        terms: term_count,
                        key_digest: seal.finish(),
                    }),
                ));
            }
            target.stage_sealed(SealedBlock::Packed { payload, terms }, context)?;
        }
        (_, sealed) => target.stage_sealed(sealed, context)?,
    }
    Ok((term_count, None))
}

impl PackTarget<'_> {
    fn stage_sealed(
        &mut self,
        sealed: SealedBlock,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        match sealed {
            SealedBlock::Leaf { term, positions } => self.stage_leaf(term, positions, context),
            SealedBlock::Packed { payload, terms } => self
                .stage_packed(
                    PreparedPayload::from_staged(payload)?,
                    terms,
                    context,
                    false,
                )
                .map(|_reference| ()),
        }
    }

    fn stage_leaf(
        &mut self,
        term: PreparedKey,
        positions: PreparedPositions,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        let payload = crate::native::text_memory::encode(&positions.occurrence, context.budget)?;
        drop(positions);
        let identified = context
            .objects
            .identify(payload.as_slice(), &[], context.budget)?;
        self.stage_identified(term, identified.reference, payload, context)
    }

    fn stage_identified(
        &mut self,
        term: PreparedKey,
        reference: ObjectRef,
        payload: PreparedPayload,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        let pending = self
            .writes
            .pending
            .iter()
            .any(|row| row.identified == reference);
        if pending {
            self.confirmed.check_attempt(context.attempt)?;
        }
        if !pending
            && self.confirmed.contains_or_spill(
                reference,
                context.attempt,
                self.spool,
                context.budget,
            )?
        {
            self.push_ready(term, reference, context.budget)?;
            return Ok(());
        }
        if self
            .writes
            .needs_flush(reference, payload.as_slice().len())?
        {
            self.writes
                .flush(context, self.spool, self.confirmed, self.inventory_edits)?;
        }
        self.writes.push(
            OccurrenceEdit {
                ordinal: self.ordinal,
                key: term,
                prior: None,
                identified: reference,
            },
            payload,
            self.spool,
            self.inventory_edits,
            context.budget,
        )?;
        self.flush_full(context)
    }

    fn stage_packed(
        &mut self,
        payload: PreparedPayload,
        terms: PreparedRows<BlockTerm>,
        context: &mut AttemptContext<'_>,
        alias_only: bool,
    ) -> Result<ObjectRef, IndexError> {
        self.stage_packed_selected(
            payload,
            PackedSelectors::Terms { terms, alias_only },
            context,
        )
    }

    fn stage_packed_selected(
        &mut self,
        payload: PreparedPayload,
        selectors: PackedSelectors,
        context: &mut AttemptContext<'_>,
    ) -> Result<ObjectRef, IndexError> {
        context.budget.checkpoint()?;
        if !(2..=MAX_BLOCK_TERMS).contains(&selectors.len()) {
            return Err(IndexError::BackendViolation(
                "native packed occurrence has invalid term count".into(),
            ));
        }
        let identified = context
            .objects
            .identify(payload.as_slice(), &[], context.budget)?;
        let reference = identified.reference;
        let pending = self
            .writes
            .pending
            .iter()
            .any(|row| row.identified == reference);
        if pending {
            self.confirmed.check_attempt(context.attempt)?;
        }
        let ready = !pending
            && self.confirmed.contains_or_spill(
                reference,
                context.attempt,
                self.spool,
                context.budget,
            )?;
        if !ready
            && self
                .writes
                .needs_flush(reference, payload.as_slice().len())?
        {
            self.writes
                .flush(context, self.spool, self.confirmed, self.inventory_edits)?;
        }
        // Only the fallback emits the remaining selectors in original order.
        let (first, mut remaining, alias_only) = match selectors {
            PackedSelectors::Terms {
                mut terms,
                alias_only,
            } => {
                if alias_only {
                    let last = terms
                        .len()
                        .checked_sub(1)
                        .ok_or(IndexError::ResourceLimited)?;
                    terms.as_mut_slice().swap(0, last);
                } else {
                    terms.as_mut_slice().reverse();
                }
                let first = terms.pop().ok_or(IndexError::ResourceLimited)?;
                (first.key, Some(terms), alias_only)
            }
            PackedSelectors::AliasFirst { first, .. } => (first, None, true),
        };
        if ready {
            self.push_ready(first, reference, context.budget)?;
            if !alias_only && let Some(terms) = remaining.as_mut() {
                while let Some(term) = terms.pop() {
                    self.push_ready(term.key, reference, context.budget)?;
                }
            }
            return Ok(reference);
        }
        let slot = self.writes.push_with_slot(
            OccurrenceEdit {
                ordinal: self.ordinal,
                key: first,
                prior: None,
                identified: reference,
            },
            payload,
            self.spool,
            self.inventory_edits,
            context.budget,
        )?;
        if !alias_only && let Some(terms) = remaining.as_mut() {
            while let Some(term) = terms.pop() {
                self.writes.push_block_alias(
                    self.ordinal,
                    term.key,
                    slot,
                    self.spool,
                    self.inventory_edits,
                    context.budget,
                )?;
            }
        }
        self.flush_full(context)?;
        Ok(reference)
    }

    fn push_ready(
        &mut self,
        term: PreparedKey,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.inventory_edits
            .push_prepared(term, None, Some(reference), budget)?;
        if self.inventory_edits.rows.len() == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            self.inventory_edits.flush(self.spool, budget)?;
        }
        Ok(())
    }

    fn flush_full(&mut self, context: &mut AttemptContext<'_>) -> Result<(), IndexError> {
        if self.writes.pending.len() == MAX_OBJECT_INSTALL_BATCH {
            self.writes
                .flush(context, self.spool, self.confirmed, self.inventory_edits)?;
        }
        Ok(())
    }
}

impl OccurrenceWrites {
    /// Add another logical selector for a pending block without another
    /// physical object input. The first term was checked by `push` above.
    pub(super) fn push_block_alias(
        &mut self,
        ordinal: u32,
        term: PreparedKey,
        slot: super::PendingOccurrenceSlot,
        spool: &TextSpool,
        inventory_edits: &mut InventoryEdits,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if usize::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?
            >= MAX_INVENTORY_ROOTS
        {
            return Err(IndexError::BackendViolation(
                "occurrence ordinal exceeds its inventory window".into(),
            ));
        }
        check_term_key(term.as_slice())?;
        let fanout_rows = self
            .fanout_rows
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let pending = self
            .pending
            .get(slot.index)
            .filter(|row| row.identified == slot.identified)
            .ok_or_else(|| {
                IndexError::BackendViolation(
                    "packed occurrence alias has no pending physical input".into(),
                )
            })?;
        let reference = pending.identified;
        inventory_edits.push_partition(
            InventoryPartition::Planned(ordinal),
            term,
            None,
            Some(reference),
            budget,
        )?;
        if inventory_edits.rows.len() == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            inventory_edits.flush(spool, budget)?;
        }
        let pending = self
            .pending
            .get_mut(slot.index)
            .ok_or(IndexError::ResourceLimited)?;
        pending.ordinal = pending.ordinal.max(ordinal);
        self.fanout_rows = fanout_rows;
        Ok(())
    }
}
