#![forbid(unsafe_code)]

//! BRIDGE-01 — Sourcegraph → LQ translator plus bridge packet export helpers.
//!
//! This crate is the `sg2lq` surface from
//! `docs/adr/JUN-06-001-sourcegraph-compatibility-boundary.md`.
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
//!   this crate; `check-rust-derive-allowlist.py` covers enforcement.
//! * **Translator version stamping** on every output, per BRIDGE-01
//!   § 5.4 step 4.
//! * **Version-pin gating** at the front door: malformed
//!   `SourcegraphVersionTag` constructors return `BRIDGE_VERSION_PIN`.
//!
//! ## p99 envelope
//!
//! Translate is pure CPU + bounded. The historical `≤ 1 ms p99` target is
//! not a current measured qualification claim; it requires an admitted
//! benchmark under `docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md`.
//! No async, no I/O, no allocator hot spots beyond `Box<str>` /
//! `Vec` clones.

pub mod candidate;
pub mod errors;
pub mod syntax;
mod translator;
pub mod version;

pub use candidate::BridgeCandidate;
pub use errors::{BridgeError, BridgeErrorCode};
pub use syntax::{SgFilter, SgQuery, parse_sourcegraph};
pub use translator::translate_query;
pub use version::{SUPPORTED_SG_VERSION, SourcegraphVersionTag, TRANSLATOR_VERSION};
