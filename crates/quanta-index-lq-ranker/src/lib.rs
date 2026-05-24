#![forbid(unsafe_code)]

//! LEX-06 — Composite ranker (partial salvage).
//!
//! Wave-4 entry point per `docs/plans/may-24-lexical-indexing-sorucegraph/tickets/LEX-06.md`.
//! Three foundational modules landed; the scorer / tiebreak / explanation
//! glue is pending the next dispatch.
//!
//! ## Shipped
//!
//! - `errors`: closed `RankerErrorCode` (5 variants) + `SignalKind` + `RankerError`
//! - `weights`: `RankerWeightsV1` + `DEFAULTS` + envelope-validated `new` + version-tagged `weights_hash`
//! - `signals`: `CandidateSignals` + envelope constants + `validate_signals`
//! - `scorer`: `CompositeScorer` with closed-form weighted sum + clamp + `RankExplanation`
//! - `tiebreak`: `ScoredCandidate` + 6-tier `TiebreakKey` + stable `rank_candidates`

pub mod errors;
pub mod scorer;
pub mod signals;
pub mod tiebreak;
pub mod weights;

pub use errors::{RankerError, RankerErrorCode, SignalKind};
pub use scorer::{CompositeScorer, RankExplanation, SignalContribution};
pub use signals::{
    BOOST_DIRECTIVE_IDENTITY, BOOST_DIRECTIVE_MAX, BOOST_DIRECTIVE_MIN, CandidateSignals,
    validate_signals,
};
pub use tiebreak::{ScoredCandidate, TiebreakKey, rank_candidates};
pub use weights::{RankerWeightsV1, weights_hash};
