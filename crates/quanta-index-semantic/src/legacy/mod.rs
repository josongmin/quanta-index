//! Canonical persisted Legacy data borrowed by existing-only verification.
//!
//! Decoding creates no runtime, provider, IO, activation, Source or query
//! authority. Physical verification separately requires genuine root-bound
//! ports, Original admission and caller proof. Caller IO/allocation admission
//! must precede this pure operation.

#[cfg(unix)]
mod existing;
mod generation_data;
#[cfg(unix)]
pub use existing::verify_existing_semantic_generation_read_only;
pub use generation_data::with_decoded_generation_data;

pub mod layout;
pub mod limits;
pub(crate) mod sealed;
