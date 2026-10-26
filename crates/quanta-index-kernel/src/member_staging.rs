//! Bounded, admitted preparation of profile lookup and member map edits.

use quanta_index_core::index::{
    IndexError, IndexInstanceId, InstanceSpec, MapEdit, MemoryLease, MemoryLifetime, ObjectRef,
    WorkBudget,
};

use crate::profile::MAX_PROFILE_INSTANCES;

pub(crate) struct ProfileIndex<'a> {
    order: Vec<&'a InstanceSpec>,
    _memory: Option<MemoryLease>,
}

impl<'a> ProfileIndex<'a> {
    pub(crate) fn new(
        instances: &'a [InstanceSpec],
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        if instances.len() > MAX_PROFILE_INSTANCES {
            return Err(IndexError::ResourceLimited);
        }
        let memory = reserve_array::<&InstanceSpec>(instances.len(), budget)?;
        let mut order = Vec::new();
        order
            .try_reserve_exact(instances.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        order.extend(instances.iter());
        order.sort_unstable_by_key(|instance| instance.instance);
        if order
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left.instance == right.instance))
        {
            return Err(IndexError::InvalidInput(
                "duplicate profile index instance".into(),
            ));
        }
        budget.checkpoint()?;
        Ok(Self {
            order,
            _memory: memory,
        })
    }

    pub(crate) fn get(&self, instance: &IndexInstanceId) -> Option<&&'a InstanceSpec> {
        match self
            .order
            .binary_search_by_key(instance, |spec| spec.instance)
        {
            Ok(index) => self.order.get(index),
            Err(_insertion_index) => None,
        }
    }

    pub(crate) fn union<'b>(
        &'b self,
        other: &'b Self,
    ) -> impl Iterator<Item = IndexInstanceId> + 'b {
        let mut left = self.order.iter().peekable();
        let mut right = other.order.iter().peekable();
        std::iter::from_fn(move || match (left.peek(), right.peek()) {
            (Some(first), Some(second)) => match first.instance.cmp(&second.instance) {
                std::cmp::Ordering::Less => left.next().map(|spec| spec.instance),
                std::cmp::Ordering::Greater => right.next().map(|spec| spec.instance),
                std::cmp::Ordering::Equal => {
                    let _same = right.next();
                    left.next().map(|spec| spec.instance)
                }
            },
            (Some(_), None) => left.next().map(|spec| spec.instance),
            (None, Some(_)) => right.next().map(|spec| spec.instance),
            (None, None) => None,
        })
    }
}

fn reserve_array<T>(count: usize, budget: &WorkBudget) -> Result<Option<MemoryLease>, IndexError> {
    let bytes = count
        .checked_mul(std::mem::size_of::<T>())
        .ok_or(IndexError::ResourceLimited)?;
    reserve_bytes(bytes, budget)
}

fn reserve_bytes(bytes: usize, budget: &WorkBudget) -> Result<Option<MemoryLease>, IndexError> {
    if bytes == 0 {
        budget.checkpoint()?;
        Ok(None)
    } else {
        budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )
    }
}

const INITIAL_EDIT_CAPACITY: usize = 64;

/// A synchronous port input owns only its actual temporary key backing.
pub(crate) struct PreparedEdit {
    edit: MapEdit,
    _memory: Option<MemoryLease>,
}

impl PreparedEdit {
    pub(crate) fn new(
        key: &[u8],
        expected: Option<ObjectRef>,
        next: Option<ObjectRef>,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        Self::values(key, expected.map(Into::into), next.map(Into::into), budget)
    }

    pub(crate) fn values(
        key: &[u8],
        expected: Option<quanta_index_core::index::MapValue>,
        next: Option<quanta_index_core::index::MapValue>,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        let memory = reserve_bytes(key.len(), budget)?;
        let mut copied = Vec::new();
        copied
            .try_reserve_exact(key.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        copied.extend_from_slice(key);
        let prepared = Self {
            edit: MapEdit {
                key: copied,
                expected,
                next,
            },
            _memory: memory,
        };
        budget.checkpoint()?;
        Ok(prepared)
    }

    pub(crate) fn as_edit(&self) -> &MapEdit {
        &self.edit
    }

    pub(crate) fn as_slice(&self) -> &[MapEdit] {
        std::slice::from_ref(&self.edit)
    }
}

pub(crate) struct MemberEdits {
    edits: Vec<MapEdit>,
    // Keys die with edits before their individual grants.
    key_memory: Vec<Option<MemoryLease>>,
    limit: usize,
    capacity: usize,
    // Both arrays die before their backing reservation.
    array_memory: Option<MemoryLease>,
}

impl MemberEdits {
    pub(crate) fn new(limit: usize, budget: &WorkBudget) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        if limit > MAX_PROFILE_INSTANCES.saturating_mul(2) {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            edits: Vec::new(),
            key_memory: Vec::new(),
            limit,
            capacity: 0,
            array_memory: None,
        })
    }

    fn grow(&mut self, budget: &WorkBudget) -> Result<(), IndexError> {
        let next = if self.capacity == 0 {
            INITIAL_EDIT_CAPACITY.min(self.limit)
        } else {
            self.capacity
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?
                .min(self.limit)
        };
        if next <= self.capacity {
            return Err(IndexError::ResourceLimited);
        }
        let slots = std::mem::size_of::<MapEdit>()
            .checked_add(std::mem::size_of::<Option<MemoryLease>>())
            .and_then(|bytes| bytes.checked_mul(next))
            .ok_or(IndexError::ResourceLimited)?;
        // Old and new backing coexist until both allocations have succeeded.
        // A failed allocation leaves the previously admitted arrays intact.
        let memory = reserve_bytes(slots, budget)?;
        let mut edits = Vec::new();
        edits
            .try_reserve_exact(next)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut key_memory = Vec::new();
        key_memory
            .try_reserve_exact(next)
            .map_err(|_error| IndexError::ResourceLimited)?;
        budget.checkpoint()?;
        edits.append(&mut self.edits);
        key_memory.append(&mut self.key_memory);
        let old_edits = std::mem::replace(&mut self.edits, edits);
        let old_keys = std::mem::replace(&mut self.key_memory, key_memory);
        drop(old_edits);
        drop(old_keys);
        self.array_memory = memory;
        self.capacity = next;
        Ok(())
    }

    pub(crate) fn push(
        &mut self,
        key: &[u8; 23],
        expected: Option<ObjectRef>,
        next: Option<ObjectRef>,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if self.edits.len() == self.limit {
            return Err(IndexError::ResourceLimited);
        }
        if self.edits.len() == self.capacity {
            self.grow(budget)?;
        }
        let memory = reserve_bytes(key.len(), budget)?;
        let mut copied = Vec::new();
        copied
            .try_reserve_exact(key.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        copied.extend_from_slice(key);
        budget.checkpoint()?;
        self.edits.push(MapEdit {
            key: copied,
            expected: expected.map(Into::into),
            next: next.map(Into::into),
        });
        self.key_memory.push(memory);
        Ok(())
    }

    pub(crate) fn sort(&mut self) {
        self.edits
            .sort_unstable_by(|left, right| left.key.cmp(&right.key));
    }

    pub(crate) fn as_slice(&self) -> &[MapEdit] {
        &self.edits
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::staging_payload::tests::{require, scoped_budget};
    use quanta_index_core::index::{
        Digest, GroupActivation, IndexKind, InputDomain, PublicationGroupId, SchemaId,
        StorageFormatId, member_map_key,
    };
    use std::collections::{BTreeMap, BTreeSet};

    fn spec(byte: u8) -> InstanceSpec {
        InstanceSpec {
            instance: IndexInstanceId([byte; 16]),
            kind: IndexKind([1; 16]),
            group: PublicationGroupId([1; 16]),
            activation: GroupActivation::CoreRequired,
            input_domain: InputDomain::SourceLocal,
            schema: SchemaId([1; 16]),
            policy: Digest([1; 32]),
            format: StorageFormatId([1; 16]),
        }
    }

    #[test]
    fn single_edit_key_floor_predicates_and_lifetime_use_original_credit() -> Result<(), IndexError>
    {
        let key = [7; 32];
        let expected = Some(ObjectRef(Digest([2; 32])));
        let next = Some(ObjectRef(Digest([3; 32])));
        let (denied, denied_ledger) = scoped_budget(31)?;
        require(matches!(
            PreparedEdit::new(&key, expected, next, &denied),
            Err(IndexError::ResourceLimited)
        ))?;
        require(denied_ledger.live() == 0)?;
        let (budget, ledger) = scoped_budget(32)?;
        let prepared = PreparedEdit::new(&key, expected, next, &budget)?;
        require(prepared.as_edit().key.as_slice() == key)?;
        require(prepared.as_edit().expected == expected.map(Into::into) && prepared.as_edit().next == next.map(Into::into))?;
        require(prepared.as_slice().len() == 1 && ledger.live() == 32)?;
        drop(budget);
        require(ledger.live() == 32)?;
        drop(prepared);
        require(ledger.live() == 0)?;
        let (empty_budget, empty_ledger) = scoped_budget(0)?;
        let empty = PreparedEdit::new(&[], None, None, &empty_budget)?;
        require(empty.as_edit().key.is_empty() && empty_ledger.live() == 0)?;
        drop(empty);
        empty_budget.request().cancel_handle().cancel();
        require(matches!(
            PreparedEdit::new(&key, expected, next, &empty_budget),
            Err(IndexError::Cancelled)
        ))?;
        require(empty_ledger.live() == 0)?;
        Ok(())
    }

    #[test]
    fn profile_lookup_and_union_match_old_trees_without_copying_specs() -> Result<(), IndexError> {
        let left = [spec(9), spec(1), spec(4)];
        let right = [spec(2), spec(9), spec(3)];
        let (budget, ledger) = scoped_budget(u64::MAX)?;
        let first = ProfileIndex::new(&left, &budget)?;
        let second = ProfileIndex::new(&right, &budget)?;
        let old_left: BTreeMap<_, _> = left.iter().map(|spec| (spec.instance, spec)).collect();
        let old_right: BTreeMap<_, _> = right.iter().map(|spec| (spec.instance, spec)).collect();
        let gold: BTreeSet<_> = old_left.keys().chain(old_right.keys()).copied().collect();
        require(first.union(&second).eq(gold))?;
        for byte in 0..=10 {
            let id = IndexInstanceId([byte; 16]);
            require(first.get(&id) == old_left.get(&id) && second.get(&id) == old_right.get(&id))?;
        }
        let floor =
            u64::try_from((left.len() + right.len()) * std::mem::size_of::<&InstanceSpec>())
                .map_err(|_error| IndexError::ResourceLimited)?;
        require(ledger.live() == floor)?;
        drop(first);
        drop(second);
        require(ledger.live() == 0)?;
        require(matches!(
            ProfileIndex::new(&[spec(1), spec(1)], &budget),
            Err(IndexError::InvalidInput(_))
        ))?;
        require(ledger.live() == 0)?;
        let (denied, ledger) = scoped_budget(0)?;
        require(matches!(
            ProfileIndex::new(&left, &denied),
            Err(IndexError::ResourceLimited)
        ))?;
        let empty = ProfileIndex::new(&[], &denied)?;
        require(empty.get(&IndexInstanceId([1; 16])).is_none() && ledger.live() == 0)?;
        Ok(())
    }

    #[test]
    fn lazy_member_edits_preserve_wire_keys_and_refuse_key_floor() -> Result<(), IndexError> {
        let slots = std::mem::size_of::<MapEdit>() + std::mem::size_of::<Option<MemoryLease>>();
        let (budget, ledger) =
            scoped_budget(u64::try_from(slots).map_err(|_error| IndexError::ResourceLimited)?)?;
        let mut edits = MemberEdits::new(1, &budget)?;
        require(edits.is_empty() && ledger.live() == 0)?;
        let key = member_map_key(IndexInstanceId([1; 16]));
        require(matches!(
            edits.push(&key, None, None, &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(
            edits.is_empty()
                && ledger.live()
                    == u64::try_from(slots).map_err(|_error| IndexError::ResourceLimited)?,
        )?;
        drop(edits);
        require(ledger.live() == 0)?;
        let (budget, ledger) = scoped_budget(u64::MAX)?;
        let mut edits = MemberEdits::new(2, &budget)?;
        let next = Some(ObjectRef(Digest([9; 32])));
        edits.push(
            &member_map_key(IndexInstanceId([4; 16])),
            None,
            next,
            &budget,
        )?;
        edits.push(&key, next, None, &budget)?;
        edits.sort();
        let gold = [
            MapEdit {
                key: key.to_vec(),
                expected: (next).map(Into::into),
                next: None,
            },
            MapEdit {
                key: member_map_key(IndexInstanceId([4; 16])).to_vec(),
                expected: None,
                next: next.map(Into::into),
            },
        ];
        require(edits.as_slice() == gold)?;
        let live = ledger.live();
        require(matches!(
            edits.push(&key, None, None, &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(ledger.live() == live && edits.as_slice() == gold)?;
        drop(edits);
        require(ledger.live() == 0)?;
        let (budget, ledger) = scoped_budget(u64::MAX)?;
        let mut edits = MemberEdits::new(65, &budget)?;
        for byte in 0..65 {
            edits.push(
                &member_map_key(IndexInstanceId([byte; 16])),
                None,
                Some(ObjectRef(Digest([byte; 32]))),
                &budget,
            )?;
        }
        let floor = slots
            .checked_mul(65)
            .and_then(|bytes| bytes.checked_add(65 * 23))
            .ok_or(IndexError::ResourceLimited)?;
        require(
            edits.as_slice().len() == 65
                && ledger.live()
                    == u64::try_from(floor).map_err(|_error| IndexError::ResourceLimited)?,
        )?;
        budget.request().cancel_handle().cancel();
        require(matches!(
            edits.push(
                &member_map_key(IndexInstanceId([66; 16])),
                None,
                None,
                &budget
            ),
            Err(IndexError::Cancelled)
        ))?;
        drop(edits);
        require(ledger.live() == 0)
    }

    #[test]
    fn failed_sixty_fifth_growth_keeps_old_rows_and_refunds_new_backing() -> Result<(), IndexError>
    {
        let slots = std::mem::size_of::<MapEdit>() + std::mem::size_of::<Option<MemoryLease>>();
        let cap = slots
            .checked_mul(129)
            .and_then(|bytes| bytes.checked_add(64 * 23))
            .and_then(|bytes| bytes.checked_sub(1))
            .ok_or(IndexError::ResourceLimited)?;
        let (budget, ledger) =
            scoped_budget(u64::try_from(cap).map_err(|_error| IndexError::ResourceLimited)?)?;
        let mut edits = MemberEdits::new(65, &budget)?;
        for byte in 0..64 {
            edits.push(
                &member_map_key(IndexInstanceId([byte; 16])),
                None,
                Some(ObjectRef(Digest([byte; 32]))),
                &budget,
            )?;
        }
        let live = ledger.live();
        require(matches!(
            edits.push(
                &member_map_key(IndexInstanceId([64; 16])),
                None,
                None,
                &budget
            ),
            Err(IndexError::ResourceLimited)
        ))?;
        require(edits.as_slice().len() == 64 && ledger.live() == live)?;
        for (row, byte) in edits.as_slice().iter().zip(0..64) {
            require(
                row.key == member_map_key(IndexInstanceId([byte; 16]))
                    && row.next == Some(ObjectRef(Digest([byte; 32])).into()),
            )?;
        }
        drop(edits);
        require(ledger.live() == 0)
    }
}
