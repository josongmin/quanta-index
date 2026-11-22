//! History compatibility and Native publication readback integration.

#![forbid(unsafe_code)]

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[path = "e2e_auxiliary_catalog/history_native.rs"]
mod history_native;

#[path = "e2e_auxiliary_catalog/history_source_readback.rs"]
mod history_source_readback;
