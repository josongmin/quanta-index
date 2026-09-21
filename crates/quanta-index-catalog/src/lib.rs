//! `SQLite`-backed durable catalog for the search plane (W2, G0-C).
//!
//! One file, one connection, two tables families:
//! - the ingest idempotency record (QI-BB-032): one row per
//!   `(operation kind, repo, revision, generation, batch_digest)` carrying
//!   the canonical body hash, the apply state and the receipt of the apply
//!   — contract [`quanta_index_core::IdempotencyCatalogPort`];
//! - the auxiliary authority rows (QI-BB-020): one row per history,
//!   runtime-metadata and structural record, applied per batch as one
//!   transaction — contract [`quanta_index_core::AuxiliaryAuthorityCatalogPort`].
//!
//! This crate is the only place the storage engine is named. The G0-C
//! conditions bind here:
//! - every row carries its own digest over its content, verified on read,
//!   because the engine serves bit-rotted cells with a clean
//!   `integrity_check`;
//! - authority rows commit under `synchronous=FULL` (and `fullfsync=ON`
//!   where the platform honors it), never `NORMAL`;
//! - a write that meets a held lock past the busy budget is a typed
//!   `CATALOG_BUSY`, not a blocked thread.

#![forbid(unsafe_code)]

mod auxiliary;
mod candidate;
mod connection;
mod idempotency;
mod open;
mod sequence;

pub use candidate::{
    ActivationOutcomeV1, RepoMapActivationRowV1, RepoMapCandidateRowV1, RepoMapCandidateStateV1,
    RepoMapQuarantineIncidentRowV1, SealOutcomeV1,
};
pub use connection::{CATALOG_FILE_NAME, SqliteCatalog, catalog_dir};
