//! Result DTOs shared between producer and search-plane.
//!
//! These types are envelope-free leaf shapes: a candidate row, a structural
//! match, a diff row, a bridge packet. They depend only on the core ID
//! newtypes from [`crate::ids`], so downstream crates that only build or
//! inspect result rows can take `quanta-index-contract-base` instead of the
//! full `quanta-index-contract`.

mod bridge;
mod candidates;
mod diff_candidate;
mod structural;

pub use bridge::*;
pub use candidates::*;
pub use diff_candidate::*;
pub use structural::*;
