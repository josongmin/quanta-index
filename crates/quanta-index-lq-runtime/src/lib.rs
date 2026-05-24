#![forbid(unsafe_code)]

//! RT-01 — Runtime metadata + `dirty:` apply-changes channel.
//!
//! Implements the search-plane-side consumer for producer-marked dirty
//! documents per `docs/plans/may-24-lexical-indexing-sorucegraph/tickets/RT-01.md`.
//!
//! The crate exposes:
//!
//! - typed shared newtypes ([`TenantId`], [`RepoId`], [`DocId`],
//!   [`ManifestGeneration`], [`ApplyTimeMs`], [`BufferConfig`]),
//! - the inbound `apply_changes` payload ([`DirtyEntry`]) and typed verdict
//!   ([`ApplyOutcome`]),
//! - the per-(tenant, repo) bounded buffer ([`DirtyBuffer`]) with TTL sweep
//!   and capacity guard,
//! - the read-side resolver ([`dirty_docs`]) that enforces generation
//!   pinning and never silently widens to other generations.
//!
//! ## Locks
//!
//! - D18 — every wire shape is hand-rolled serde; no proc-macro derives.
//! - No silent failure — every reject is a typed [`RuntimeErrorCode`]; every
//!   TTL eviction surfaces the explicit list of evicted [`DocId`]s.
//! - Time is caller-supplied via [`ApplyTimeMs`]; the crate never reads the
//!   wall clock.
//! - Write-coordinator gated — the buffer is single-writer per (tenant, repo)
//!   sharing the LEX-04 advisory lock per RT-01 § 4.4.

pub mod apply_changes;
pub mod buffer;
pub mod errors;
pub mod query;
pub mod types;

pub use apply_changes::{ApplyOutcome, DirtyEntry, PAYLOAD_HASH_LEN};
pub use buffer::DirtyBuffer;
pub use errors::{RuntimeError, RuntimeErrorCode, StateNotReadyReason};
pub use query::dirty_docs;
pub use types::{
    ApplyTimeMs, BufferConfig, DEFAULT_BUFFER_CAPACITY, DEFAULT_TTL_MS, DocId, ManifestGeneration,
    RepoId, TenantId,
};
