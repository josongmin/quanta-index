//! Selected-row preview semantics for Native readers.
//!
//! The caller supplies an authenticated source, prepared predicates and its
//! own work authority. This module never creates source identity or query
//! eligibility from a returned hit.

mod budget;
mod window;
#[cfg(test)]
mod witness;

pub(crate) use budget::{SnippetLimits, token_allocation_bound_bytes};
#[cfg(test)]
pub(crate) use budget::{WitnessBudget, WitnessResult, WitnessStop};
pub(crate) use window::{PositiveWitness, WindowError, selected_content_window};
