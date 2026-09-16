//! `SQLite`-backed durable catalog for the search plane (W2, G0-C).
//!
//! The first table is the ingest idempotency record (QI-BB-032): one row
//! per `(operation kind, repo, revision, generation, batch_digest)` carrying
//! the canonical body hash, the apply state and the receipt of the apply.
//! Its contract is [`quanta_index_core::IdempotencyCatalogPort`]; this crate
//! is the only place the storage engine is named.
//!
//! The G0-C conditions bind here:
//! - every row carries its own digest over its content, verified on read,
//!   because the engine serves bit-rotted cells with a clean
//!   `integrity_check`;
//! - authority rows commit under `synchronous=FULL` (and `fullfsync=ON`
//!   where the platform honors it), never `NORMAL`;
//! - a write that meets a held lock past the busy budget is a typed
//!   `CATALOG_BUSY`, not a blocked thread.

#![forbid(unsafe_code)]

mod idempotency;

pub use idempotency::{IDEMPOTENCY_CATALOG_FILE_NAME, SqliteIdempotencyCatalog, catalog_dir};
