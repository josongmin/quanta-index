#![forbid(unsafe_code)]

//! BRIDGE-01 — Sourcegraph → LQ translator plus bridge packet export helpers.
//!
//! This crate is the `sg2lq` surface from
//! [BRIDGE-01](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/BRIDGE-01.md).
//! It accepts a Sourcegraph-syntax query string, lowers the
//! documented subset into a canonical typed `LqQuery`, emits a
//! [`BridgeCandidate`] envelope stamped with [`TRANSLATOR_VERSION`],
//! and can export active bridge packets for downstream CodeQL-facing
//! consumers.
//!
//! ## Scope lock
//!
//! * One-way: Sourcegraph → LQ. The reverse direction is **explicitly
//!   out of scope** (RFC § Claim Discipline item 4 / § Compatibility
//!   Rules: `Sourcegraph query ⊂ LQ`, not bijective).
//! * This crate owns Sourcegraph translation plus bridge packet
//!   shaping. It does **not** embed an LQ executor, a sink router, or
//!   an `OTel` emitter.
//! * Subset table buckets (adopted / normalized / refused) are codified
//!   by the bridge translator implementation exported as
//!   [`translate_query`].
//!
//! ## Discipline
//!
//! * **No silent failure**: every unsupported Sourcegraph construct
//!   returns a typed [`BridgeError`]. No `panic!`/`unwrap`/`expect`/
//!   `todo!`/`unimplemented!` on production paths.
//! * **D18 — hand-rolled serde**. No proc-macro derives anywhere in
//!   this crate; semgrep `rust-no-serde-derive` covers enforcement.
//! * **Translator version stamping** on every output, per BRIDGE-01
//!   § 5.4 step 4.
//! * **Version-pin gating** at the front door: malformed
//!   `SourcegraphVersionTag` constructors return `BRIDGE_VERSION_PIN`.
//!
//! ## p99 envelope
//!
//! Translate is pure CPU + bounded; the [BRIDGE-01 § 9](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/BRIDGE-01.md)
//! `≤ 1 ms p99` target is the consumer of this crate's perf budget.
//! No async, no I/O, no allocator hot spots beyond `Box<str>` /
//! `Vec` clones.

pub mod candidate;
pub mod errors;
pub mod packet;
pub mod syntax;
mod translator;
pub mod version;

pub use candidate::BridgeCandidate;
pub use errors::{BridgeError, BridgeErrorCode};
pub use packet::export_bridge_candidate_packet;
pub use syntax::{SgFilter, SgQuery, parse_sourcegraph};
pub use translator::translate_query;
pub use version::{SUPPORTED_SG_VERSION, SourcegraphVersionTag, TRANSLATOR_VERSION};
