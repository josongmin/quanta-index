//! Result DTOs shared between producer and search-plane.
//!
//! These types are envelope-free leaf shapes: a candidate row, a structural
//! match, a diff row, a bridge packet. They depend only on the core ID
//! newtypes from [`crate::ids`], so downstream crates that only build or
//! inspect result rows can take `quanta-index-contract-base` instead of the
//! full `quanta-index-contract`.

mod candidates;
mod cursor_envelope;
mod diff_candidate;
mod history_score;
mod query_window;
mod structural;

pub use candidates::*;
pub use cursor_envelope::*;
pub use diff_candidate::*;
pub use history_score::*;
pub use query_window::*;
pub use structural::*;
