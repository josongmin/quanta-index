//! Required post-child retirement and conservative root custody on failure.

use std::sync::Arc;

use quanta_index_core::CoreError;

use super::StateRootLease;

#[cfg(test)]
mod tests;

/// Composition-owned physical retirement after all consumers have stopped.
pub trait RuntimeRetirementPort: Send + Sync {
    /// Close the original Catalog and retire the same physical authority.
    /// Failure must retain its original resources for conservative custody.
    fn retire(&self) -> Result<(), CoreError>;
}

pub(super) struct RuntimeRetirementGuard {
    port: Arc<dyn RuntimeRetirementPort>,
    lease: StateRootLease,
    completed: bool,
    attempted: bool,
}

impl RuntimeRetirementGuard {
    pub(super) fn new(port: Arc<dyn RuntimeRetirementPort>, lease: StateRootLease) -> Self {
        Self {
            port,
            lease,
            completed: false,
            attempted: false,
        }
    }

    pub(super) fn retire_checked(&mut self) -> Result<(), CoreError> {
        if self.completed {
            return Ok(());
        }
        self.attempted = true;
        let result =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.port.retire())) {
                Ok(result) => result,
                Err(_panic) => Err(CoreError::Storage("runtime retirement panicked".into())),
            };
        self.completed = result.is_ok();
        result
    }
}

impl Drop for RuntimeRetirementGuard {
    fn drop(&mut self) {
        if !self.attempted {
            let _retirement = self.retire_checked();
        }
        if !self.completed {
            // Process quarantine: neither a failed close nor a panic authorizes
            // dropping the original resources and surrendering the leased root.
            std::mem::forget((Arc::clone(&self.port), self.lease.clone()));
        }
    }
}
