//! Mock `LqQueryNormalizer` + `ConformanceExecutor` for self-tests.
//!
//! Behind a `test-utils` feature for downstream test consumption.
//! Internally always compiled under `#[cfg(test)]` so the runner's
//! own unit tests can use them.

mod impls;

pub use impls::{MockError, MockExecutor, MockNormalizer, MockResponse};
