//! Channel backends.
//!
//! Public surface inhabitants must use the trait/factory re-exports at the
//! crate root. Direct imports from this module by downstream crates are denied
//! by `tools/ci/lint/lint-hexagonal-boundaries.py`.

pub mod wal_mmap;
