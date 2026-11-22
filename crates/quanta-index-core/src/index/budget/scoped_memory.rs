use std::ops::{Deref, DerefMut};

use super::WorkBudget;
use crate::index::{MemoryCredit, MemoryScope};

/// Restores the parent's allocation scope before closing unused credit.
pub struct ScopedMemoryBudget<'a> {
    pub(super) budget: &'a mut WorkBudget,
    pub(super) previous: Option<MemoryScope>,
    pub(super) previous_optional: Option<MemoryScope>,
    pub(super) _credit: MemoryCredit,
}

impl Deref for ScopedMemoryBudget<'_> {
    type Target = WorkBudget;

    fn deref(&self) -> &Self::Target {
        self.budget
    }
}

impl DerefMut for ScopedMemoryBudget<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.budget
    }
}

impl Drop for ScopedMemoryBudget<'_> {
    fn drop(&mut self) {
        self.budget
            .restore_scoped_memory_credit(self.previous.take(), self.previous_optional.take());
    }
}
