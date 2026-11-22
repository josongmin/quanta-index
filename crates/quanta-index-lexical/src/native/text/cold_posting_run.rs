//! Term-major phase-zero production from bounded, independently checked owners.

use std::cmp::Ordering;

use quanta_index_core::index::{
    AttemptContext, IndexError, MemoryCreditLimits, MemoryLease, MemoryLifetime, WorkBudget,
    WorkUsage,
};

use super::{
    Contribution, EntryMap, PostingEdits, PostingScratchRow, TextSpool,
    cold_inventory_cursor::{ColdInventoryCursor, ColdInventoryProvenance},
    entry_patch::ColdInventoryAlias,
};
use crate::native::{text_memory::PreparedRows, text_spool::PostingTermKey};

/// Caller-owned arrays and the bounded posting edit window that coexist with
/// one owner-head group. Checked DATA and packet writer have separate bounds.
pub(super) fn local_credit_limits(
    owners: usize,
    posting_width: usize,
) -> Result<MemoryCreditLimits, IndexError> {
    if owners == 0 || owners > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
        return Err(IndexError::ResourceLimited);
    }
    // The negotiated writer has only these two widths. PreparedRows doubles
    // through half the width before its final replacement.
    if posting_width != quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
        && posting_width != quanta_index_scratch_sqlite::MAX_WIDE_WRITE_BATCH_ROWS
    {
        return Err(IndexError::ResourceLimited);
    }
    let arrays = owners
        .checked_mul(
            std::mem::size_of::<OwnerInput>()
                .checked_add(std::mem::size_of::<ColdInventoryCursor<'static>>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[Option<u8>; 128]>()))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[u8; 64]>()))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[u16; 64]>()))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[u8; 64]>()))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[u8; 64]>()))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Option<u8>>()))
        .ok_or(IndexError::ResourceLimited)?;
    // PreparedRows retains its old half-width backing while reserving the
    // negotiated writer width (64 or 128) on the final growth step.
    let posting_rows = posting_width
        .checked_add(
            posting_width
                .checked_div(2)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?
        .checked_mul(std::mem::size_of::<PostingScratchRow>())
        .ok_or(IndexError::ResourceLimited)?;
    let maximum_key_bytes =
        usize::try_from(super::MAX_TERM_KEY_BYTES).map_err(|_error| IndexError::ResourceLimited)?;
    let long_keys = posting_width
        .checked_add(1)
        .and_then(|count| count.checked_mul(maximum_key_bytes))
        .ok_or(IndexError::ResourceLimited)?;
    let temporary = arrays
        .checked_add(posting_rows)
        .and_then(|bytes| bytes.checked_add(long_keys))
        .ok_or(IndexError::ResourceLimited)?;
    Ok(MemoryCreditLimits {
        temporary_data_bytes: u64::try_from(temporary)
            .map_err(|_error| IndexError::ResourceLimited)?,
        retained_data_bytes: 0,
        max_live_temporary_leases: u32::try_from(
            posting_width
                .checked_add(1 + 5)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?,
        max_live_retained_leases: 0,
    })
}

pub(super) struct OwnerInput {
    pub(super) owner: [u8; 32],
    pub(super) provenance: ColdInventoryProvenance,
    pub(super) next: Contribution,
    pub(super) alias: Option<(ColdInventoryAlias, Option<EntryMap>)>,
    pub(super) directory_receipt: Option<super::entry_patch::DirectoryReceipt>,
}

#[derive(Clone, Copy)]
struct Loser {
    slot: u8,
    // Exact LCP with the winner at this node, never an abbreviated key.
    lcp: u16,
}

/// A bounded tournament over borrowed authenticated cursor heads. Each game
/// retains its loser and exact LCP with its winner for one path repair.
struct WinnerTree {
    root: Option<u8>,
    loser_slots: [u8; 64],
    loser_lcps: [u16; 64],
    ranks: [u8; 64],
    base: usize,
    limit: usize,
    // Retained tournament metadata drops before its one Original grant.
    _memory: Option<MemoryLease>,
}

impl WinnerTree {
    fn new(owners: &[OwnerInput], budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let capacity = owners.len();
        if capacity == 0 || capacity > 64 {
            return Err(IndexError::ResourceLimited);
        }
        let bytes = std::mem::size_of::<[u8; 64]>()
            .checked_add(std::mem::size_of::<[u16; 64]>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[u8; 64]>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Option<u8>>()))
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        // Fixed arrays are initialized once. A bounded binary-insertion sort
        // charges each owner comparison and shifted slot before cursor reads.
        let initialized_bytes = std::mem::size_of::<[u8; 64]>()
            .checked_add(std::mem::size_of::<[u16; 64]>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[u8; 64]>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Option<u8>>()))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(initialized_bytes)
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let ranks = {
            let _order_memory = budget.reserve_memory(
                u64::try_from(std::mem::size_of::<[u8; 64]>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?;
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<[u8; 64]>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let mut order = [0_u8; 64];
            let mut ranks = [0_u8; 64];
            for slot in 0..capacity {
                let mut low = 0usize;
                let mut high = slot;
                while low < high {
                    budget.checkpoint()?;
                    budget.charge(WorkUsage {
                        bytes: 32,
                        ..WorkUsage::default()
                    })?;
                    let middle = low
                        .checked_add(high)
                        .and_then(|sum| sum.checked_div(2))
                        .ok_or(IndexError::ResourceLimited)?;
                    let prior = usize::from(*order.get(middle).ok_or(IndexError::ResourceLimited)?);
                    let owner = owners.get(slot).ok_or(IndexError::ResourceLimited)?;
                    let prior_owner = owners.get(prior).ok_or(IndexError::ResourceLimited)?;
                    if owner.owner < prior_owner.owner {
                        high = middle;
                    } else {
                        low = middle.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    }
                }
                let shifted = slot.checked_sub(low).ok_or(IndexError::ResourceLimited)?;
                budget.charge(WorkUsage {
                    bytes: u64::try_from(
                        shifted.checked_add(1).ok_or(IndexError::ResourceLimited)?,
                    )
                    .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                order.copy_within(
                    low..slot,
                    low.checked_add(1).ok_or(IndexError::ResourceLimited)?,
                );
                *order.get_mut(low).ok_or(IndexError::ResourceLimited)? =
                    u8::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?;
            }
            let mut rank = 0_u8;
            for sorted in 0..capacity {
                if sorted > 0 {
                    budget.charge(WorkUsage {
                        bytes: 32,
                        ..WorkUsage::default()
                    })?;
                    let preceding = sorted.checked_sub(1).ok_or(IndexError::ResourceLimited)?;
                    let prior =
                        usize::from(*order.get(preceding).ok_or(IndexError::ResourceLimited)?);
                    let current =
                        usize::from(*order.get(sorted).ok_or(IndexError::ResourceLimited)?);
                    if owners.get(prior).ok_or(IndexError::ResourceLimited)?.owner
                        != owners
                            .get(current)
                            .ok_or(IndexError::ResourceLimited)?
                            .owner
                    {
                        rank =
                            u8::try_from(sorted).map_err(|_error| IndexError::ResourceLimited)?;
                    }
                }
                budget.charge(WorkUsage {
                    bytes: 1,
                    ..WorkUsage::default()
                })?;
                let slot = usize::from(*order.get(sorted).ok_or(IndexError::ResourceLimited)?);
                *ranks.get_mut(slot).ok_or(IndexError::ResourceLimited)? = rank;
            }
            ranks
        };
        Ok(Self {
            root: None,
            loser_slots: [u8::MAX; 64],
            loser_lcps: [0; 64],
            ranks,
            base: capacity.next_power_of_two(),
            limit: capacity,
            _memory: memory,
        })
    }

    fn compare_bytes(
        left: &[u8],
        right: &[u8],
        known_lcp: usize,
        budget: &mut WorkBudget,
    ) -> Result<(Ordering, u16), IndexError> {
        budget.checkpoint()?;
        let left_suffix = left.get(known_lcp..).ok_or(IndexError::ResourceLimited)?;
        let right_suffix = right.get(known_lcp..).ok_or(IndexError::ResourceLimited)?;
        let mut matched = 0usize;
        let mut order = Ordering::Equal;
        for (a, b) in left_suffix.iter().zip(right_suffix) {
            if a != b {
                order = a.cmp(b);
                break;
            }
            matched = matched.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        if order == Ordering::Equal {
            order = left_suffix.len().cmp(&right_suffix.len());
        }
        // Preserve the existing comparison's conservative per-suffix byte
        // charge. Only bytes in a prefix proved by earlier games are skipped.
        let charged = left_suffix
            .len()
            .min(right_suffix.len())
            .checked_add(usize::from(order == Ordering::Equal))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(charged).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let lcp = known_lcp
            .checked_add(matched)
            .ok_or(IndexError::ResourceLimited)?;
        Ok((
            order,
            u16::try_from(lcp).map_err(|_error| IndexError::ResourceLimited)?,
        ))
    }

    fn compare_suffix(
        &self,
        cursors: &[ColdInventoryCursor<'_>],
        left_slot: usize,
        right_slot: usize,
        known_lcp: usize,
        budget: &mut WorkBudget,
    ) -> Result<(Ordering, u16), IndexError> {
        let (left, _) = cursors
            .get(left_slot)
            .ok_or(IndexError::ResourceLimited)?
            .current()?
            .ok_or(IndexError::ResourceLimited)?;
        let (right, _) = cursors
            .get(right_slot)
            .ok_or(IndexError::ResourceLimited)?
            .current()?
            .ok_or(IndexError::ResourceLimited)?;
        let (term, lcp) = Self::compare_bytes(left, right, known_lcp, budget)?;
        if term == Ordering::Equal {
            let left_rank = self
                .ranks
                .get(left_slot)
                .ok_or(IndexError::ResourceLimited)?;
            let right_rank = self
                .ranks
                .get(right_slot)
                .ok_or(IndexError::ResourceLimited)?;
            Ok((
                left_rank.cmp(right_rank).then(left_slot.cmp(&right_slot)),
                lcp,
            ))
        } else {
            Ok((term, lcp))
        }
    }

    fn choose_initial(
        &self,
        cursors: &[ColdInventoryCursor<'_>],
        left: Option<u8>,
        right: Option<u8>,
        budget: &mut WorkBudget,
    ) -> Result<(Option<u8>, Option<Loser>), IndexError> {
        match (left, right) {
            (Some(left), Some(right)) => {
                let (order, lcp) =
                    self.compare_suffix(cursors, usize::from(left), usize::from(right), 0, budget)?;
                if order == Ordering::Greater {
                    Ok((Some(right), Some(Loser { slot: left, lcp })))
                } else {
                    Ok((Some(left), Some(Loser { slot: right, lcp })))
                }
            }
            (Some(left), None) => Ok((Some(left), None)),
            (None, Some(right)) => Ok((Some(right), None)),
            (None, None) => Ok((None, None)),
        }
    }

    fn read_loser(
        &self,
        node: usize,
        budget: &mut WorkBudget,
    ) -> Result<Option<Loser>, IndexError> {
        budget.charge(WorkUsage {
            bytes: 1,
            ..WorkUsage::default()
        })?;
        let slot = *self
            .loser_slots
            .get(node)
            .ok_or(IndexError::ResourceLimited)?;
        if slot == u8::MAX {
            return Ok(None);
        }
        if usize::from(slot) >= self.limit {
            return Err(IndexError::BackendViolation(
                "cold loser slot exceeds owner window".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: 2,
            ..WorkUsage::default()
        })?;
        Ok(Some(Loser {
            slot,
            lcp: *self
                .loser_lcps
                .get(node)
                .ok_or(IndexError::ResourceLimited)?,
        }))
    }

    fn write_loser(
        &mut self,
        node: usize,
        old: Option<Loser>,
        next: Option<Loser>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let old_slot = old.map_or(u8::MAX, |loser| loser.slot);
        let next_slot = next.map_or(u8::MAX, |loser| loser.slot);
        let old_lcp = old.map_or(0, |loser| loser.lcp);
        let next_lcp = next.map_or(0, |loser| loser.lcp);
        let slot_changed = old_slot != next_slot;
        let lcp_changed = old_lcp != next_lcp;
        let bytes = u64::from(lcp_changed)
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(u64::from(slot_changed)))
            .ok_or(IndexError::ResourceLimited)?;
        if bytes > 0 {
            budget.charge(WorkUsage {
                bytes,
                ..WorkUsage::default()
            })?;
        }
        if slot_changed {
            *self
                .loser_slots
                .get_mut(node)
                .ok_or(IndexError::ResourceLimited)? = next_slot;
        }
        if lcp_changed {
            *self
                .loser_lcps
                .get_mut(node)
                .ok_or(IndexError::ResourceLimited)? = next_lcp;
        }
        Ok(())
    }

    fn write_build_winner(
        winners: &mut [Option<u8>; 128],
        node: usize,
        winner: Option<u8>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if winners
            .get(node)
            .copied()
            .ok_or(IndexError::ResourceLimited)?
            == winner
        {
            return Ok(());
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(std::mem::size_of::<Option<u8>>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        *winners.get_mut(node).ok_or(IndexError::ResourceLimited)? = winner;
        Ok(())
    }

    fn build(
        &mut self,
        cursors: &[ColdInventoryCursor<'_>],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if cursors.len() != self.limit {
            return Err(IndexError::BackendViolation(
                "cold winner cursor cardinality differs".into(),
            ));
        }
        let root = {
            let _build_memory = budget.reserve_memory(
                u64::try_from(std::mem::size_of::<[Option<u8>; 128]>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?;
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<[Option<u8>; 128]>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let mut winners = [None; 128];
            for slot in 0..self.limit {
                let winner = cursors
                    .get(slot)
                    .ok_or(IndexError::ResourceLimited)?
                    .current()?
                    .map(|_| u8::try_from(slot).map_err(|_error| IndexError::ResourceLimited))
                    .transpose()?;
                let leaf = self
                    .base
                    .checked_add(slot)
                    .ok_or(IndexError::ResourceLimited)?;
                Self::write_build_winner(&mut winners, leaf, winner, budget)?;
            }
            for node in (1..self.base).rev() {
                let left = node.checked_mul(2).ok_or(IndexError::ResourceLimited)?;
                let right = left.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                let (winner, loser) = self.choose_initial(
                    cursors,
                    *winners.get(left).ok_or(IndexError::ResourceLimited)?,
                    *winners.get(right).ok_or(IndexError::ResourceLimited)?,
                    budget,
                )?;
                self.write_loser(node, None, loser, budget)?;
                Self::write_build_winner(&mut winners, node, winner, budget)?;
            }
            *winners.get(1).ok_or(IndexError::ResourceLimited)?
        };
        budget.charge(WorkUsage {
            bytes: u64::try_from(std::mem::size_of::<Option<u8>>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.root = root;
        Ok(())
    }

    fn winner_slot(&self) -> Option<usize> {
        self.root.map(usize::from)
    }

    fn replace(
        &mut self,
        cursors: &[ColdInventoryCursor<'_>],
        slot: usize,
        active: bool,
        successor_lcp: Option<u16>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.winner_slot() != Some(slot)
            || active != successor_lcp.is_some()
            || cursors
                .get(slot)
                .ok_or(IndexError::ResourceLimited)?
                .current()?
                .is_some()
                != active
        {
            return Err(IndexError::BackendViolation(
                "cold winner replacement slot differs".into(),
            ));
        }
        let leaf = self
            .base
            .checked_add(slot)
            .ok_or(IndexError::ResourceLimited)?;
        let winner = active
            .then(|| u8::try_from(slot).map_err(|_error| IndexError::ResourceLimited))
            .transpose()?;
        // Every replay candidate carries its exact LCP with the emitted key.
        let mut moving = winner.zip(successor_lcp);
        let mut node = leaf.checked_div(2).ok_or(IndexError::ResourceLimited)?;
        while node > 0 {
            let prior_loser = self.read_loser(node, budget)?;
            if prior_loser.is_some_and(|loser| usize::from(loser.slot) == slot) {
                return Err(IndexError::BackendViolation(
                    "cold loser aliases the emitted owner".into(),
                ));
            }
            let (next, loser) = match (moving, prior_loser) {
                (Some((candidate, candidate_lcp)), Some(prior)) => {
                    // Both LCPs use the same emitted key. Neither candidate
                    // precedes that key, so unequal LCPs decide the game
                    // without reading either already proved prefix.
                    let (order, pair_lcp) = match candidate_lcp.cmp(&prior.lcp) {
                        Ordering::Less => (Ordering::Greater, candidate_lcp),
                        Ordering::Greater => (Ordering::Less, prior.lcp),
                        Ordering::Equal => self.compare_suffix(
                            cursors,
                            usize::from(candidate),
                            usize::from(prior.slot),
                            usize::from(candidate_lcp),
                            budget,
                        )?,
                    };
                    if order == Ordering::Greater {
                        (
                            Some((prior.slot, prior.lcp)),
                            Some(Loser {
                                slot: candidate,
                                lcp: pair_lcp,
                            }),
                        )
                    } else {
                        (
                            Some((candidate, candidate_lcp)),
                            Some(Loser {
                                slot: prior.slot,
                                lcp: pair_lcp,
                            }),
                        )
                    }
                }
                (Some(candidate), None) => (Some(candidate), None),
                (None, Some(prior)) => (Some((prior.slot, prior.lcp)), None),
                (None, None) => (None, None),
            };
            self.write_loser(node, prior_loser, loser, budget)?;
            moving = next;
            node = node.checked_div(2).ok_or(IndexError::ResourceLimited)?;
        }
        let next_root = moving.map(|(slot, _)| slot);
        if self.root != next_root {
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<Option<u8>>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            self.root = next_root;
        }
        Ok(())
    }
}

/// Exhaust every owner and its final EOF seal before the caller closes the
/// already active ordered run. The enclosing inventory flush poisons scratch
/// on any error.
pub(super) fn run_window(
    owners: &[OwnerInput],
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    posting_edits: &mut PostingEdits,
) -> Result<(), IndexError> {
    if owners.is_empty() || owners.len() > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
        return Err(IndexError::InvalidInput(
            "ordered cold owner window exceeds bound".into(),
        ));
    }
    let mut cursors = PreparedRows::with_capacity(owners.len(), context.budget)?;
    let mut winners = WinnerTree::new(owners, context.budget)?;
    for input in owners {
        context.budget.checkpoint()?;
        let mut cursor = ColdInventoryCursor::open(
            &input.next,
            input.alias.as_ref(),
            input.directory_receipt.as_ref(),
            input.provenance,
            context.objects,
            context.maps,
            context.attempt,
            context.budget,
        )?;
        let _active = cursor.advance(context)?;
        cursors.push(cursor, context.budget)?;
    }
    winners.build(cursors.as_slice(), context.budget)?;
    while let Some(slot) = winners.winner_slot() {
        let (term, reference) = cursors
            .as_slice()
            .get(slot)
            .ok_or(IndexError::ResourceLimited)?
            .current()?
            .ok_or_else(|| IndexError::BackendViolation("cold winner has no current row".into()))?;
        // The checked cursor keeps the key and reference alive until advance.
        // Only the output row owns a term copy; the tournament stores slots.
        posting_edits.push(
            term,
            &owners.get(slot).ok_or(IndexError::ResourceLimited)?.owner,
            None,
            Some(reference),
            context.budget,
        )?;
        let held_term = if posting_edits.is_full() {
            Some(
                super::flush_diff_posting_edits_return_last(posting_edits, spool, context)?
                    .ok_or(IndexError::ResourceLimited)?,
            )
        } else {
            None
        };
        let cursor = cursors
            .as_mut_slice()
            .get_mut(slot)
            .ok_or(IndexError::ResourceLimited)?;
        let active = cursor.advance(context)?;
        let successor_lcp = if active {
            let old = held_term
                .as_ref()
                .map(PostingTermKey::as_slice)
                .or_else(|| posting_edits.last_term())
                .ok_or(IndexError::ResourceLimited)?;
            let (next, _) = cursors
                .as_slice()
                .get(slot)
                .ok_or(IndexError::ResourceLimited)?
                .current()?
                .ok_or(IndexError::ResourceLimited)?;
            let (order, lcp) = WinnerTree::compare_bytes(old, next, 0, context.budget)?;
            if order != Ordering::Less {
                return Err(IndexError::CorruptObject(
                    "cold owner terms are not strictly ordered".into(),
                ));
            }
            Some(lcp)
        } else {
            None
        };
        winners.replace(
            cursors.as_slice(),
            slot,
            active,
            successor_lcp,
            context.budget,
        )?;
    }
    super::flush_diff_posting_edits(posting_edits, spool, context)?;
    posting_edits.release_empty_backing()?;
    Ok(())
}
