//! Prepare disposable diffs before completing any owner in a confirmed batch.

use super::{
    AttemptContext, ConfirmedTextChange, Contribution, ContributionWrites, EntryMap, IndexError,
    InventoryBatch, InventoryReuse, PendingTextChange, PendingTextNext, ReadyTextNext, Root,
    TextSpool, VerifiedInventory,
    cold_inventory_cursor::{ColdInventoryCursor, ColdInventoryProvenance},
    cold_posting_run::{self, OwnerInput},
    complete_prepared_change, flush_diff_posting_edits, prepare_inventory_diff,
    validate_reused_inventory,
};
use crate::native::text_memory::PreparedRows;
use quanta_index_core::index::MemoryCreditLimits;

fn cold_new_batch(batch: &InventoryBatch<'_>) -> bool {
    batch.occurrence_writes.pack_owners
        && batch.rows.len() != 0
        && batch
            .rows
            .as_slice()
            .iter()
            .zip(batch.ready.as_slice())
            .all(|(row, ready)| {
                row.prior_ref.is_none()
                    && row.prior.is_none()
                    && match &row.next {
                        PendingTextNext::Draft { slot, .. } => {
                            batch.bases.as_slice().get(*slot) == Some(&EntryMap::default())
                        }
                        PendingTextNext::Ready => !ready.reused && ready.contribution.is_some(),
                        PendingTextNext::Preparing => false,
                    }
            })
}

fn resolve_prepared_owner(
    row: &PendingTextChange,
    ready: &ReadyTextNext,
    roots: Option<&[EntryMap]>,
    pack_owners: bool,
    bases: &[EntryMap],
) -> Result<
    (
        ConfirmedTextChange,
        Option<super::entry_patch::ColdInventoryDiff>,
    ),
    IndexError,
> {
    let verified_alias = if let PendingTextNext::Draft {
        slot,
        alias: Some(alias),
        ..
    } = &row.next
    {
        if !pack_owners || bases.get(*slot) != Some(&EntryMap::default()) {
            return Err(IndexError::BackendViolation(
                "inventory alias requires an empty cold base".into(),
            ));
        }
        let root = roots.and_then(|roots| roots.get(*slot)).ok_or_else(|| {
            IndexError::BackendViolation("inventory alias lost its ordinal".into())
        })?;
        if alias.layout == super::entry_patch::ColdAliasLayout::Directory {
            Some(super::entry_patch::ColdInventoryDiff::Directory(*alias))
        } else {
            Some(super::entry_patch::ColdInventoryDiff::Inline {
                alias: *alias,
                anchor: *root,
            })
        }
    } else {
        None
    };
    let (change, _) = resolve_change(row, ready, roots)?;
    Ok((change, verified_alias))
}

/// Admission uses prepared metadata before opening cursors and posting packets.
fn cold_credit_limits_for_range(
    batch: &InventoryBatch<'_>,
    roots: Option<&[EntryMap]>,
    maps: &dyn quanta_index_core::index::ports::PersistentMapPort,
    start: usize,
    end: usize,
) -> Result<Option<MemoryCreditLimits>, IndexError> {
    let rows = batch
        .rows
        .as_slice()
        .get(start..end)
        .ok_or(IndexError::ResourceLimited)?;
    let ready = batch
        .ready
        .as_slice()
        .get(start..end)
        .ok_or(IndexError::ResourceLimited)?;
    let mut window = None;
    for (row, ready) in rows.iter().zip(ready) {
        let (change, _) = resolve_change(row, ready, roots)?;
        let next = change.next.ok_or_else(|| {
            IndexError::BackendViolation("cold posting owner lost prepared inventory".into())
        })?;
        let alias = match &row.next {
            PendingTextNext::Draft { alias, .. } => *alias,
            PendingTextNext::Ready | PendingTextNext::Preparing => None,
        };
        let provenance = match &row.next {
            PendingTextNext::Draft { .. } => ColdInventoryProvenance::PreparedCanonical,
            PendingTextNext::Ready | PendingTextNext::Preparing => {
                ColdInventoryProvenance::Unsealed
            }
        };
        let receipt = match &row.next {
            PendingTextNext::Draft { slot, .. } => batch
                .directory_receipts
                .as_slice()
                .get(*slot)
                .copied()
                .flatten(),
            PendingTextNext::Ready | PendingTextNext::Preparing => None,
        };
        let Some(limits) = ColdInventoryCursor::credit_limits(
            &next,
            alias.as_ref(),
            receipt.as_ref(),
            provenance,
            maps,
        )?
        else {
            return Ok(None);
        };
        window = Some(match window {
            Some(prior) => MemoryCreditLimits::checked_add(prior, limits)?,
            None => limits,
        });
    }
    Ok(Some(
        window
            .ok_or(IndexError::ResourceLimited)?
            .checked_add(cold_posting_run::local_credit_limits(
                rows.len(),
                batch.posting_edits.limit,
            )?)?
            .checked_add(TextSpool::ordered_cold_run_credit_limits()?)?,
    ))
}

/// Selected runs are disposable until owner EOF seals and prepare finish.
///
/// A failure after this guard starts poisons the attempt.
fn prepare_cold_window(
    batch: &mut InventoryBatch<'_>,
    roots: Option<&[EntryMap]>,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    start: usize,
    end: usize,
) -> Result<Option<()>, IndexError> {
    // The child is already admitted. Optional reservations use its Original
    // parent, so spool-owned metadata cannot keep a window's child alive.
    // A child admission miss therefore retains no unused registry capacity.
    if !spool.prepare_ordered_cold_run_parent(context.budget)? {
        return Ok(None);
    }
    let Some(run) = spool.begin_ordered_cold_run(context.budget)? else {
        return Ok(None);
    };
    let rows = batch
        .rows
        .as_slice()
        .get(start..end)
        .ok_or(IndexError::ResourceLimited)?;
    let ready = batch
        .ready
        .as_slice()
        .get(start..end)
        .ok_or(IndexError::ResourceLimited)?;
    let mut owners = PreparedRows::with_capacity(rows.len(), context.budget)?;
    for (row, ready) in rows.iter().zip(ready) {
        context.budget.checkpoint()?;
        let (change, verified_alias) = resolve_prepared_owner(
            row,
            ready,
            roots,
            batch.occurrence_writes.pack_owners,
            batch.bases.as_slice(),
        )?;
        if change.reused_inventory || change.prior.is_some() || change.prior_ref.is_some() {
            return Err(IndexError::BackendViolation(
                "ordered cold posting owner is not new".into(),
            ));
        }
        let next = change.next.ok_or_else(|| {
            IndexError::BackendViolation("ordered cold posting owner is absent".into())
        })?;
        let alias = verified_alias.map(|proof| match proof {
            super::entry_patch::ColdInventoryDiff::Directory(alias) => (alias, None),
            super::entry_patch::ColdInventoryDiff::Inline { alias, anchor } => {
                (alias, Some(anchor))
            }
        });
        owners.push(
            OwnerInput {
                owner: change.owner,
                next,
                alias,
                directory_receipt: match &row.next {
                    PendingTextNext::Draft { slot, .. } => batch
                        .directory_receipts
                        .as_slice()
                        .get(*slot)
                        .copied()
                        .flatten(),
                    PendingTextNext::Ready | PendingTextNext::Preparing => None,
                },
                provenance: match &row.next {
                    PendingTextNext::Draft { .. } => ColdInventoryProvenance::PreparedCanonical,
                    PendingTextNext::Ready | PendingTextNext::Preparing => {
                        ColdInventoryProvenance::Unsealed
                    }
                },
            },
            context.budget,
        )?;
    }
    let diagnostic = context.budget.request().diagnostic_context().cloned();
    super::observe_text_step(
        diagnostic.as_ref(),
        super::DiagnosticStageV1::NativeTextInventoryRead,
        32,
        context,
        |context| {
            cold_posting_run::run_window(
                owners.as_slice(),
                context,
                spool,
                &mut batch.posting_edits,
            )
        },
    )?;
    run.finish(context.budget)?;
    Ok(Some(()))
}

enum ColdProgress {
    Complete,
    UnsupportedOwner(usize),
    CreditMiss(usize),
}

/// Read the supported prefix of the remaining owners in ordered windows.
/// The caller can resume after an unsupported owner. A credit miss retains
/// the existing owner-major suffix without repeated admission attempts.
fn prepare_cold_ordered(
    batch: &mut InventoryBatch<'_>,
    roots: Option<&[EntryMap]>,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    mut start: usize,
) -> Result<ColdProgress, IndexError> {
    let width = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS;
    while start < batch.rows.len() {
        context.budget.checkpoint()?;
        let maximum_end = start
            .checked_add(width)
            .ok_or(IndexError::ResourceLimited)?
            .min(batch.rows.len());
        let mut eligible_end = start;
        while eligible_end < maximum_end {
            let next = eligible_end
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if cold_credit_limits_for_range(batch, roots, context.maps, eligible_end, next)?
                .is_none()
            {
                break;
            }
            eligible_end = next;
        }
        if eligible_end == start {
            return Ok(ColdProgress::UnsupportedOwner(start));
        }
        let remaining = eligible_end
            .checked_sub(start)
            .ok_or(IndexError::ResourceLimited)?;
        let mut slots = remaining.min(width);
        loop {
            let end = start
                .checked_add(slots)
                .ok_or(IndexError::ResourceLimited)?;
            let Some(limits) =
                cold_credit_limits_for_range(batch, roots, context.maps, start, end)?
            else {
                return Err(IndexError::BackendViolation(
                    "cold posting shape changed after preflight".into(),
                ));
            };
            let attempt = context.attempt;
            let objects = context.objects;
            let maps = context.maps;
            let custody = context.custody;
            let scratch = context.scratch;
            let admitted =
                if let Some(mut credit) = context.budget.try_scoped_memory_credit(limits)? {
                    let mut scoped_context = AttemptContext {
                        attempt,
                        objects,
                        maps,
                        custody,
                        scratch,
                        budget: &mut credit,
                    };
                    prepare_cold_window(batch, roots, &mut scoped_context, spool, start, end)?
                } else {
                    None
                };
            if admitted.is_some() {
                start = end;
                break;
            }
            if slots == 1 {
                return Ok(ColdProgress::CreditMiss(start));
            }
            slots = slots
                .checked_add(1)
                .and_then(|next| next.checked_div(2))
                .ok_or(IndexError::ResourceLimited)?;
        }
    }
    Ok(ColdProgress::Complete)
}

/// A borrowed view of the canonical batch after every alias and diff passed.
///
/// Its exclusive batch borrow prevents inputs or staged roots from changing
/// between preparation and completion. This view retains no decoded block;
/// the existing bounded occurrence reader keeps its original cache policy.
pub(super) struct PreparedInventoryChanges<'batch, 'objects> {
    batch: &'batch mut InventoryBatch<'objects>,
    roots: Option<&'batch [EntryMap]>,
    spool: &'batch TextSpool,
    attempt: quanta_index_core::index::AttemptRef,
    completed: bool,
}

fn resolve_change(
    row: &PendingTextChange,
    ready: &ReadyTextNext,
    roots: Option<&[EntryMap]>,
) -> Result<(ConfirmedTextChange, Option<VerifiedInventory>), IndexError> {
    let (next, reused, verified) = match &row.next {
        PendingTextNext::Preparing => {
            return Err(IndexError::BackendViolation(
                "unfinished inventory owner".into(),
            ));
        }
        PendingTextNext::Ready => (ready.contribution, ready.reused, ready.verified),
        PendingTextNext::Draft {
            record,
            slot,
            body,
            tokens,
            alias,
            shared,
        } => {
            let staged = roots
                .and_then(|roots| roots.get(*slot))
                .copied()
                .ok_or_else(|| {
                    IndexError::BackendViolation("inventory batch lost ordinal output".into())
                })?;
            let inventory = alias
                .filter(|alias| alias.layout == super::entry_patch::ColdAliasLayout::Inline)
                .map_or(staged, |alias| EntryMap::OccurrenceBlock(alias.reference));
            (
                Some(Contribution {
                    record: *record,
                    inventory,
                    indexed_tokens: *tokens,
                }),
                false,
                Some(VerifiedInventory {
                    bytes: body.bytes,
                    byte_len: body.byte_len,
                    inventory,
                    indexed_tokens: *tokens,
                    repeated: *shared,
                }),
            )
        }
    };
    Ok((
        ConfirmedTextChange {
            owner: row.owner,
            prior_ref: row.prior_ref,
            prior: row.prior,
            next,
            reused_inventory: reused,
        },
        verified,
    ))
}

impl<'batch, 'objects> PreparedInventoryChanges<'batch, 'objects> {
    pub(super) fn prepare(
        batch: &'batch mut InventoryBatch<'objects>,
        roots: Option<&'batch [EntryMap]>,
        context: &mut AttemptContext<'_>,
        spool: &'batch TextSpool,
        inventories: &mut InventoryReuse,
    ) -> Result<Self, IndexError> {
        batch.check_attempt(context)?;
        if batch.rows.len() != batch.ready.len() {
            return Err(IndexError::BackendViolation(
                "inventory preparation lost owner cardinality".into(),
            ));
        }
        let mut start = 0;
        let mut ordered = cold_new_batch(batch);
        while start < batch.rows.len() {
            let end = if ordered {
                match prepare_cold_ordered(batch, roots, context, spool, start)? {
                    ColdProgress::Complete => break,
                    ColdProgress::UnsupportedOwner(owner) => {
                        start = owner;
                        let mut end = owner.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                        while end < batch.rows.len() {
                            context.budget.checkpoint()?;
                            let next = end.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                            if cold_credit_limits_for_range(batch, roots, context.maps, end, next)?
                                .is_some()
                            {
                                break;
                            }
                            end = next;
                        }
                        end
                    }
                    ColdProgress::CreditMiss(owner) => {
                        start = owner;
                        ordered = false;
                        batch.rows.len()
                    }
                }
            } else {
                batch.rows.len()
            };
            for (row, ready) in batch
                .rows
                .as_slice()
                .get(start..end)
                .ok_or(IndexError::ResourceLimited)?
                .iter()
                .zip(
                    batch
                        .ready
                        .as_slice()
                        .get(start..end)
                        .ok_or(IndexError::ResourceLimited)?,
                )
            {
                context.budget.checkpoint()?;
                let (change, verified_alias) = resolve_prepared_owner(
                    row,
                    ready,
                    roots,
                    batch.occurrence_writes.pack_owners,
                    batch.bases.as_slice(),
                )?;
                if change.reused_inventory {
                    if verified_alias.is_some() {
                        return Err(IndexError::BackendViolation(
                            "published inventory reuse cannot consume a cold alias".into(),
                        ));
                    }
                    let prior = change.prior.as_ref().ok_or_else(|| {
                        IndexError::BackendViolation("native text reusable prior missing".into())
                    })?;
                    let next = change.next.as_ref().ok_or_else(|| {
                        IndexError::BackendViolation("native text reusable next missing".into())
                    })?;
                    validate_reused_inventory(prior, next)?;
                } else {
                    prepare_inventory_diff(
                        &change.owner,
                        change.prior.as_ref(),
                        change.next.as_ref(),
                        context,
                        spool,
                        &mut batch.posting_edits,
                        &mut inventories.occurrences,
                        verified_alias,
                    )?;
                }
            }
            // These rows are provisional until every owner passed. A later error
            // poisons the enclosing spool; no prepared prefix may be completed.
            flush_diff_posting_edits(&mut batch.posting_edits, spool, context)?;
            start = end;
            if ordered && start < batch.rows.len() {
                // Finish this run under the parent grant before attempting
                // another ordered window's narrower child allocation.
                spool.finish_owner_major_posting_run(context.budget)?;
            }
        }
        Ok(Self {
            batch,
            roots,
            spool,
            attempt: context.attempt,
            completed: false,
        })
    }

    pub(super) fn complete(
        mut self,
        root: &mut Root,
        context: &mut AttemptContext<'_>,
        contributions: &mut ContributionWrites,
        inventories: &mut InventoryReuse,
    ) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        if self.attempt != context.attempt {
            return Err(IndexError::FenceLost);
        }
        self.batch.check_attempt(context)?;
        for (row, ready) in self
            .batch
            .rows
            .as_slice()
            .iter()
            .zip(self.batch.ready.as_slice())
        {
            context.budget.checkpoint()?;
            let (change, verified) = resolve_change(row, ready, self.roots)?;
            if let Some(verified) = verified {
                inventories.remember_verified(&verified, context.budget)?;
            }
            complete_prepared_change(&change, root, context, self.spool, contributions)?;
        }
        self.completed = true;
        Ok(())
    }
}

impl Drop for PreparedInventoryChanges<'_, '_> {
    fn drop(&mut self) {
        if !self.completed {
            self.batch.failed = true;
            self.spool.invalidate_inventory();
        }
    }
}
