//! Immutable, document-partitioned Native Text.
pub(super) mod build;
pub(super) mod build_v2;
mod compact_v2;
pub(super) mod read;
mod retired;
mod spill;
pub(super) mod wire;
pub(super) mod wire_v2;
