//! Regex planning and deployment-policy admission.
//!
//! Consumes a regex source string and produces a typed [`RegexPlan`] that
//! the executor routes through
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
    /// behaviour of `quanta_index_lq_regex::RegexExecutor::prepare`).
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
/// The default planning-state budget equals the upstream `MAX_NFA_STATES`.
/// Deployments can tighten it; a larger value cannot bypass the upstream cap.
/// The gate runs on validated HIR before literal extraction or engine creation.
/// This structural charge is not an aggregate physical heap bound.
///
/// Candidate materialization is bounded by the canonical trigram cap and
/// request execution budget. The generation's text authority owns trigram
/// availability; it does not depend on a separate corpus-size threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegexPolicy {
    pub max_nfa_states: u64,
    pub require_literal: bool,
}

impl RegexPolicy {
    /// Defaults: upstream planning-state budget without requiring a literal.
    /// Patterns without usable literals use bounded verification.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            max_nfa_states: quanta_index_lq_regex::MAX_NFA_STATES,
            require_literal: false,
        }
    }
}

/// Frozen plan for a single regex leaf.
///
/// The constructor is private to this module; produce instances via
/// [`plan_regex`] so the policy gate is the only entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexPlan {
    source: String,
    literal_alternation: Vec<Vec<u8>>,
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
}

/// Plan a regex leaf.
///
/// Pipeline:
///
/// 1. prepare validated HIR and apply the upstream planning-state budget;
/// 2. enforce the deployment's tighter state budget before engine creation;
/// 3. extract mandatory byte literals via
///    `RegexCompilationPlan::prefilter_literal_alternation`;
/// 4. enforce [`RegexPolicy::require_literal`] when set.
///
/// Errors are typed against [`RegexPlannerError`]; no silent fallback.
pub fn plan_regex(
    source: &str,
    _options: &LqOptions,
    policy: &RegexPolicy,
) -> Result<RegexPlan, RegexPlannerError> {
    let prepared = match RegexExecutor::prepare(source) {
        Ok(prepared) => prepared,
        Err(err) => return Err(map_compile_error(source, &err, policy)),
    };
    let budget = policy
        .max_nfa_states
        .min(quanta_index_lq_regex::MAX_NFA_STATES);
    if prepared.estimated_states() > budget {
        return Err(RegexPlannerError::UnboundedCandidatePlan {
            estimated_states: prepared.estimated_states(),
            budget,
        });
    }
    let mut literals = match prepared.prefilter_literal_alternation() {
        Ok(v) => v,
        Err(err)
            if err.code == RegexErrorCode::RegexPrefilterUnusable && !policy.require_literal =>
        {
            // An explicit empty alternation selects the bounded verify-only
            // path. This preserves regex truth when no usable literal exists.
            Vec::new()
        }
        Err(err) => return Err(map_literal_error(source, &err, policy)),
    };
    if literals.is_empty() || literals.iter().any(Vec::is_empty) {
        if policy.require_literal {
            return Err(RegexPlannerError::UnsupportedFeature {
                feature: "regex_without_extractable_literal",
            });
        }
        // An empty alternative gives no nonempty witness for every match.
        // Normalize it to the explicit bounded verify-only plan.
        literals.clear();
    }
    Ok(RegexPlan {
        source: source.to_owned(),
        literal_alternation: literals,
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
                budget: policy
                    .max_nfa_states
                    .min(quanta_index_lq_regex::MAX_NFA_STATES),
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

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "planner regressions assert fixed policy boundaries and literal oracles"
)]
mod tests {
    use super::{RegexPlannerError, RegexPolicy, plan_regex};
    use quanta_index_contract::LqOptions;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn configured_state_limit_is_enforced_at_the_exact_boundary() -> TestResult {
        let mut policy = RegexPolicy {
            max_nfa_states: 6,
            ..RegexPolicy::defaults()
        };
        // A six-byte literal has a fixed seven-state planning charge.
        assert_eq!(
            plan_regex("needle", &LqOptions::defaults(), &policy),
            Err(RegexPlannerError::UnboundedCandidatePlan {
                estimated_states: 7,
                budget: 6,
            })
        );
        policy.max_nfa_states = 7;
        let admitted = plan_regex("needle", &LqOptions::defaults(), &policy)?;
        assert_eq!(admitted.literal_alternation(), &[b"needle".to_vec()]);
        Ok(())
    }

    #[test]
    fn zero_state_policy_refuses_valid_input_without_hiding_syntax_errors() {
        let policy = RegexPolicy {
            max_nfa_states: 0,
            ..RegexPolicy::defaults()
        };
        assert_eq!(
            plan_regex("", &LqOptions::defaults(), &policy),
            Err(RegexPlannerError::UnboundedCandidatePlan {
                estimated_states: 1,
                budget: 0,
            })
        );
        assert!(matches!(
            plan_regex("[", &LqOptions::defaults(), &policy),
            Err(RegexPlannerError::ParseError { .. })
        ));
    }

    #[test]
    fn fallback_only_requires_literal_when_policy_explicitly_demands_it() -> TestResult {
        let mut policy = RegexPolicy::defaults();
        let admitted = plan_regex(".*", &LqOptions::defaults(), &policy)?;
        assert!(admitted.literal_alternation().is_empty());
        policy.require_literal = true;
        assert_eq!(
            plan_regex(".*", &LqOptions::defaults(), &policy),
            Err(RegexPlannerError::UnsupportedFeature {
                feature: "regex_without_extractable_literal",
            })
        );
        Ok(())
    }

    #[test]
    fn planning_extracts_literals_without_constructing_an_oversized_engine() -> TestResult {
        let plan = plan_regex(
            r"needle[\x{80}-\x{10FFFF}]{20000}",
            &LqOptions::defaults(),
            &RegexPolicy::defaults(),
        )?;
        assert_eq!(plan.literal_alternation(), &[b"needle".to_vec()]);
        Ok(())
    }

    #[test]
    fn strict_literal_policy_rejects_empty_match_alternatives() -> TestResult {
        let policy = RegexPolicy {
            require_literal: true,
            ..RegexPolicy::defaults()
        };
        assert!(plan_regex("needle|other", &LqOptions::defaults(), &policy).is_ok());
        for source in ["", "^$", r"\b", "(?:needle)?", "needle|"] {
            assert_eq!(
                plan_regex(source, &LqOptions::defaults(), &policy),
                Err(RegexPlannerError::UnsupportedFeature {
                    feature: "regex_without_extractable_literal",
                }),
                "zero-width alternative bypassed literal policy: {source:?}"
            );
            let admitted = plan_regex(source, &LqOptions::defaults(), &RegexPolicy::defaults())?;
            assert!(admitted.literal_alternation().is_empty(), "{source:?}");
        }
        Ok(())
    }

    #[test]
    fn deployment_policy_cannot_raise_the_upstream_state_cap() {
        let policy = RegexPolicy {
            max_nfa_states: u64::MAX,
            ..RegexPolicy::defaults()
        };
        assert_eq!(
            plan_regex("a{1000000}", &LqOptions::defaults(), &policy),
            Err(RegexPlannerError::UnboundedCandidatePlan {
                estimated_states: 0,
                budget: quanta_index_lq_regex::MAX_NFA_STATES,
            })
        );
    }
}
