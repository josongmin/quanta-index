//! Compile-gated process-test observation at the lexical selected/view seam.
//!
//! The hook has no wire, environment or product configuration entry point.
//! Only a runtime test explicitly installing a gate can make it wait.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use quanta_index_contract::GenerationPin;
use quanta_index_core::CoreError;

type LexicalGate = dyn Fn(&GenerationPin) -> Result<(), CoreError> + Send + Sync + 'static;

static LEXICAL_ACTIVE_BEFORE_VIEW: Mutex<Option<Arc<LexicalGate>>> = Mutex::new(None);
static LEXICAL_VIEW_ACQUIRE_ATTEMPTS: AtomicU64 = AtomicU64::new(0);

/// Keep this guard for the test child's lifetime. Dropping it removes the
/// installed callback so another test in the process cannot inherit it.
pub struct LexicalActiveBeforeViewGuard;

impl Drop for LexicalActiveBeforeViewGuard {
    fn drop(&mut self) {
        if let Ok(mut gate) = LEXICAL_ACTIVE_BEFORE_VIEW.lock() {
            *gate = None;
        }
    }
}

pub fn install_lexical_active_before_view(
    gate: impl Fn(&GenerationPin) -> Result<(), CoreError> + Send + Sync + 'static,
) -> Result<LexicalActiveBeforeViewGuard, CoreError> {
    let mut installed = LEXICAL_ACTIVE_BEFORE_VIEW
        .lock()
        .map_err(|error| CoreError::Storage(format!("lexical test gate poisoned: {error}")))?;
    if installed.is_some() {
        return Err(CoreError::InvalidContract(
            "lexical test gate already installed".into(),
        ));
    }
    *installed = Some(Arc::new(gate));
    Ok(LexicalActiveBeforeViewGuard)
}

pub(crate) fn lexical_active_selected_before_view(pin: &GenerationPin) -> Result<(), CoreError> {
    let gate = LEXICAL_ACTIVE_BEFORE_VIEW
        .lock()
        .map_err(|error| CoreError::Storage(format!("lexical test gate poisoned: {error}")))?
        .clone();
    if let Some(gate) = gate {
        gate(pin)?;
    }
    Ok(())
}

pub(crate) fn record_lexical_view_acquire_attempt() {
    let _previous = LEXICAL_VIEW_ACQUIRE_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
}

pub fn lexical_view_acquire_attempts() -> u64 {
    LEXICAL_VIEW_ACQUIRE_ATTEMPTS.load(Ordering::SeqCst)
}
