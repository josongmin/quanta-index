//! Bounded positions into the authenticated install group.
//!
//! This is not traversal or publication authority. Full references decide
//! equality; hashing selects a candidate only. The table dies with, and is
//! cleared after, its group.

use super::{IndexError, MapBuffer, ObjectRef, WorkBudget, WorkUsage};
use crate::object::{MAX_INSTALL_BATCH_INPUTS, PreparedObject};

const SLOTS: usize = (MAX_INSTALL_BATCH_INPUTS * 2).next_power_of_two();
const EMPTY: usize = usize::MAX;

pub(super) struct CompletedGroup {
    slots: MapBuffer<usize>,
}

impl CompletedGroup {
    pub(super) fn new() -> Self {
        Self {
            slots: MapBuffer::new(SLOTS),
        }
    }

    fn bucket(reference: ObjectRef) -> usize {
        let [first, second, ..] = (reference.0).0;
        usize::from(u16::from_le_bytes([first, second])) & (SLOTS - 1)
    }

    fn find(
        &self,
        reference: ObjectRef,
        objects: &[PreparedObject],
        budget: &mut WorkBudget,
    ) -> Result<(usize, bool), IndexError> {
        budget.charge(WorkUsage {
            bytes: 2,
            ..WorkUsage::default()
        })?;
        let mut slot = Self::bucket(reference);
        for _probe in 0..SLOTS {
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<usize>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let position = *self.slots.get(slot).ok_or_else(|| {
                IndexError::BackendViolation("completed group table is not initialized".into())
            })?;
            if position == EMPTY {
                return Ok((slot, false));
            }
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<ObjectRef>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let object = objects.get(position).ok_or_else(|| {
                IndexError::BackendViolation("completed group position escaped its group".into())
            })?;
            if object.registration.reference == reference {
                return Ok((slot, true));
            }
            slot = slot.wrapping_add(1) & (SLOTS - 1);
        }
        Err(IndexError::BackendViolation(
            "completed group exceeded its proved load bound".into(),
        ))
    }

    pub(super) fn contains(
        &self,
        reference: ObjectRef,
        objects: &[PreparedObject],
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        if self.slots.is_empty() {
            return Ok(false);
        }
        self.find(reference, objects, budget)
            .map(|(_, found)| found)
    }

    pub(super) fn insert(
        &mut self,
        reference: ObjectRef,
        position: usize,
        objects: &[PreparedObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            bytes: u64::try_from(std::mem::size_of::<ObjectRef>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        if objects.len() > MAX_INSTALL_BATCH_INPUTS
            || objects
                .get(position)
                .is_none_or(|object| object.registration.reference != reference)
        {
            return Err(IndexError::BackendViolation(
                "completed group insertion is not authenticated".into(),
            ));
        }
        if self.slots.is_empty() {
            budget.charge(WorkUsage {
                bytes: u64::try_from(
                    SLOTS
                        .checked_mul(std::mem::size_of::<usize>())
                        .and_then(|bytes| bytes.checked_mul(2))
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            self.slots = MapBuffer::with_capacity(SLOTS, SLOTS, budget)?;
            for _slot in 0..SLOTS {
                self.slots.push(EMPTY, budget)?;
            }
        }
        let (slot, found) = self.find(reference, objects, budget)?;
        if found {
            return Err(IndexError::BackendViolation(
                "duplicate completed map object".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(std::mem::size_of::<usize>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        *self
            .slots
            .get_mut(slot)
            .ok_or(IndexError::ResourceLimited)? = position;
        Ok(())
    }

    pub(super) fn clear(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            bytes: u64::try_from(
                self.slots
                    .len()
                    .checked_mul(std::mem::size_of::<usize>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.slots.fill(EMPTY);
        Ok(())
    }
}
