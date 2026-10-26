//! Bind compact metadata to the complete original CAS stream before publication.

use super::{
    MapEntries, PreparedRows,
    entry_map::{EntryMap, INLINE_ENTRIES, SmallMap},
};
use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MapRoot, OrdinalMapEdit, WorkBudget, WorkUsage,
    ports::MapEditCursor,
};
use sha2::{Digest as _, Sha256};

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

#[derive(Clone, Copy)]
struct Plan {
    small: Option<SmallMap>,
    stamp: Option<Stamp>,
}

fn resolve_small(
    prior: &EntryMap,
    count: u64,
    edits: &mut dyn MapEditCursor,
    context: &mut AttemptContext<'_>,
) -> Result<Plan, IndexError> {
    let mut input = RootInput {
        cursor: edits,
        previous: None,
        digest: Edits::new(),
    };
    let mut before = MapEntries::new(context.maps, prior);
    let mut old = before.next(context.budget)?;
    let mut delta = input.next(context.budget)?;
    let mut small = Some(SmallMap::default());
    let mut actual = 0_u64;
    while old.is_some() || delta.is_some() {
        context.budget.checkpoint()?;
        let old_row = old.as_ref().map(|entry| entry.object_parts()).transpose()?;
        let ordering = match (old_row, delta.as_ref()) {
            (Some((key, _)), Some(edit)) => key.as_slice().cmp(&edit.edit.key),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => break,
        };
        let (key, value) = match ordering {
            std::cmp::Ordering::Less => {
                let (key, value) = old_row
                    .ok_or_else(|| IndexError::BackendViolation("entry merge lost base".into()))?;
                (key.as_slice(), Some(*value))
            }
            std::cmp::Ordering::Equal | std::cmp::Ordering::Greater => {
                let edit = delta
                    .as_ref()
                    .ok_or_else(|| IndexError::BackendViolation("entry merge lost delta".into()))?;
                let expected = if ordering == std::cmp::Ordering::Equal {
                    old_row.map(|(_, value)| *value)
                } else {
                    None
                };
                if edit.edit.expected != expected.map(Into::into) {
                    return Err(IndexError::Conflict);
                }
                (
                    edit.edit.key.as_slice(),
                    edit.edit
                        .next
                        .map(quanta_index_core::index::MapValue::object_ref)
                        .transpose()?,
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
            if let Some(map) = small.as_mut()
                && !map.push(key, value)?
            {
                small = None;
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
    Ok(Plan {
        small,
        stamp: Some(input.digest.finish()),
    })
}

/// Revalidate the complete original stream before publishing compact metadata.
///
/// Input is consumed by the map producer or drained here. Per-root digests forbid using a
/// compact result prepared from another/changed/incomplete input stream.
struct MapInput<'a> {
    input: &'a mut dyn MapEditCursor,
    priors: &'a [EntryMap],
    plans: &'a [Plan],
    root: usize,
    old_slot: usize,
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
                .is_some_and(|plan| plan.small.is_some())
            {
                if has_delta {
                    let _edit = self.take(budget)?;
                    continue;
                }
                self.close()?;
                continue;
            }
            let Some(small) = prior.inline_map() else {
                if has_delta {
                    return self.take(budget).map(Some);
                }
                self.close()?;
                continue;
            };
            let old = small.entry(self.old_slot)?;
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
                    IndexError::BackendViolation("inline base entry missing".into())
                })?;
                self.old_slot = self
                    .old_slot
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                return OrdinalMapEdit::copy_object_key(root, key, None, Some(value), budget)
                    .map(Some);
            }
            let expected = if order == std::cmp::Ordering::Equal {
                old.map(|(_, value)| value)
            } else {
                None
            };
            let edit = self.take(budget)?;
            if edit.edit.expected != expected.map(Into::into) {
                return Err(IndexError::Conflict);
            }
            if order == std::cmp::Ordering::Equal {
                self.old_slot = self
                    .old_slot
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            if let Some(value) = edit
                .edit
                .next
                .map(quanta_index_core::index::MapValue::object_ref)
                .transpose()?
            {
                return OrdinalMapEdit::copy_object_key(
                    root,
                    &edit.edit.key,
                    None,
                    Some(value),
                    budget,
                )
                .map(Some);
            }
        }
    }
}

pub(super) fn apply<C: MapEditCursor>(
    priors: &[EntryMap],
    counts: &[u64],
    input: &mut dyn MapEditCursor,
    mut root_cursor: impl FnMut(usize) -> Result<C, IndexError>,
    context: &mut AttemptContext<'_>,
) -> Result<PreparedRows<EntryMap>, IndexError> {
    if priors.is_empty() || priors.len() > 64 || counts.len() != priors.len() {
        return Err(IndexError::InvalidInput(
            "entry-map batch count differs".into(),
        ));
    }
    let mut plans = PreparedRows::new(priors.len())?;
    let mut bases = PreparedRows::new(priors.len())?;
    let mut result = PreparedRows::new(priors.len())?;
    // Admit every retained slot before reading a base or creating any map.
    for prior in priors {
        prior.validate()?;
        plans.push(
            Plan {
                small: None,
                stamp: None,
            },
            context.budget,
        )?;
        bases.push(prior.tree_base(), context.budget)?;
        result.push(EntryMap::default(), context.budget)?;
    }
    for (slot, (prior, count)) in priors.iter().zip(counts).enumerate() {
        if *count <= u64::try_from(INLINE_ENTRIES).map_err(|_error| IndexError::ResourceLimited)? {
            let plan = resolve_small(prior, *count, &mut root_cursor(slot)?, context)?;
            if plan.small.is_some() {
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
    let needs_tree = plans.as_slice().iter().any(|plan| plan.small.is_none());
    let mut checked = MapInput {
        input,
        priors,
        plans: plans.as_slice(),
        root: 0,
        old_slot: 0,
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
    for (slot, ((plan, count), output)) in plans
        .as_slice()
        .iter()
        .zip(counts)
        .zip(result.as_mut_slice())
        .enumerate()
    {
        *output = if let Some(small) = plan.small {
            EntryMap::inline(&small)
        } else {
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
        };
    }
    Ok(result)
}
