//! Regex planner scaffold (ticket LXE-04).
//!
//! Consumes a regex source string and produces a typed [`RegexPlan`] that
//! the executor (follow-up integration pass) will route through
//! `quanta_index_lq_regex::RegexExecutor` for verification. The planner is
//! a pure function from `(source, options, policy)` to `Result<RegexPlan,
//! RegexPlannerError>`; it owns no I/O, no candidate iteration, and no
//! regex engine state.
//!
//! Vendor sealing: this module talks to `quanta_index_lq_regex` only
//! through its public domain surface (`RegexExecutor`, `RegexError`,
//! `RegexErrorCode`, `ForbiddenKind`, `LimitDimension`). No `regex::*`
//! or `regex_syntax::*` token escapes through error messages or trace
//! fields.

use core::fmt;

use quanta_index_contract::LqOptions;
use quanta_index_lq_regex::{ForbiddenKind, RegexErrorCode, RegexExecutor};

/// Typed regex planner errors.
///
/// `Display` / [`std::error::Error`] are implemented by hand
/// (per the workspace no-proc-macro-derive build-hygiene rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegexPlannerError {
    /// Regex source did not parse under the RE2 dialect.
    ///
    /// `source` is the offending pattern; `detail` is an engineering-
    /// facing message carrying the byte offset of the parse failure.
    ParseError { source: String, detail: String },
    /// Pattern uses a construct the LQ regex dialect forbids
    /// (lookbehind, lookahead, backreference, possessive group,
    /// named-capture reference, mid-pattern inline flag, unicode class).
    ///
    /// `feature` is a stable static label suitable for explain traces
    /// and metrics (e.g. `lookbehind`, `lookahead`, `backreference`).
    UnsupportedFeature { feature: &'static str },
    /// Planning-time NFA budget exceeded.
    ///
    /// `estimated_states` is `0` when the upstream executor surfaced the
    /// cap exceed without returning the actual estimate (current
    /// behaviour of `quanta_index_lq_regex::RegexExecutor::compile`).
    /// `budget` carries the planner's policy cap so explain-trace
    /// consumers can attribute the failure correctly.
    UnboundedCandidatePlan { estimated_states: u64, budget: u64 },
}

impl fmt::Display for RegexPlannerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParseError { source, detail } => {
                write!(f, "regex planner: parse error for {source:?}: {detail}")
            }
            Self::UnsupportedFeature { feature } => {
                write!(f, "regex planner: unsupported feature '{feature}'")
            }
            Self::UnboundedCandidatePlan {
                estimated_states,
                budget,
            } => write!(
                f,
                "regex planner: unbounded candidate plan (estimated {estimated_states} states, budget {budget})"
            ),
        }
    }
}

impl std::error::Error for RegexPlannerError {}

/// Policy knobs for [`plan_regex`] and for the lexical adapter's regex
/// execution gating.
///
/// Default values produced by [`RegexPolicy::defaults`] are coupled to the
/// upstream `quanta_index_lq_regex` constants (`MAX_NFA_STATES = 100_000`).
/// Tightening `max_nfa_states` below the upstream constant is accepted by
/// the planner surface but not enforced today, because
/// `RegexExecutor::compile` does not return the estimated state count
/// when it succeeds. The field is preserved for future enforcement and
/// for error reporting on cap-exceed.
///
/// `trigram_missing_doc_threshold` is the corpus-size cap above which the
/// lexical adapter's `compile_regex_content_leaf` surfaces
/// `LEX_REGEX_TRIGRAM_INDEX_MISSING` instead of running a vendor full-scan
/// regex. The lexical adapter does not yet maintain a trigram-postings
/// field over indexed content; until that lands we honestly admit the gap
/// above this threshold rather than silently running an O(corpus) regex
/// on a large index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegexPolicy {
    pub max_nfa_states: u64,
    pub default_candidate_cap: u32,
    pub require_literal: bool,
    pub trigram_missing_doc_threshold: u64,
}

impl RegexPolicy {
    /// Sane defaults: 10k NFA states budget, 10k candidate cap, mandatory
    /// literal NOT required (pure-wildcard patterns fall through to
    /// verify-only execution downstream), and 10k corpus-size cap before
    /// the trigram-missing typed error fires.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            max_nfa_states: 10_000,
            default_candidate_cap: 10_000,
            require_literal: false,
            trigram_missing_doc_threshold: 10_000,
        }
    }
}

/// Reason a regex execution loop terminates before exhausting its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EarlyStopReason {
    /// The per-leaf candidate cap was hit and further candidates dropped.
    CandidateCapHit,
    /// The verification budget elapsed.
    BudgetExhausted,
    /// The pre-verify candidate set was empty.
    EmptyResult,
}

/// Mutable accumulator the executor pass populates while iterating.
///
/// Frozen into [`RegexTrace`] via [`RegexTraceBuilder::build`] once a
/// leaf finishes. The planner constructs an empty builder; the executor
/// (follow-up integration pass) fills in counts and the optional
/// early-stop reason.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegexTraceBuilder {
    pub extracted_literals: Vec<Vec<u8>>,
    pub prefilter_candidate_count: u64,
    pub verify_count: u64,
    pub early_stop_reason: Option<EarlyStopReason>,
}

impl RegexTraceBuilder {
    /// Construct an empty builder. Equivalent to `Default::default()`
    /// but call-site explicit.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Freeze the accumulator into an immutable [`RegexTrace`].
    #[must_use]
    pub fn build(self) -> RegexTrace {
        RegexTrace {
            extracted_literals: self.extracted_literals,
            prefilter_candidate_count: self.prefilter_candidate_count,
            verify_count: self.verify_count,
            early_stop_reason: self.early_stop_reason,
        }
    }
}

/// Frozen trace value emitted by [`RegexTraceBuilder::build`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexTrace {
    pub extracted_literals: Vec<Vec<u8>>,
    pub prefilter_candidate_count: u64,
    pub verify_count: u64,
    pub early_stop_reason: Option<EarlyStopReason>,
}

/// Frozen plan for a single regex leaf.
///
/// The constructor is private to this module; produce instances via
/// [`plan_regex`] so the policy gate is the only entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexPlan {
    source: String,
    literal_alternation: Vec<Vec<u8>>,
    candidate_cap: u32,
    trace: RegexTraceBuilder,
}

impl RegexPlan {
    /// Borrow the regex source pattern.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Borrow the planner-extracted byte-literal alternation.
    ///
    /// A match must contain **one** of these, not all of them; see
    /// `quanta_index_lq_regex::extract_prefilter_literal_alternation`.
    #[must_use]
    pub fn literal_alternation(&self) -> &[Vec<u8>] {
        &self.literal_alternation
    }

    /// Per-leaf candidate cap chosen by [`plan_regex`].
    #[must_use]
    pub const fn candidate_cap(&self) -> u32 {
        self.candidate_cap
    }

    /// Borrow the in-progress trace builder.
    #[must_use]
    pub fn trace(&self) -> &RegexTraceBuilder {
        &self.trace
    }

    /// Borrow the trace builder mutably so the executor pass can fill
    /// candidate counts and early-stop reason.
    pub fn trace_mut(&mut self) -> &mut RegexTraceBuilder {
        &mut self.trace
    }
}

/// Plan a regex leaf.
///
/// Pipeline:
///
/// 1. compile via `quanta_index_lq_regex::RegexExecutor::compile`, which
///    runs the dialect filter, parses to HIR, and applies the upstream
///    NFA-state budget;
/// 2. extract mandatory byte literals via
///    `RegexExecutor::prefilter_literal_alternation`;
/// 3. enforce [`RegexPolicy::require_literal`] when set.
///
/// Errors are typed against [`RegexPlannerError`]; no silent fallback.
pub fn plan_regex(
    source: &str,
    _options: &LqOptions,
    policy: &RegexPolicy,
) -> Result<RegexPlan, RegexPlannerError> {
    let executor = match RegexExecutor::compile(source) {
        Ok(e) => e,
        Err(err) => return Err(map_compile_error(source, &err, policy)),
    };
    let literals = match executor.prefilter_literal_alternation() {
        Ok(v) => v,
        Err(err) => return Err(map_literal_error(source, &err, policy)),
    };
    if literals.is_empty() && policy.require_literal {
        return Err(RegexPlannerError::UnsupportedFeature {
            feature: "regex_without_extractable_literal",
        });
    }
    Ok(RegexPlan {
        source: source.to_owned(),
        literal_alternation: literals,
        candidate_cap: policy.default_candidate_cap,
        trace: RegexTraceBuilder::new(),
    })
}

/// Map a `RegexError` from the compile step into a typed
/// [`RegexPlannerError`].
///
/// Vendor tokens (`regex::*`, `regex_syntax::*`) are deliberately
/// stripped: only the upstream domain code/qualifier surface and a
/// short detail string are forwarded.
fn map_compile_error(
    source: &str,
    err: &quanta_index_lq_regex::RegexError,
    policy: &RegexPolicy,
) -> RegexPlannerError {
    match err.code {
        RegexErrorCode::ParseFail | RegexErrorCode::ExecutionInternal => {
            RegexPlannerError::ParseError {
                source: source.to_owned(),
                detail: err.detail.to_string(),
            }
        }
        // Only candidate verification can be interrupted; a compile that
        // reports it is an executor defect, surfaced rather than mapped
        // onto a construct the pattern does not have.
        RegexErrorCode::Interrupted => RegexPlannerError::ParseError {
            source: source.to_owned(),
            detail: format!(
                "compile reported an interruption, which only verification raises: {}",
                err.detail
            ),
        },
        RegexErrorCode::ForbiddenSyntax => RegexPlannerError::UnsupportedFeature {
            feature: err.forbidden.map_or("forbidden_construct", forbidden_label),
        },
        RegexErrorCode::PlanLimitExceeded | RegexErrorCode::QueryTimeout => {
            RegexPlannerError::UnboundedCandidatePlan {
                estimated_states: 0,
                budget: policy.max_nfa_states,
            }
        }
        RegexErrorCode::RegexPrefilterUnusable => RegexPlannerError::UnsupportedFeature {
            feature: "regex_without_extractable_literal",
        },
    }
}

/// Map a `RegexError` from the literal-extraction step. Distinct from
/// [`map_compile_error`] because the only failure here is
/// [`RegexErrorCode::RegexPrefilterUnusable`] (= no mandatory literal).
fn map_literal_error(
    source: &str,
    err: &quanta_index_lq_regex::RegexError,
    policy: &RegexPolicy,
) -> RegexPlannerError {
    if err.code == RegexErrorCode::RegexPrefilterUnusable {
        return RegexPlannerError::UnsupportedFeature {
            feature: "regex_without_extractable_literal",
        };
    }
    map_compile_error(source, err, policy)
}

/// Stable static label for a forbidden-construct kind. Mirrors the
/// upstream `as_code_str` shape but pinned to `&'static str` so the
/// planner error can carry it without allocating.
const fn forbidden_label(kind: ForbiddenKind) -> &'static str {
    match kind {
        ForbiddenKind::Lookahead => "lookahead",
        ForbiddenKind::Lookbehind => "lookbehind",
        ForbiddenKind::Backref => "backreference",
        ForbiddenKind::Possessive => "possessive",
        ForbiddenKind::NamedCaptureRef => "named_capture_ref",
        ForbiddenKind::InlineFlagMidPattern => "inline_flag_midpattern",
        ForbiddenKind::UnicodeClass => "unicode_class",
    }
}
