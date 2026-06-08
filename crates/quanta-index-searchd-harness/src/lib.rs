//! Dev/test/bench support harness for `quanta-index-searchd-runtime`.
//!
//! This crate is consumed **only as a dev-dependency** (by the runtime's
//! e2e tests and by the Layer-3 DSL latency benchmarks). It never enters
//! the production / daemon build graph, so it does not move the
//! `daemon-lane` compile-timing baseline.
//!
//! It owns three things:
//!
//! - [`harness`] — the reusable tempdir-backed runtime harness (`E2eRuntime`)
//!   that boots a real searchd driver over a UDS frontdoor, ingests typed
//!   batches, seals + activates generations, and issues query IPC requests.
//!   Promoted here from the runtime crate's test-private `common/` tree so a
//!   single source of truth is shared by tests, benches, and the cold runner.
//! - [`artifact`] + [`scenarios`] — the bench-owned scenario authority and the
//!   machine-readable latency-artifact model (RFC-DSL-Benchmarking, Layer 3).
//! - [`bench_support`] — the glue that prepares a runtime for a scenario and
//!   runs one scenario query, mapping the harness result onto an artifact row.

mod harness;

pub mod ambiguity;
pub mod artifact;
pub mod bench_support;
pub mod ops;
pub mod relevance;
pub mod scale;
pub mod scenarios;
pub mod snippet;
pub mod tail;

pub use harness::*;
