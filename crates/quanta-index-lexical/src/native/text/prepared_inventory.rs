//! Prepare disposable diffs before completing any owner in a confirmed batch.

use super::{
    AttemptContext, ConfirmedTextChange, Contribution, ContributionWrites, EntryMap, IndexError,
    InventoryBatch, InventoryReuse, PendingTextChange, PendingTextNext, ReadyTextNext, Root,
    TextSpool, VerifiedInventory, complete_prepared_change, flush_diff_posting_edits,
    prepare_inventory_diff, validate_reused_inventory,
};

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
            ..
        } => {
            let inventory = roots
                .and_then(|roots| roots.get(*slot))
                .copied()
                .ok_or_else(|| {
                    IndexError::BackendViolation("inventory batch lost ordinal output".into())
                })?;
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
        mut roots: Option<&'batch mut [EntryMap]>,
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
        for (row, ready) in batch.rows.as_slice().iter().zip(batch.ready.as_slice()) {
            context.budget.checkpoint()?;
            let verified_alias = if let PendingTextNext::Draft {
                slot,
                alias: Some(alias),
                ..
            } = &row.next
            {
                if !batch.occurrence_writes.pack_owners
                    || batch.bases.as_slice().get(*slot) != Some(&EntryMap::default())
                {
                    return Err(IndexError::BackendViolation(
                        "inventory alias requires an empty cold base".into(),
                    ));
                }
                let root = roots
                    .as_deref_mut()
                    .and_then(|roots| roots.get_mut(*slot))
                    .ok_or_else(|| {
                        IndexError::BackendViolation("inventory alias lost its ordinal".into())
                    })?;
                if alias.layout == super::entry_patch::ColdAliasLayout::Directory {
                    Some(super::entry_patch::ColdInventoryDiff::Directory(*alias))
                } else {
                    let anchor = *root;
                    *root = EntryMap::OccurrenceBlock(alias.reference);
                    Some(super::entry_patch::ColdInventoryDiff::Inline {
                        alias: *alias,
                        anchor,
                    })
                }
            } else {
                None
            };
            let (change, _) = resolve_change(row, ready, roots.as_deref())?;
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
        Ok(Self {
            batch,
            roots: roots.map(|roots| &*roots),
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
