//! Bounded structural owner-root diff for a sealed source mutation stream.

use super::{IndexError, MapRoot, OwnerMutation, WorkBudget, WorkUsage, owner_map_key};
use crate::index::{
    MapValue, MemoryLease, MemoryLifetime, owner_from_map_key,
    ports::{MapDiffRange, MapDiffVisitor, PersistentMapPort},
};

const PAGE_ROWS: usize = 64;

/// Retained rows precede the allocation grant so they are destroyed first.
pub(super) struct SourceDiffPages<'a> {
    maps: &'a dyn PersistentMapPort,
    before: MapRoot,
    after: MapRoot,
    rows: Vec<OwnerMutation>,
    offset: usize,
    last: Option<[u8; 38]>,
    more: bool,
    loaded: bool,
    memory: Option<MemoryLease>,
}

impl<'a> SourceDiffPages<'a> {
    pub(super) fn new(before: MapRoot, after: MapRoot, maps: &'a dyn PersistentMapPort) -> Self {
        Self {
            maps,
            before,
            after,
            rows: Vec::new(),
            offset: 0,
            last: None,
            more: true,
            loaded: false,
            memory: None,
        }
    }

    pub(super) fn peek(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<OwnerMutation>, IndexError> {
        budget.checkpoint()?;
        // Identical immutable roots have no structural changes. Avoid both
        // backing admission and a backend capability call for this case.
        if self.before == self.after {
            return Ok(None);
        }
        if self.offset < self.rows.len() {
            return Ok(self.rows.get(self.offset).cloned());
        }
        if self.loaded && !self.more {
            return Ok(None);
        }
        self.load(budget)?;
        Ok(self.rows.first().cloned())
    }

    pub(super) fn advance(&mut self) -> Result<(), IndexError> {
        if self.offset >= self.rows.len() {
            return Err(IndexError::BackendViolation(
                "source root diff advanced without a cached row".into(),
            ));
        }
        self.offset = self
            .offset
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }

    fn load(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if self.rows.capacity() == 0 {
            if self.memory.is_some() {
                return Err(IndexError::BackendViolation(
                    "source root diff grant outlived its backing".into(),
                ));
            }
            let requested = PAGE_ROWS
                .checked_mul(std::mem::size_of::<OwnerMutation>())
                .ok_or(IndexError::ResourceLimited)?;
            let bytes = u64::try_from(requested).map_err(|_error| IndexError::ResourceLimited)?;
            let memory = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
            self.rows
                .try_reserve_exact(PAGE_ROWS)
                .map_err(|_error| IndexError::ResourceLimited)?;
            self.memory = memory;
        }
        self.rows.clear();
        self.offset = 0;
        let (more, previous) = {
            let mut visitor = DiffRows {
                rows: &mut self.rows,
                previous: self.last,
            };
            let more = self.maps.visit_diff(
                self.before,
                self.after,
                MapDiffRange {
                    prefix: &[],
                    after: self.last.as_ref().map(<[u8; 38]>::as_slice),
                    limit: u32::try_from(PAGE_ROWS)
                        .map_err(|_error| IndexError::ResourceLimited)?,
                },
                &mut visitor,
                budget,
            )?;
            (more, visitor.previous)
        };
        if more && self.rows.is_empty() {
            return Err(IndexError::BackendViolation(
                "source root diff returned an empty continuation page".into(),
            ));
        }
        self.last = previous;
        self.more = more;
        self.loaded = true;
        Ok(())
    }
}

struct DiffRows<'a> {
    rows: &'a mut Vec<OwnerMutation>,
    previous: Option<[u8; 38]>,
}

impl MapDiffVisitor for DiffRows<'_> {
    fn changed(
        &mut self,
        key: &[u8],
        expected: Option<MapValue>,
        next: Option<MapValue>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let owner = owner_from_map_key(key)?;
        let canonical = owner_map_key(owner);
        if key != canonical.as_slice()
            || self.previous.is_some_and(|previous| canonical <= previous)
            || self.rows.len() == PAGE_ROWS
            || expected == next
        {
            return Err(IndexError::BackendViolation(
                "source root diff escaped its ordered page".into(),
            ));
        }
        let expected = expected.map(MapValue::object_ref).transpose()?;
        let next = next.map(MapValue::object_ref).transpose()?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(std::mem::size_of::<OwnerMutation>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        if self.rows.len() == self.rows.capacity() {
            return Err(IndexError::BackendViolation(
                "source root diff row backing was not admitted".into(),
            ));
        }
        self.rows.push(OwnerMutation {
            owner,
            expected,
            next,
        });
        self.previous = Some(canonical);
        Ok(())
    }
}
