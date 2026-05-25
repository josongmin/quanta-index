//! LXE-05 — phrase planner + position-engine binding scaffold.
//!
//! Owns the plan-time shape produced for `LqLeaf::Phrase`: tokenization,
//! field routing (content / path / symbol), case-sensitivity carryover from
//! [`LqOptions`], and typed rejection of phrase extensions the position
//! engine does not yet model (slop, symbol-field phrase).
//!
//! Execution against the live position index is wired through the local
//! [`PhrasePositionLookup`] port so the planner stays adapter-agnostic; the
//! Tantivy / `quanta-index-lq-positions` integration happens in a follow-up
//! coordination pass (planner.rs / lib.rs touch by sibling agents). The
//! tokenization called here is a deliberate whitespace split — see the
//! `TODO[LXE-05-integration]` marker for the position-engine analyzer that
//! must replace it once both sides land in the same PR.
//!
//! Display / `std::error::Error` impls are hand-rolled per the workspace
//! no-proc-macro-derive build-hygiene rule (`thiserror` may not be added
//! to this crate).

use core::fmt;

use quanta_index_contract::{LqCase, LqOptions};

/// Field the position engine should resolve the phrase against.
///
/// `Content` and `Path` are the supported text-field routes today. `Symbol`
/// is a planner-recognized variant so callers can express the intent, but
/// the planner rejects it with [`PhrasePlannerError::UnsupportedField`]
/// until the symbol position index lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PhraseField {
    Content,
    Path,
    Symbol,
}

impl PhraseField {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Path => "path",
            Self::Symbol => "symbol",
        }
    }
}

/// Reason a phrase plan tripped an early-stop guard during execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EarlyStopReason {
    /// Candidate cap exhausted before the postings stream ended.
    CandidateCapHit,
    /// One of the phrase terms had no postings — phrase cannot match.
    EmptyPostings,
}

impl EarlyStopReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CandidateCapHit => "candidate_cap_hit",
            Self::EmptyPostings => "empty_postings",
        }
    }
}

/// Per-leaf plan-time policy knobs for phrase planning.
///
/// All values are intentionally `const` — phrase policy is a pure planner
/// concern, not an adapter knob, and lives next to the planner so reviewers
/// see the limits without leaving the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhrasePolicy {
    /// Default candidate cap surfaced into [`PhrasePlan::candidate_cap`].
    pub default_candidate_cap: u32,
    /// Minimum number of tokens after normalization. Single-token phrase is
    /// degenerate but accepted (the call site owns the policy decision).
    pub min_tokens: usize,
    /// Whether non-zero slop is accepted by the DSL/planner. Today the LQ
    /// DSL has no slop syntax, so this is `false` and any slop > 0 is a
    /// typed error.
    pub allow_slop: bool,
    /// Maximum slop value, only consulted when `allow_slop` is true.
    pub max_slop: u32,
}

impl PhrasePolicy {
    /// Canonical defaults: cap `10_000` candidates, min 1 token, slop banned.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            default_candidate_cap: 10_000,
            min_tokens: 1,
            allow_slop: false,
            max_slop: 0,
        }
    }
}

/// Typed planner errors for the phrase route.
///
/// Carries enough structured payload that callers can format their own
/// diagnostics without re-parsing the `Display` string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PhrasePlannerError {
    /// Input text was empty (or only whitespace) after tokenization.
    EmptyPhrase,
    /// Tokenization produced fewer terms than the policy minimum.
    TooFewTokens { count: usize, min_required: usize },
    /// Slop was requested but the policy / DSL does not yet allow it.
    UnsupportedSlop { requested: u32, max_allowed: u32 },
    /// The chosen field is recognized but not yet executable.
    UnsupportedField {
        field: PhraseField,
        reason: &'static str,
    },
}

impl fmt::Display for PhrasePlannerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPhrase => f.write_str("phrase planner: empty phrase"),
            Self::TooFewTokens {
                count,
                min_required,
            } => write!(
                f,
                "phrase planner: too few tokens (got {count}, need {min_required})",
            ),
            Self::UnsupportedSlop {
                requested,
                max_allowed,
            } => write!(
                f,
                "phrase planner: unsupported slop {requested} (max {max_allowed})",
            ),
            Self::UnsupportedField { field, reason } => write!(
                f,
                "phrase planner: unsupported field `{}` ({reason})",
                field.as_str(),
            ),
        }
    }
}

impl std::error::Error for PhrasePlannerError {}

/// Mutable trace accumulator.
///
/// Plan-time fields are filled by [`plan_phrase`]; execution-time counters
/// (`candidate_count`, `verify_count`, `early_stop_reason`) are filled by
/// the executor before calling [`PhraseTraceBuilder::build`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhraseTraceBuilder {
    pub normalized_tokens: Vec<String>,
    pub field: PhraseField,
    pub candidate_count: u32,
    pub verify_count: u32,
    pub early_stop_reason: Option<EarlyStopReason>,
}

impl PhraseTraceBuilder {
    fn new(field: PhraseField, normalized_tokens: Vec<String>) -> Self {
        Self {
            normalized_tokens,
            field,
            candidate_count: 0,
            verify_count: 0,
            early_stop_reason: None,
        }
    }

    /// Finalize into the immutable explain-trace record.
    #[must_use]
    pub fn build(self) -> PhraseTrace {
        PhraseTrace {
            normalized_tokens: self.normalized_tokens,
            field: self.field,
            candidate_count: self.candidate_count,
            verify_count: self.verify_count,
            early_stop_reason: self.early_stop_reason,
        }
    }
}

/// Immutable explain-trace snapshot produced after execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhraseTrace {
    pub normalized_tokens: Vec<String>,
    pub field: PhraseField,
    pub candidate_count: u32,
    pub verify_count: u32,
    pub early_stop_reason: Option<EarlyStopReason>,
}

/// Planned phrase shape — the position engine's input contract.
///
/// `tokens` is the post-normalization sequence that the position lookup
/// will use as term keys. `case_sensitive` is the carry-through of
/// [`LqCase`] so the executor can fold case identically on the read side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhrasePlan {
    pub tokens: Vec<String>,
    pub slop: u32,
    pub case_sensitive: bool,
    pub field: PhraseField,
    pub candidate_cap: u32,
    pub trace: PhraseTraceBuilder,
}

/// Tokenize the phrase text for the position-engine lookup.
///
/// TODO[LXE-05-integration]: replace whitespace tokenization with the
/// `quanta-index-lq-text-norm::tokenize_text` + `fold_case` pipeline that
/// the position index ingests on the write side. The whitespace path here
/// is a deliberately narrow placeholder so this file can land without
/// adding a new workspace dependency in the same PR as the planner
/// scaffold; the coordination pass that wires `planner.rs` to this module
/// will route through the real analyzer.
fn tokenize_phrase(text: &str, case_sensitive: bool) -> Vec<String> {
    text.split_whitespace()
        .map(|t| {
            if case_sensitive {
                t.to_string()
            } else {
                t.to_ascii_lowercase()
            }
        })
        .collect()
}

/// Plan a phrase leaf onto the position engine.
///
/// Order of checks:
///
/// 1. Slop request validated against policy (DSL has no slop today, so
///    only slop = 0 passes when `allow_slop` is false).
/// 2. Field validated: `Symbol` rejected, `Content` / `Path` accepted.
/// 3. Tokenize using the position-engine analyzer (placeholder today).
/// 4. Empty → `EmptyPhrase`; below `min_tokens` → `TooFewTokens`.
pub fn plan_phrase(
    text: &str,
    options: &LqOptions,
    policy: &PhrasePolicy,
    field: PhraseField,
) -> Result<PhrasePlan, PhrasePlannerError> {
    // Field gate first — cheaper to reject than to tokenize a symbol phrase.
    if matches!(field, PhraseField::Symbol) {
        return Err(PhrasePlannerError::UnsupportedField {
            field,
            reason: "phrase on symbol field not yet supported",
        });
    }

    // Slop is implicit-zero today (LQ DSL surfaces no slop syntax). The
    // explicit check guards the policy seam so when a slop-capable DSL
    // lands the gate flips by toggling `PhrasePolicy::allow_slop`.
    let slop: u32 = 0;
    if !policy.allow_slop && slop > 0 {
        return Err(PhrasePlannerError::UnsupportedSlop {
            requested: slop,
            max_allowed: 0,
        });
    }
    if policy.allow_slop && slop > policy.max_slop {
        return Err(PhrasePlannerError::UnsupportedSlop {
            requested: slop,
            max_allowed: policy.max_slop,
        });
    }

    let case_sensitive = matches!(options.case, Some(LqCase::Sensitive));
    let tokens = tokenize_phrase(text, case_sensitive);

    if tokens.is_empty() {
        return Err(PhrasePlannerError::EmptyPhrase);
    }
    if tokens.len() < policy.min_tokens {
        return Err(PhrasePlannerError::TooFewTokens {
            count: tokens.len(),
            min_required: policy.min_tokens,
        });
    }

    let trace = PhraseTraceBuilder::new(field, tokens.clone());
    Ok(PhrasePlan {
        tokens,
        slop,
        case_sensitive,
        field,
        candidate_cap: policy.default_candidate_cap,
        trace,
    })
}

/// A position-engine lookup primitive the executor depends on.
///
/// Kept narrow so the position-index adapter implements only what the phrase
/// executor consumes; the real adapter (over `quanta-index-lq-positions`)
/// lands in the coordination pass alongside `planner.rs` wiring. Today the
/// only implementer is the in-module test mock — that is sufficient to keep
/// the port off the "dead surface" list because the unit tests exercise it.
pub trait PhrasePositionLookup {
    /// Resolve a phrase plan into ordered hits.
    ///
    /// Implementations must respect `plan.case_sensitive` (the read-side
    /// case fold), `plan.field`, and `plan.candidate_cap` (early-stop).
    fn lookup_phrase(&self, plan: &PhrasePlan) -> Result<Vec<PhraseHit>, PhrasePlannerError>;
}

/// One phrase hit as observed by the position engine.
///
/// The doc / span shape intentionally mirrors `quanta-index-lq-positions::PhraseMatch`
/// without naming that crate's types — the adapter does the translation so
/// this module stays vendor-token-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PhraseHit {
    pub doc_id: u64,
    pub start_position: u32,
    pub end_position: u32,
}

/// Execute a phrase plan against the supplied position lookup.
///
/// Thin pass-through today: the executor wraps the port so callers don't
/// need to know whether the lookup is mocked or backed by a real index.
/// When the real adapter lands the trace-update path (filling
/// `candidate_count` / `verify_count` / `early_stop_reason`) moves here.
pub fn execute_phrase(
    plan: &PhrasePlan,
    index: &dyn PhrasePositionLookup,
) -> Result<Vec<PhraseHit>, PhrasePlannerError> {
    index.lookup_phrase(plan)
}

#[cfg(test)]
mod tests {
    use super::{
        EarlyStopReason, PhraseField, PhraseHit, PhrasePlan, PhrasePlannerError, PhrasePolicy,
        PhrasePositionLookup, PhraseTraceBuilder, execute_phrase, plan_phrase,
    };
    use quanta_index_contract::{LqCase, LqOptions};

    fn opts_with_case(case: Option<LqCase>) -> LqOptions {
        let mut o = LqOptions::defaults();
        o.case = case;
        o
    }

    #[test]
    fn single_token_phrase_plans_content_insensitive() {
        let opts = LqOptions::defaults();
        let outcome = plan_phrase(
            "Hello",
            &opts,
            &PhrasePolicy::defaults(),
            PhraseField::Content,
        );
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            assert_eq!(plan.tokens, vec!["hello".to_string()]);
            assert!(!plan.case_sensitive);
            assert_eq!(plan.field, PhraseField::Content);
            assert_eq!(plan.slop, 0);
            assert_eq!(plan.candidate_cap, 10_000);
            assert_eq!(plan.trace.normalized_tokens, vec!["hello".to_string()]);
            assert!(plan.trace.early_stop_reason.is_none());
        }
    }

    #[test]
    fn empty_text_is_empty_phrase() {
        let opts = LqOptions::defaults();
        let outcome = plan_phrase(
            "   ",
            &opts,
            &PhrasePolicy::defaults(),
            PhraseField::Content,
        );
        assert_eq!(outcome, Err(PhrasePlannerError::EmptyPhrase));
    }

    #[test]
    fn case_sensitive_option_propagates() {
        let opts = opts_with_case(Some(LqCase::Sensitive));
        let outcome = plan_phrase(
            "Foo Bar",
            &opts,
            &PhrasePolicy::defaults(),
            PhraseField::Content,
        );
        assert!(outcome.is_ok(), "expected Ok, got {outcome:?}");
        if let Ok(plan) = outcome {
            assert!(plan.case_sensitive);
            // tokens preserved exactly; no ASCII lowercase fold under Sensitive.
            assert_eq!(plan.tokens, vec!["Foo".to_string(), "Bar".to_string()]);
        }
    }

    #[test]
    fn symbol_field_is_unsupported() {
        let opts = LqOptions::defaults();
        let outcome = plan_phrase(
            "anything",
            &opts,
            &PhrasePolicy::defaults(),
            PhraseField::Symbol,
        );
        match outcome {
            Err(PhrasePlannerError::UnsupportedField { field, reason }) => {
                assert_eq!(field, PhraseField::Symbol);
                assert_eq!(reason, "phrase on symbol field not yet supported");
            }
            other => assert!(false, "expected UnsupportedField, got {other:?}"),
        }
    }

    struct StaticLookup {
        hits: Vec<PhraseHit>,
    }

    impl PhrasePositionLookup for StaticLookup {
        fn lookup_phrase(&self, _plan: &PhrasePlan) -> Result<Vec<PhraseHit>, PhrasePlannerError> {
            Ok(self.hits.clone())
        }
    }

    #[test]
    fn execute_phrase_passes_through_lookup() {
        let plan_outcome = plan_phrase(
            "foo bar",
            &LqOptions::defaults(),
            &PhrasePolicy::defaults(),
            PhraseField::Content,
        );
        assert!(plan_outcome.is_ok(), "expected Ok, got {plan_outcome:?}");
        let Ok(plan) = plan_outcome else {
            return;
        };
        let lookup = StaticLookup {
            hits: vec![PhraseHit {
                doc_id: 7,
                start_position: 3,
                end_position: 4,
            }],
        };
        let exec_outcome = execute_phrase(&plan, &lookup);
        assert!(exec_outcome.is_ok(), "expected Ok, got {exec_outcome:?}");
        let Ok(hits) = exec_outcome else {
            return;
        };
        assert_eq!(hits.len(), 1);
        let Some(h) = hits.first().copied() else {
            assert!(false, "expected one hit");
            return;
        };
        assert_eq!(h.doc_id, 7);
        assert_eq!(h.start_position, 3);
        assert_eq!(h.end_position, 4);
    }

    #[test]
    fn trace_builder_records_execution_counters() {
        let mut trace = PhraseTraceBuilder::new(
            PhraseField::Content,
            vec!["foo".to_string(), "bar".to_string()],
        );
        trace.candidate_count = 12;
        trace.verify_count = 3;
        trace.early_stop_reason = Some(EarlyStopReason::CandidateCapHit);
        let snapshot = trace.build();
        assert_eq!(snapshot.candidate_count, 12);
        assert_eq!(snapshot.verify_count, 3);
        assert_eq!(
            snapshot.early_stop_reason,
            Some(EarlyStopReason::CandidateCapHit)
        );
        assert_eq!(snapshot.field, PhraseField::Content);
        assert_eq!(snapshot.normalized_tokens.len(), 2);
    }
}
