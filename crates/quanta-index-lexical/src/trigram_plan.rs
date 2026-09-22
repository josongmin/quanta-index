//! Raw-substring (trigram) planner scaffold (ticket LXE-04).
//!
//! Consumes a raw needle string and produces a typed [`TrigramPlan`] the
//! executor (follow-up integration pass) will route through
//! `quanta_index_lq_trigram::query_raw_substring` for prefilter +
//! exact-byte verification. The planner is a pure function from
//! `(needle, options, policy)` to `Result<TrigramPlan,
//! TrigramPlannerError>`; it owns no I/O and no candidate iteration.
//!
//! Vendor sealing: this module imports `quanta_index_lq_trigram` only
//! for the [`TRIGRAM_LEN`] constant; no `memchr::*` / vendor trigram
//! token escapes through error messages or trace fields.
//!
//! [`TRIGRAM_LEN`]: quanta_index_lq_trigram::TRIGRAM_LEN

use core::fmt;

use quanta_index_contract::LqOptions;
use quanta_index_lq_trigram::TRIGRAM_LEN;

/// Typed trigram planner errors.
///
/// `Display` / [`std::error::Error`] are implemented by hand
/// (per the workspace no-proc-macro-derive build-hygiene rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrigramPlannerError {
    /// Needle is shorter than the trigram width and cannot be prefiltered.
    ///
    /// The trigram shard cannot answer sub-3-byte needles: every document
    /// would have to be verified. Surfacing this as typed forces the
    /// caller to either widen the needle or route through a different
    /// engine — there is NO silent full-scan fallback.
    NeedleTooShort { len: usize, min_required: usize },
    /// Needle is well-formed under the length cap but is rejected for a
    /// structural reason (e.g. empty after normalization).
    ///
    /// `reason` is a stable static label suitable for explain traces.
    UnsupportedNeedle { reason: &'static str },
}

impl fmt::Display for TrigramPlannerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedleTooShort { len, min_required } => {
                write!(
                    f,
                    "trigram planner: needle length {len} below minimum {min_required}"
                )
            }
            Self::UnsupportedNeedle { reason } => {
                write!(f, "trigram planner: unsupported needle '{reason}'")
            }
        }
    }
}

impl std::error::Error for TrigramPlannerError {}

/// Policy knobs for [`plan_raw_substring`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrigramPolicy {
    pub default_candidate_cap: u32,
    pub min_needle_bytes: usize,
}

impl TrigramPolicy {
    /// Sane defaults: 50k candidate cap, 3-byte minimum needle.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            default_candidate_cap: 50_000,
            min_needle_bytes: TRIGRAM_LEN,
        }
    }
}

/// Reason a trigram execution loop terminates before exhausting its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrigramEarlyStopReason {
    /// The per-leaf candidate cap was hit and further candidates dropped.
    CandidateCapHit,
    /// The verification budget elapsed.
    BudgetExhausted,
    /// The pre-verify candidate set was empty.
    EmptyResult,
}

/// Mutable accumulator the executor pass populates while iterating.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrigramTraceBuilder {
    pub prefilter_candidate_count: u64,
    pub verify_count: u64,
    pub early_stop_reason: Option<TrigramEarlyStopReason>,
}

impl TrigramTraceBuilder {
    /// Construct an empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Freeze the accumulator into an immutable [`TrigramTrace`].
    #[must_use]
    pub fn build(self) -> TrigramTrace {
        TrigramTrace {
            prefilter_candidate_count: self.prefilter_candidate_count,
            verify_count: self.verify_count,
            early_stop_reason: self.early_stop_reason,
        }
    }
}

/// Frozen trace value emitted by [`TrigramTraceBuilder::build`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrigramTrace {
    pub prefilter_candidate_count: u64,
    pub verify_count: u64,
    pub early_stop_reason: Option<TrigramEarlyStopReason>,
}

/// Frozen plan for a single raw-substring leaf.
///
/// The constructor is private; produce instances via
/// [`plan_raw_substring`] so the policy gate is the only entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrigramPlan {
    needle: String,
    candidate_cap: u32,
    trace: TrigramTraceBuilder,
}

impl TrigramPlan {
    /// Borrow the raw needle.
    #[must_use]
    pub fn needle(&self) -> &str {
        &self.needle
    }

    /// Per-leaf candidate cap chosen by [`plan_raw_substring`].
    #[must_use]
    pub const fn candidate_cap(&self) -> u32 {
        self.candidate_cap
    }

    /// Borrow the in-progress trace builder.
    #[must_use]
    pub fn trace(&self) -> &TrigramTraceBuilder {
        &self.trace
    }

    /// Borrow the trace builder mutably so the executor pass can fill
    /// candidate counts and early-stop reason.
    pub fn trace_mut(&mut self) -> &mut TrigramTraceBuilder {
        &mut self.trace
    }
}

/// Plan a raw-substring (trigram) leaf.
///
/// Pipeline:
///
/// 1. enforce the minimum-byte-length policy
///    ([`TrigramPolicy::min_needle_bytes`], default [`TRIGRAM_LEN`]);
/// 2. attach the per-leaf candidate cap.
///
/// Errors are typed against [`TrigramPlannerError`]; no silent fallback.
pub fn plan_raw_substring(
    needle: &str,
    _options: &LqOptions,
    policy: &TrigramPolicy,
) -> Result<TrigramPlan, TrigramPlannerError> {
    if needle.is_empty() {
        return Err(TrigramPlannerError::UnsupportedNeedle {
            reason: "empty_needle",
        });
    }
    if needle.len() < policy.min_needle_bytes {
        return Err(TrigramPlannerError::NeedleTooShort {
            len: needle.len(),
            min_required: policy.min_needle_bytes,
        });
    }
    Ok(TrigramPlan {
        needle: needle.to_owned(),
        candidate_cap: policy.default_candidate_cap,
        trace: TrigramTraceBuilder::new(),
    })
}
