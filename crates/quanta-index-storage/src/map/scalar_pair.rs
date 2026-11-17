//! Two independent scalar edits reuse bounded resident COW staging. The complete
//! stream and both original-base CAS checks precede any immutable installation.

use super::{
    FileObjectStore, IndexError, MapEdit, MapEditCursor, MapRoot, MapValue, ObjectWriteSink, OrdinalMapEdit,
    ResidentStage, StagedMapRoots, WorkBudget, WorkUsage, charge_key_allocation, key_nibbles,
    resident,
};
use quanta_index_core::index::MapRootBuilder;
use quanta_index_core::index::ports::RootPackingPlan;

pub(super) trait ResidentEditRef {
    fn key(&self) -> &[u8];
    fn expected_value(&self) -> Result<Option<MapValue>, IndexError>;
    fn next_value(&self) -> Result<Option<MapValue>, IndexError>;
}

impl ResidentEditRef for MapEdit {
    fn key(&self) -> &[u8] { &self.key }
    fn expected_value(&self) -> Result<Option<MapValue>, IndexError> { Ok(self.expected) }
    fn next_value(&self) -> Result<Option<MapValue>, IndexError> { Ok(self.next) }
}

impl ResidentEditRef for OrdinalMapEdit {
    fn key(&self) -> &[u8] { OrdinalMapEdit::key(self) }
    fn expected_value(&self) -> Result<Option<MapValue>, IndexError> { self.expected() }
    fn next_value(&self) -> Result<Option<MapValue>, IndexError> { self.next() }
}

pub(super) struct Input<'a> {
    inner: &'a mut dyn MapEditCursor,
    prefix: [Option<OrdinalMapEdit>; 3],
    next: usize,
    finished: bool,
}

impl<'a> Input<'a> {
    pub(super) fn new(
        inner: &'a mut dyn MapEditCursor,
        probe: bool,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let mut input = Self {
            inner,
            prefix: [None, None, None],
            next: 0,
            finished: false,
        };
        if probe {
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<[Option<OrdinalMapEdit>; 3]>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            for slot in &mut input.prefix {
                budget.checkpoint()?;
                *slot = input.inner.next_edit(budget)?;
                if slot.is_none() {
                    input.finished = true;
                    break;
                }
            }
        }
        Ok(input)
    }

    pub(super) fn pair(&self) -> Result<Option<[&OrdinalMapEdit; 2]>, IndexError> {
        let [Some(first), Some(second), None] = &self.prefix else {
            return Ok(None);
        };
        Ok((self.finished
            && first.root() == 0
            && second.root() == 1
            && first.key().len() <= resident::MAX_SINGLE_KEY_BYTES
            && second.key().len() <= resident::MAX_SINGLE_KEY_BYTES)
            .then_some([first, second]))
    }
}

impl MapEditCursor for Input<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        if let Some(slot) = self.prefix.get_mut(self.next) {
            self.next = self
                .next
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if let Some(edit) = slot.take() {
                return Ok(Some(edit));
            }
        }
        if self.finished {
            Ok(None)
        } else {
            self.inner.next_edit(budget)
        }
    }
}

impl FileObjectStore {
    pub(super) fn apply_resident_pair(
        &self,
        bases: &[MapRoot],
        edits: [&OrdinalMapEdit; 2],
        roots: MapRootBuilder,
        packing: RootPackingPlan,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        let staged = ResidentStage::pair(packing);
        let roots = self.prepare_resident_roots(bases, &edits, roots, &staged, budget)?;
        self.install_reachable_map_nodes(roots.as_slice(), &staged, sink, budget)?;
        budget.checkpoint()?;
        Ok(roots)
    }

    pub(super) fn prepare_resident_roots<E: ResidentEditRef>(
        &self,
        bases: &[MapRoot],
        edits: &[&E],
        mut roots: MapRootBuilder,
        staged: &ResidentStage,
        budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        if bases.is_empty()
            || bases.len() > 2
            || bases.len() != edits.len()
            || edits
                .iter()
                .any(|edit| edit.key().len() > resident::MAX_SINGLE_KEY_BYTES)
        {
            return Err(IndexError::ResourceLimited);
        }
        let mut keys = [None, None];
        for (slot, edit) in edits.iter().enumerate() {
            budget.checkpoint()?;
            budget.charge(WorkUsage {
                mutations: 1,
                bytes: u64::try_from(edit.key().len())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            charge_key_allocation(edit.key(), budget)?;
            let key = key_nibbles(edit.key(), budget)?;
            *keys.get_mut(slot).ok_or(IndexError::ResourceLimited)? = Some(key);
        }
        let mut built = [MapRoot(None); 2];
        let shared = if staged.shares_pair_layout() && bases.first() == bases.get(1) {
            match (edits.first(), edits.get(1)) {
                (Some(first), Some(second)) => first.key() == second.key()
                    && first.expected_value()? == second.expected_value()?
                    && first.next_value()? == second.next_value()?,
                _ => false,
            }
        } else {
            false
        };
        // Finish both bounded DAGs before the first physical reservation. A
        // failure in the second build cannot install a successful first prefix.
        for (slot, (base, edit)) in bases.iter().zip(edits.iter().copied()).enumerate() {
            budget.checkpoint()?;
            staged.select_root(slot);
            let root = if slot == 1 && shared {
                *built.first().ok_or(IndexError::ResourceLimited)?
            } else if edit.expected_value()? == edit.next_value()? {
                let key = keys.get(slot).and_then(Option::as_ref).ok_or_else(|| {
                    IndexError::BackendViolation("scalar pair lost a checked key".into())
                })?;
                if self.lookup_inner(*base, key, budget)? != edit.expected_value()? {
                    return Err(IndexError::Conflict);
                }
                *base
            } else {
                let key = keys.get(slot).and_then(Option::as_ref).ok_or_else(|| {
                    IndexError::BackendViolation("scalar pair lost a checked key".into())
                })?;
                self.update_one_checked(*base, key, edit.expected_value()?, edit.next_value()?, staged, budget)?
            };
            *built.get_mut(slot).ok_or(IndexError::ResourceLimited)? = root;
            roots.push(root)?;
        }
        roots.finish()
    }
}
