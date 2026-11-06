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
};
use crate::native::{
    text_memory::{PreparedPayload, PreparedRows},
    text_occurrence::block::{BlockDraft, BlockPush, BlockTerm, MAX_BLOCK_TERMS, SealedBlock},
};

struct PackTarget<'a> {
    ordinal: u32,
    spool: &'a TextSpool,
    confirmed: &'a mut ConfirmedOccurrences,
    inventory_edits: &'a mut InventoryEdits,
    writes: &'a mut OccurrenceWrites,
}

/// Prepare one new owner's sorted token inventory. The caller must establish
/// an empty prior source and empty prior inventory before choosing this path.
pub(super) fn prepare_owner_blocks(
    ordinal: u32,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    confirmed: &mut ConfirmedOccurrences,
    inventory_edits: &mut InventoryEdits,
    writes: &mut OccurrenceWrites,
) -> Result<u64, IndexError> {
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
    let mut draft = BlockDraft::new()?;
    let mut term_count = 0_u64;
    let attempt = context.attempt;
    let objects = context.objects;
    let maps = context.maps;
    let custody = context.custody;
    let scratch = context.scratch;
    spool.visit_token_groups(
        usize::try_from(super::MAX_POSITIONS_PER_TERM)
            .map_err(|_error| IndexError::ResourceLimited)?,
        context.budget,
        |term, positions, budget| {
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
            match draft.try_push(term, positions, group_context.budget)? {
                BlockPush::Added => Ok(()),
                BlockPush::Full { term, positions } => {
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
        },
    )?;
    if draft.len() != 0 {
        let sealed = draft.finish(context.budget)?;
        target.stage_sealed(sealed, context)?;
    }
    Ok(term_count)
}

impl PackTarget<'_> {
    fn stage_sealed(
        &mut self,
        sealed: SealedBlock,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        match sealed {
            SealedBlock::Leaf { term, positions } => self.stage_leaf(term, positions, context),
            SealedBlock::Packed { payload, terms } => {
                self.stage_packed(PreparedPayload::from_staged(payload)?, terms, context)
            }
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
        mut terms: PreparedRows<BlockTerm>,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        if !(2..=MAX_BLOCK_TERMS).contains(&terms.len()) {
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
        // PreparedRows owns its bounded backing. Reverse in place so consuming
        // pop retains the spool's original sorted term order without another Vec.
        terms.as_mut_slice().reverse();
        let first = terms.pop().ok_or(IndexError::ResourceLimited)?;
        if ready {
            self.push_ready(first.key, reference, context.budget)?;
            while let Some(term) = terms.pop() {
                self.push_ready(term.key, reference, context.budget)?;
            }
            return Ok(());
        }
        self.writes.push(
            OccurrenceEdit {
                ordinal: self.ordinal,
                key: first.key,
                prior: None,
                identified: reference,
            },
            payload,
            self.spool,
            self.inventory_edits,
            context.budget,
        )?;
        while let Some(term) = terms.pop() {
            self.writes.push_block_alias(
                self.ordinal,
                term.key,
                reference,
                self.spool,
                self.inventory_edits,
                context.budget,
            )?;
        }
        self.flush_full(context)
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
    fn push_block_alias(
        &mut self,
        ordinal: u32,
        term: PreparedKey,
        reference: ObjectRef,
        spool: &TextSpool,
        inventory_edits: &mut InventoryEdits,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if usize::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?
            >= quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
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
        let slot = self
            .pending
            .iter()
            .position(|row| row.identified == reference)
            .ok_or_else(|| {
                IndexError::BackendViolation(
                    "packed occurrence alias has no pending physical input".into(),
                )
            })?;
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
            .get_mut(slot)
            .ok_or(IndexError::ResourceLimited)?;
        pending.ordinal = pending.ordinal.max(ordinal);
        self.fanout_rows = fanout_rows;
        Ok(())
    }
}
