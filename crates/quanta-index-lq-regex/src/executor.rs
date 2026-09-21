//! Regex-executor entry point — compiles a pattern through the LEX-04
//! pipeline and exposes verification + cooperative-cancel iteration.
//!
//! Compile path:
//!
//! 1. run [`crate::dialect_ast_walk::ast_walk_filter`] FIRST — this fires
//!    typed [`crate::errors::ForbiddenKind::Possessive`],
//!    [`crate::errors::ForbiddenKind::NamedCaptureRef`], and
//!    [`crate::errors::ForbiddenKind::InlineFlagMidPattern`] before the
//!    HIR parse can either accept them (mid-pattern `(?i)`) or surface
//!    them with a generic untyped error (possessive / named-capture-ref);
//! 2. parse with `regex_syntax::parse`;
//! 3. on AST-stage rejection, classify the construct via
//!    [`crate::dialect::classify_ast_error`] and surface
//!    [`RegexErrorCode::ForbiddenSyntax`] when applicable; otherwise
//!    [`RegexErrorCode::ParseFail`];
//! 4. run [`crate::dialect::dialect_filter`] over the HIR;
//! 5. estimate NFA states via [`crate::estimate_nfa_states`];
//! 6. compile with `regex::bytes::Regex::new`, wrapping `regex::Error`
//!    into [`RegexErrorCode::ExecutionInternal`].
//!
//! Verify path: [`RegexExecutor::verify`] calls
//! `regex::bytes::Regex::is_match` against a single document's bytes.
//!
//! Cooperative cancel: [`RegexExecutor::execute_with_budget`] iterates
//! a candidate list, polling elapsed wall time after each candidate;
//! exceed the budget → [`RegexErrorCode::QueryTimeout`].
//! [`RegexExecutor::execute_interruptible`] additionally asks the caller's
//! interruption check between candidates and stops with
//! [`RegexErrorCode::Interrupted`] when it answers `true`.

use core::time::Duration;
use std::time::Instant;

use quanta_index_lq_trigram::{DocId, DocResolver};
use regex_syntax::hir::Hir;

use crate::dialect::{classify_ast_error, classify_construct_from_slice, dialect_filter};
use crate::dialect_ast_walk::ast_walk_filter;
use crate::errors::{RegexError, RegexErrorCode};
use crate::estimator::estimate_nfa_states;
use crate::literal_extract::extract_prefilter_literal_alternation;

/// Compiled regex paired with its HIR so callers can re-run
/// literal extraction without re-parsing.
pub struct RegexExecutor {
    pattern: Box<str>,
    hir: Hir,
    compiled: regex::bytes::Regex,
}

impl RegexExecutor {
    /// Run the full LEX-04 compile pipeline.
    ///
    /// Errors:
    ///
    /// - [`RegexErrorCode::ForbiddenSyntax`] — pattern uses a construct
    ///   forbidden by the LQ regex dialect;
    /// - [`RegexErrorCode::ParseFail`] — pattern fails RE2 syntax;
    /// - [`RegexErrorCode::PlanLimitExceeded`] — estimator overshoots the
    ///   NFA budget;
    /// - [`RegexErrorCode::ExecutionInternal`] — `regex::Regex::new`
    ///   internal-budget overshoot despite the planner-time estimator.
    pub fn compile(pattern: &str) -> Result<Self, RegexError> {
        // Precise AST-level rejection of `(?>...)`, `\k<name>`, and
        // mid-pattern `(?i)` MUST run before `parse_hir`: the first two
        // surface as generic `FlagUnrecognized` / `EscapeUnrecognized`
        // parse errors (losing typed classification), and the third is
        // accepted unconditionally by `regex_syntax` so the HIR walk
        // cannot see it.
        ast_walk_filter(pattern)?;
        let hir = parse_hir(pattern)?;
        dialect_filter(&hir)?;
        let _estimated: u64 = estimate_nfa_states(&hir)?;
        let compiled = regex::bytes::Regex::new(pattern).map_err(|e| {
            RegexError::new(
                RegexErrorCode::ExecutionInternal,
                format!("regex::Regex::new rejected pattern: {e}"),
            )
        })?;
        Ok(Self {
            pattern: pattern.into(),
            hir,
            compiled,
        })
    }

    /// Borrow the source pattern.
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Borrow the compiled HIR.
    #[must_use]
    pub fn hir(&self) -> &Hir {
        &self.hir
    }

    /// Run the trigram-prefilter literal extractor on the compiled HIR.
    ///
    /// The result is an alternation: a match must contain one of the returned
    /// literals, not all of them. See
    /// [`extract_prefilter_literal_alternation`].
    pub fn prefilter_literal_alternation(&self) -> Result<Vec<Vec<u8>>, RegexError> {
        extract_prefilter_literal_alternation(&self.hir)
    }

    /// Verify a single document's bytes against the compiled regex.
    #[must_use]
    pub fn verify(&self, doc_text: &[u8]) -> bool {
        self.compiled.is_match(doc_text)
    }

    /// Iterate `candidates`, verify each via [`RegexExecutor::verify`],
    /// and yield the verified [`DocId`]s.
    ///
    /// Budget cooperation:
    ///
    /// - `budget_ms == 0` → no budget enforcement (caller-owned timeout
    ///   regime);
    /// - elapsed wall time after each candidate exceeds `budget_ms` →
    ///   [`RegexErrorCode::QueryTimeout`].
    pub fn execute_with_budget(
        &self,
        candidates: &[DocId],
        corpus: &dyn DocResolver,
        budget_ms: u64,
    ) -> Result<Vec<DocId>, RegexError> {
        self.execute_interruptible(candidates, corpus, budget_ms, &|| false)
    }

    /// [`RegexExecutor::execute_with_budget`] that also asks `interrupted`
    /// before every candidate and stops with
    /// [`RegexErrorCode::Interrupted`] once it answers `true`.
    ///
    /// The check is the caller's request budget (a peer that left, a
    /// deadline that passed); the executor knows nothing of why and says
    /// only where it stopped, so the caller can name the checkpoint.
    pub fn execute_interruptible(
        &self,
        candidates: &[DocId],
        corpus: &dyn DocResolver,
        budget_ms: u64,
        interrupted: &dyn Fn() -> bool,
    ) -> Result<Vec<DocId>, RegexError> {
        let started = Instant::now();
        let budget = if budget_ms == 0 {
            None
        } else {
            Some(Duration::from_millis(budget_ms))
        };
        let mut out: Vec<DocId> = Vec::with_capacity(candidates.len());
        for (index, cand) in candidates.iter().enumerate() {
            if interrupted() {
                return Err(RegexError::new(
                    RegexErrorCode::Interrupted,
                    format!(
                        "regex verify interrupted by the caller before candidate {index} of {}",
                        candidates.len()
                    ),
                ));
            }
            let bytes = corpus.resolve(*cand).ok_or_else(|| {
                RegexError::new(
                    RegexErrorCode::ExecutionInternal,
                    format!("resolver missing doc {cand}"),
                )
            })?;
            if self.verify(bytes) {
                out.push(*cand);
            }
            if let Some(b) = budget {
                let elapsed = started.elapsed();
                if elapsed > b {
                    return Err(RegexError::new(
                        RegexErrorCode::QueryTimeout,
                        format!(
                            "regex verify budget {}ms exceeded after {}ms",
                            budget_ms,
                            elapsed.as_millis()
                        ),
                    ));
                }
            }
        }
        Ok(out)
    }
}

/// Parse `pattern` through `regex_syntax`, translating AST-stage
/// rejections into typed [`RegexErrorCode::ForbiddenSyntax`] where
/// applicable.
fn parse_hir(pattern: &str) -> Result<Hir, RegexError> {
    match regex_syntax::parse(pattern) {
        Ok(h) => Ok(h),
        Err(e) => Err(classify_parse_error(pattern, &e)),
    }
}

/// Map a `regex_syntax::Error` to a typed [`RegexError`].
fn classify_parse_error(pattern: &str, err: &regex_syntax::Error) -> RegexError {
    match *err {
        regex_syntax::Error::Parse(ref a) => {
            let span = a.span();
            let slice = span_slice(pattern, span);
            if let Some(kind) = classify_ast_error(a.kind(), slice) {
                return RegexError::forbidden(
                    kind,
                    format!("regex dialect rejection at byte {}: {}", span.start.offset, a),
                );
            }
            if let Some(kind) = classify_construct_from_slice(slice) {
                return RegexError::forbidden(
                    kind,
                    format!("regex dialect rejection at byte {}: {}", span.start.offset, a),
                );
            }
            RegexError::new(
                RegexErrorCode::ParseFail,
                format!("regex parse failed at byte {}: {}", span.start.offset, a),
            )
        }
        regex_syntax::Error::Translate(ref t) => {
            RegexError::new(RegexErrorCode::ParseFail, format!("regex HIR translation failed: {t}"))
        }
        // `regex_syntax::Error` is `#[non_exhaustive]`. Treat any future
        // variant as a generic parse failure rather than panicking.
        _ => RegexError::new(RegexErrorCode::ParseFail, format!("regex parse failed: {err}")),
    }
}

/// Extract the offending span's text via a manual byte walk.
///
/// Manual byte indexing keeps us clear of the `string_slice` clippy
/// denial. Invalid spans return the empty string rather than a `Result`
/// since the caller only uses the slice for heuristic classification.
fn span_slice<'a>(pattern: &'a str, span: &regex_syntax::ast::Span) -> &'a str {
    let bytes = pattern.as_bytes();
    let start = span.start.offset;
    let end = span.end.offset;
    if end <= start || end > bytes.len() {
        return "";
    }
    let Some(window) = bytes.get(start..end) else {
        return "";
    };
    core::str::from_utf8(window).map_or("", |v| v)
}

#[cfg(test)]
mod tests {
    use super::RegexExecutor;
    use crate::errors::{ForbiddenKind, RegexErrorCode};
    use quanta_index_lq_trigram::{DocId, DocResolver};
    use std::collections::BTreeMap;

    struct Map(BTreeMap<DocId, Vec<u8>>);

    impl DocResolver for Map {
        fn resolve(&self, doc_id: DocId) -> Option<&[u8]> {
            self.0.get(&doc_id).map(Vec::as_slice)
        }
    }

    fn fixture() -> Map {
        let mut m: BTreeMap<DocId, Vec<u8>> = BTreeMap::new();
        let docs: &[(DocId, &[u8])] = &[
            (DocId(1), b"fn handle_request() {}"),
            (DocId(2), b"fn handle_response() {}"),
            (DocId(3), b"fn other() {}"),
            (DocId(4), b"struct Handler {}"),
        ];
        for (d, bytes) in docs {
            let prior = m.insert(*d, bytes.to_vec());
            assert!(prior.is_none());
        }
        Map(m)
    }

    #[test]
    fn compile_plain_literal() {
        match RegexExecutor::compile("foo") {
            Ok(_) => {}
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn compile_rejects_lookbehind() {
        match RegexExecutor::compile("(?<=foo)bar") {
            Ok(_) => assert!(false, "expected FORBIDDEN_SYNTAX"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::Lookbehind));
            }
        }
    }

    #[test]
    fn compile_rejects_lookahead() {
        match RegexExecutor::compile("foo(?=bar)") {
            Ok(_) => assert!(false, "expected FORBIDDEN_SYNTAX"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::Lookahead));
            }
        }
    }

    #[test]
    fn compile_rejects_negative_lookahead() {
        match RegexExecutor::compile("foo(?!bar)") {
            Ok(_) => assert!(false, "expected FORBIDDEN_SYNTAX"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::Lookahead));
            }
        }
    }

    #[test]
    fn compile_rejects_backref() {
        match RegexExecutor::compile(r"(foo)\1") {
            Ok(_) => assert!(false, "expected FORBIDDEN_SYNTAX"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::Backref));
            }
        }
    }

    #[test]
    fn compile_rejects_syntax_error() {
        match RegexExecutor::compile("foo(") {
            Ok(_) => assert!(false, "expected PARSE_FAIL"),
            Err(e) => assert_eq!(e.code, RegexErrorCode::ParseFail),
        }
    }

    #[test]
    fn compile_rejects_possessive_group_precisely() {
        match RegexExecutor::compile("(?>abc)") {
            Ok(_) => assert!(false, "expected FORBIDDEN_SYNTAX(possessive)"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::Possessive));
            }
        }
    }

    #[test]
    fn compile_rejects_named_capture_ref_precisely() {
        match RegexExecutor::compile(r"\b\k<x>\b") {
            Ok(_) => assert!(false, "expected FORBIDDEN_SYNTAX(named-capture-ref)"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::NamedCaptureRef));
            }
        }
    }

    #[test]
    fn compile_rejects_mid_pattern_inline_flag() {
        match RegexExecutor::compile("foo(?i)bar") {
            Ok(_) => assert!(false, "expected FORBIDDEN_SYNTAX(inline-flag-midpattern)"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::ForbiddenSyntax);
                assert_eq!(e.forbidden, Some(ForbiddenKind::InlineFlagMidPattern));
            }
        }
    }

    #[test]
    fn compile_accepts_leading_inline_flag() {
        // Leading `(?i)` is canonicalization-friendly per LEX-04 spec
        // §3.4 — PRE-NORM strips it before tokenizer handoff. The
        // executor must accept it so that the canonicalization round-
        // trip is invisible to callers.
        match RegexExecutor::compile("(?i)foo") {
            Ok(_) => {}
            Err(e) => assert!(false, "expected leading `(?i)` to be accepted, got {e}"),
        }
    }

    #[test]
    fn compile_accepts_scoped_inline_flag_group() {
        // `(?i:foo)` is a scoped non-capturing group; this is a
        // different AST shape from a set-flag and remains allowed.
        match RegexExecutor::compile("(?i:foo)") {
            Ok(_) => {}
            Err(e) => assert!(false, "expected `(?i:foo)` to be accepted, got {e}"),
        }
    }

    #[test]
    fn compile_rejects_nfa_explosion() {
        // Unicode-wide `\w` class * 1000 repetition pushes the
        // conservative estimator past the 100k cap.
        match RegexExecutor::compile(r"\w{0,1000}") {
            Ok(_) => assert!(false, "expected PLAN_LIMIT_EXCEEDED"),
            Err(e) => assert_eq!(e.code, RegexErrorCode::PlanLimitExceeded),
        }
    }

    #[test]
    fn verify_matches_handler_pattern() {
        let exec = match RegexExecutor::compile(r"fn\s+handle_\w+") {
            Ok(x) => x,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert!(exec.verify(b"fn handle_request() {}"));
        assert!(exec.verify(b"fn handle_response() {}"));
        assert!(!exec.verify(b"fn other() {}"));
        assert!(!exec.verify(b"struct Handler {}"));
    }

    #[test]
    fn execute_with_budget_returns_verified() {
        let exec = match RegexExecutor::compile(r"fn\s+handle_\w+") {
            Ok(x) => x,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let m = fixture();
        let cands = vec![DocId(1), DocId(2), DocId(3), DocId(4)];
        match exec.execute_with_budget(&cands, &m, 0) {
            Ok(v) => assert_eq!(v, vec![DocId(1), DocId(2)]),
            Err(e) => assert!(false, "{e}"),
        }
    }

    /// The interruption check is asked before every candidate, so a check
    /// that turns true after `n` answers stops the verify at candidate `n`
    /// with the verified prefix discarded and the stop named.
    #[test]
    fn execute_interruptible_stops_at_the_first_true_check() {
        let exec = match RegexExecutor::compile(r"fn\s+handle_\w+") {
            Ok(x) => x,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let m = fixture();
        let cands = vec![DocId(1), DocId(2), DocId(3), DocId(4)];
        let asked = std::cell::Cell::new(0_usize);
        let interrupted = || {
            asked.set(asked.get().saturating_add(1));
            asked.get() > 2
        };
        match exec.execute_interruptible(&cands, &m, 0, &interrupted) {
            Ok(v) => assert!(false, "expected INTERRUPTED, got {v:?}"),
            Err(e) => {
                assert_eq!(e.code, RegexErrorCode::Interrupted);
                assert!(
                    e.detail.contains("before candidate 2 of 4"),
                    "the stop is named: {}",
                    e.detail
                );
            }
        }
        assert_eq!(asked.get(), 3, "the check is asked once per candidate until it answers true");
        // A check that never answers true changes nothing.
        match exec.execute_interruptible(&cands, &m, 0, &|| false) {
            Ok(v) => assert_eq!(v, vec![DocId(1), DocId(2)]),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn execute_with_budget_resolver_missing_is_typed() {
        let exec = match RegexExecutor::compile("foo") {
            Ok(x) => x,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let m = Map(BTreeMap::new());
        let cands = vec![DocId(42)];
        match exec.execute_with_budget(&cands, &m, 0) {
            Ok(_) => assert!(false, "expected EXECUTION_INTERNAL"),
            Err(e) => assert_eq!(e.code, RegexErrorCode::ExecutionInternal),
        }
    }

    #[test]
    fn prefilter_literal_alternation_typed_for_pure_wildcard() {
        let exec = match RegexExecutor::compile(".*") {
            Ok(x) => x,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        match exec.prefilter_literal_alternation() {
            Ok(v) => assert!(false, "expected REGEX_PREFILTER_UNUSABLE, got {v:?}"),
            Err(e) => assert_eq!(e.code, RegexErrorCode::RegexPrefilterUnusable),
        }
    }
}
