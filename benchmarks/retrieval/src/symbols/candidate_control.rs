//! Extraction limits reused when the producer is compiled in the isolated
//! registry-grammar process without benchmark corpus admission.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use super::SymbolExtractError;

pub(super) struct ExtractionControl<'a> {
    pub path: &'a str,
    pub deadline: Instant,
    pub cancellation: Option<&'a AtomicBool>,
    pub max_symbols: usize,
}

impl ExtractionControl<'_> {
    pub(super) fn check(&self) -> Result<(), SymbolExtractError> {
        if self
            .cancellation
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            return Err(SymbolExtractError::Cancelled {
                path: self.path.to_string(),
            });
        }
        if Instant::now() >= self.deadline {
            return Err(SymbolExtractError::TimedOut {
                path: self.path.to_string(),
            });
        }
        Ok(())
    }

    pub(super) fn check_symbols(&self, count: usize) -> Result<(), SymbolExtractError> {
        self.check()?;
        if count >= self.max_symbols {
            return Err(SymbolExtractError::ResourceLimit {
                path: self.path.to_string(),
            });
        }
        Ok(())
    }
}
