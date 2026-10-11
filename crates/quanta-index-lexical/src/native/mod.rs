//! Native owner-local byte-literal index.

mod edit_spool;
mod literal;

pub use literal::{
    LiteralCandidate, LiteralHit, LiteralPage, LiteralPlan, LiteralQuery, LiteralReadContext,
    NativeLiteralIndex,
};
