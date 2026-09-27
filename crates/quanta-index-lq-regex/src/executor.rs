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
//! 5. compute the structural planning charge via [`crate::estimate_nfa_states`];
//! 6. remove unobserved explicit captures, then compile with
//!    `regex::bytes::Regex::new`, wrapping `regex::Error`
//!    size refusals into [`RegexErrorCode::PlanLimitExceeded`] and other
//!    engine failures into [`RegexErrorCode::ExecutionInternal`].
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
use std::borrow::Cow;
use std::time::Instant;

use quanta_index_lq_trigram::{DocId, DocResolver};
use regex_syntax::hir::Hir;

use crate::dialect::{classify_ast_error, classify_construct_from_slice, dialect_filter};
use crate::dialect_ast_walk::ast_walk_filter;
use crate::errors::{LimitDimension, RegexError, RegexErrorCode};
use crate::estimator::estimate_nfa_states;
use crate::literal_extract::extract_prefilter_literal_alternation;

/// Compiled regex paired with its HIR so callers can re-run
/// literal extraction without re-parsing.
pub struct RegexExecutor {
    pattern: Box<str>,
    hir: Hir,
    compiled: regex::bytes::Regex,
}

/// Validated regex input before the engine allocates its automata.
///
/// Callers with an optional resource ledger can inspect the estimated state
/// count and refuse compilation while keeping the original query result.
pub struct RegexCompilationPlan {
    pattern: Box<str>,
    hir: Hir,
    execution_pattern: Box<str>,
    estimated_states: u64,
}

impl RegexCompilationPlan {
    /// The dialect estimator's state count, not an aggregate byte bound.
    #[must_use]
    pub const fn estimated_states(&self) -> u64 {
        self.estimated_states
    }

    /// Extract prefilter literals from validated HIR without allocating an engine.
    pub fn prefilter_literal_alternation(&self) -> Result<Vec<Vec<u8>>, RegexError> {
        extract_prefilter_literal_alternation(&self.hir)
    }
}

/// A bounded prefix of this executor's non-overlapping byte matches.
///
/// Empty ranges retain the engine's zero-width semantics. Consumers must not
/// turn one into a one-byte highlight or assume byte offsets are UTF-8 offsets.
#[derive(Debug, Eq, PartialEq)]
pub struct RegexRanges {
    pub ranges: Vec<core::ops::Range<usize>>,
    /// False when the range cap was reached; exhaustion was not established.
    pub exhausted: bool,
}

/// Optional range reconstruction stopped before producing a complete answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegexRangeError {
    SourceByteLimit,
    Interrupted,
}

impl core::fmt::Display for RegexRangeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::SourceByteLimit => "regex preview source-byte limit exceeded",
            Self::Interrupted => "regex preview interrupted",
        })
    }
}

impl std::error::Error for RegexRangeError {}

impl RegexExecutor {
    /// Run the full LEX-04 compile pipeline.
    ///
    /// Errors:
    ///
    /// - [`RegexErrorCode::ForbiddenSyntax`] — pattern uses a construct
    ///   forbidden by the LQ regex dialect;
    /// - [`RegexErrorCode::ParseFail`] — pattern fails RE2 syntax;
    /// - [`RegexErrorCode::PlanLimitExceeded`] — the structural planning
    ///   charge or compiled-engine byte ceiling is exceeded;
    /// - [`RegexErrorCode::ExecutionInternal`] — an engine construction
    ///   failure distinct from a resource limit.
    pub fn compile(pattern: &str) -> Result<Self, RegexError> {
        Self::compile_prepared(Self::prepare(pattern)?)
    }

    /// Validate and plan without allocating the regex engine's automata.
    pub fn prepare(pattern: &str) -> Result<RegexCompilationPlan, RegexError> {
        // Precise AST-level rejection of `(?>...)`, `\k<name>`, and
        // mid-pattern `(?i)` MUST run before `parse_hir`: the first two
        // surface as generic `FlagUnrecognized` / `EscapeUnrecognized`
        // parse errors (losing typed classification), and the third is
        // accepted unconditionally by `regex_syntax` so the HIR walk
        // cannot see it.
        ast_walk_filter(pattern)?;
        let hir = parse_hir(pattern)?;
        dialect_filter(&hir)?;
        let estimated_states = estimate_nfa_states(&hir)?;
        // Only boolean truth and whole-match ranges escape this executor.
        // Backreferences are forbidden, so explicit capture storage cannot
        // affect either result. Keeping it grows the engine's per-state cache
        // with every capture, even when the actual source focus is tiny.
        let execution_pattern = without_explicit_captures(pattern, &hir)?;
        Ok(RegexCompilationPlan {
            pattern: pattern.into(),
            hir,
            execution_pattern: execution_pattern.into_owned().into_boxed_str(),
            estimated_states,
        })
    }

    /// Compile an already validated plan without repeating dialect parsing.
    pub fn compile_prepared(plan: RegexCompilationPlan) -> Result<Self, RegexError> {
        let compiled = regex::bytes::Regex::new(&plan.execution_pattern).map_err(|e| {
            if let regex::Error::CompiledTooBig(limit) = e {
                RegexError::plan_limit(
                    LimitDimension::CompiledBytes,
                    format!("compiled regex exceeds engine byte ceiling {limit}"),
                )
            } else {
                RegexError::new(
                    RegexErrorCode::ExecutionInternal,
                    format!("validated regex failed engine construction: {e}"),
                )
            }
        })?;
        Ok(Self {
            pattern: plan.pattern,
            hir: plan.hir,
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

    /// Reconstruct ranges with the exact compiled matcher used by [`Self::verify`].
    ///
    /// The byte cap is checked before entering the regex engine. Interruption is
    /// checked before and after each search, including the no-match search. One
    /// engine search is not preemptible; its input is bounded by `max_source_bytes`.
    /// No truncated document is searched: that would change anchors and lookaround.
    /// Reaching the range cap is conservatively non-exhaustive without an extra
    /// unbudgeted search. No partial result escapes on interruption.
    pub fn find_ranges_bounded(
        &self,
        doc_text: &[u8],
        max_source_bytes: usize,
        max_ranges: usize,
        interrupted: &dyn Fn() -> bool,
    ) -> Result<RegexRanges, RegexRangeError> {
        if interrupted() {
            return Err(RegexRangeError::Interrupted);
        }
        if doc_text.len() > max_source_bytes {
            return Err(RegexRangeError::SourceByteLimit);
        }
        let mut ranges = Vec::new();
        let mut matches = self.compiled.find_iter(doc_text);
        while ranges.len() < max_ranges {
            if interrupted() {
                return Err(RegexRangeError::Interrupted);
            }
            let next = matches.next();
            if interrupted() {
                return Err(RegexRangeError::Interrupted);
            }
            let Some(found) = next else {
                return Ok(RegexRanges {
                    ranges,
                    exhausted: true,
                });
            };
            ranges.push(found.range());
        }
        Ok(RegexRanges {
            ranges,
            exhausted: false,
        })
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
    /// before every candidate and before returning a complete set, and stops with
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
        // Prefilter hits are only candidates. Reserving their entire count
        // amplifies memory even when none of them pass exact verification.
        let mut out: Vec<DocId> = Vec::new();
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
                out.try_reserve(1).map_err(|error| {
                    RegexError::new(
                        RegexErrorCode::ExecutionInternal,
                        format!("regex verified-result allocation refused: {error}"),
                    )
                })?;
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
        // The final resolver or matcher may observe cancellation after its
        // preceding check. Do not publish a complete set without checking the
        // request once more; this also covers an empty candidate list.
        if interrupted() {
            return Err(RegexError::new(
                RegexErrorCode::Interrupted,
                format!(
                    "regex verify interrupted before publishing {} candidate results",
                    candidates.len()
                ),
            ));
        }
        Ok(out)
    }
}

/// Drop capture storage that is unobservable through truth and whole-match APIs.
fn without_explicit_captures<'a>(pattern: &'a str, hir: &Hir) -> Result<Cow<'a, str>, RegexError> {
    if hir.properties().explicit_captures_len() == 0 {
        return Ok(Cow::Borrowed(pattern));
    }
    let mut ast = regex_syntax::ast::parse::Parser::new()
        .parse(pattern)
        .map_err(|error| {
            RegexError::new(
                RegexErrorCode::ExecutionInternal,
                format!("validated regex AST could not be reconstructed: {error}"),
            )
        })?;
    erase_capture_storage(&mut ast);
    Ok(Cow::Owned(ast.to_string()))
}

fn erase_capture_storage(ast: &mut regex_syntax::ast::Ast) {
    use regex_syntax::ast::{Ast, Flags, GroupKind};
    match ast {
        Ast::Group(group) => {
            if group.is_capturing() {
                group.kind = GroupKind::NonCapturing(Flags {
                    span: group.span,
                    items: Vec::new(),
                });
            }
            erase_capture_storage(&mut group.ast);
        }
        Ast::Repetition(repetition) => erase_capture_storage(&mut repetition.ast),
        Ast::Concat(concat) => {
            for child in &mut concat.asts {
                erase_capture_storage(child);
            }
        }
        Ast::Alternation(alternation) => {
            for child in &mut alternation.asts {
                erase_capture_storage(child);
            }
        }
        Ast::Empty(_)
        | Ast::Flags(_)
        | Ast::Literal(_)
        | Ast::Dot(_)
        | Ast::Assertion(_)
        | Ast::ClassUnicode(_)
        | Ast::ClassPerl(_)
        | Ast::ClassBracketed(_) => {}
    }
}

/// Parse `pattern` through `regex_syntax`, translating AST-stage
/// rejections into typed [`RegexErrorCode::ForbiddenSyntax`] where applicable.
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
                    format!(
                        "regex dialect rejection at byte {}: {}",
                        span.start.offset, a
                    ),
                );
            }
            if let Some(kind) = classify_construct_from_slice(slice) {
                return RegexError::forbidden(
                    kind,
                    format!(
                        "regex dialect rejection at byte {}: {}",
                        span.start.offset, a
                    ),
                );
            }
            RegexError::new(
                RegexErrorCode::ParseFail,
                format!("regex parse failed at byte {}: {}", span.start.offset, a),
            )
        }
        regex_syntax::Error::Translate(ref t) => RegexError::new(
            RegexErrorCode::ParseFail,
            format!("regex HIR translation failed: {t}"),
        ),
        // `regex_syntax::Error` is `#[non_exhaustive]`. Treat any future
        // variant as a generic parse failure rather than panicking.
        _ => RegexError::new(
            RegexErrorCode::ParseFail,
            format!("regex parse failed: {err}"),
        ),
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
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning range regressions propagate setup errors and assert byte-exact fixture oracles"
)]
mod tests {
    use super::RegexExecutor;
    use crate::errors::{ForbiddenKind, LimitDimension, RegexErrorCode};
    use quanta_index_lq_trigram::{DocId, DocResolver};
    use std::collections::BTreeMap;

    #[test]
    fn l4_capture_heavy_matcher_retains_only_whole_match_slots()
    -> Result<(), Box<dyn std::error::Error>> {
        // This valid expression has a six-byte focus but hundreds of optional
        // captures. The executor never exposes those capture groups; retaining
        // them makes the engine's per-state capture tables grow quadratically.
        let pattern = format!("needle{}", "(a?)".repeat(512));
        let executor = RegexExecutor::compile(&pattern)?;
        assert_eq!(executor.pattern(), pattern);
        assert_eq!(executor.compiled.captures_len(), 1);
        assert!(executor.verify(b"needle"));
        assert_eq!(
            executor
                .find_ranges_bounded(b"needle", 6, 2, &|| false)?
                .ranges,
            vec![0..6]
        );
        Ok(())
    }

    #[test]
    fn l4_unobserved_capture_removal_preserves_reference_ranges()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut patterns: Vec<String> = [
            "(ab|a)+",
            "(?i:(?<named>é))",
            "(x?)(y*)",
            "(?x)(a) # comment\n (b)",
            "((?:)?)",
            "(?i)(K)(ſ)",
            "(?m)(^a)(b?)",
            r"(\b)(needle)(\b)",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        for atom in ["a", "é", r"\w", "[a-z]", "(?:a|é)"] {
            patterns.push(format!("(({atom})?)"));
            patterns.push(format!("(?i:(?<hit>{atom}))({atom})?"));
        }
        for pattern in patterns {
            let reference = regex::bytes::Regex::new(&pattern)?;
            let executor = RegexExecutor::compile(&pattern)?;
            assert_eq!(executor.compiled.captures_len(), 1, "{pattern:?}");
            for source in [
                "",
                "a",
                "aa",
                "b",
                "ab",
                "aba",
                "é",
                "É",
                "e\u{301}",
                "café",
                "CAFE",
                "needle42",
                "needle",
                "xxyyy",
                "a\nb",
                "a\r\nb",
                "\u{212a}S",
                "İi",
                "ſK",
                "123_",
                "aaaaé",
            ] {
                let expected: Vec<_> = reference
                    .find_iter(source.as_bytes())
                    .map(|m| m.range())
                    .collect();
                let actual =
                    executor.find_ranges_bounded(source.as_bytes(), 128, 128, &|| false)?;
                assert!(actual.exhausted);
                assert_eq!(actual.ranges, expected, "{pattern:?} on {source:?}");
                assert_eq!(
                    executor.verify(source.as_bytes()),
                    reference.is_match(source.as_bytes()),
                    "{pattern:?} on {source:?}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn l4_ranges_share_case_and_pattern_semantics() -> Result<(), Box<dyn std::error::Error>> {
        let executor = RegexExecutor::compile("(?i)needle[0-9]+")?;
        let source = "é NEEDLE42 needle7";
        let found = executor.find_ranges_bounded(source.as_bytes(), 64, 3, &|| false)?;
        assert!(executor.verify(source.as_bytes()));
        assert_eq!(found.ranges, vec![3..11, 12..19]);
        assert!(found.exhausted);
        assert!(!executor.verify(b"needle"));
        assert!(
            executor
                .find_ranges_bounded(b"needle", 64, 3, &|| false)?
                .ranges
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn l4_range_cap_never_claims_exhaustion() -> Result<(), Box<dyn std::error::Error>> {
        let executor = RegexExecutor::compile("a")?;
        for source in [b"a".as_slice(), b"aaa".as_slice()] {
            let found = executor.find_ranges_bounded(source, 3, 1, &|| false)?;
            assert_eq!(found.ranges, vec![0..1]);
            assert!(!found.exhausted);
        }
        let zero = executor.find_ranges_bounded(b"a", 1, 0, &|| false)?;
        assert!(zero.ranges.is_empty());
        assert!(!zero.exhausted);
        assert_eq!(
            executor.find_ranges_bounded(b"aa", 1, 1, &|| false),
            Err(super::RegexRangeError::SourceByteLimit)
        );
        Ok(())
    }

    #[test]
    fn l4_ranges_preserve_zero_width_and_utf8_byte_coordinates()
    -> Result<(), Box<dyn std::error::Error>> {
        let executor = RegexExecutor::compile("^")?;
        let found = executor.find_ranges_bounded("é".as_bytes(), 2, 2, &|| false)?;
        assert_eq!(found.ranges, vec![0..0]);
        assert!(found.exhausted);
        let executor = RegexExecutor::compile("é")?;
        assert_eq!(
            executor
                .find_ranges_bounded("xé".as_bytes(), 3, 2, &|| false)?
                .ranges,
            vec![1..3]
        );
        Ok(())
    }

    #[test]
    fn l4_ranges_discard_partial_work_on_cancellation() -> Result<(), Box<dyn std::error::Error>> {
        let executor = RegexExecutor::compile("a")?;
        assert_eq!(
            executor.find_ranges_bounded(b"aaa", 3, 3, &|| true),
            Err(super::RegexRangeError::Interrupted)
        );
        let checks = std::cell::Cell::new(0_usize);
        let cancelled = || {
            checks.set(checks.get().saturating_add(1));
            checks.get() >= 5
        };
        assert_eq!(
            executor.find_ranges_bounded(b"aaa", 3, 3, &cancelled),
            Err(super::RegexRangeError::Interrupted)
        );
        // A deadline passing during the final no-match search is also observed.
        checks.set(0);
        let expired = || {
            checks.set(checks.get().saturating_add(1));
            checks.get() >= 3
        };
        assert_eq!(
            executor.find_ranges_bounded(b"bbb", 3, 3, &expired),
            Err(super::RegexRangeError::Interrupted)
        );
        Ok(())
    }

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
        assert_eq!(
            asked.get(),
            3,
            "the check is asked once per candidate until it answers true"
        );
        // A check that never answers true changes nothing.
        match exec.execute_interruptible(&cands, &m, 0, &|| false) {
            Ok(v) => assert_eq!(v, vec![DocId(1), DocId(2)]),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn l4_final_candidate_interruption_discards_verified_prefix()
    -> Result<(), Box<dyn std::error::Error>> {
        struct CancelOnResolve(std::cell::Cell<bool>);

        impl DocResolver for CancelOnResolve {
            fn resolve(&self, _: DocId) -> Option<&[u8]> {
                self.0.set(true);
                Some(b"needle")
            }
        }

        let executor = RegexExecutor::compile("needle")?;
        let resolver = CancelOnResolve(std::cell::Cell::new(false));
        let result =
            executor.execute_interruptible(&[DocId(1)], &resolver, 0, &|| resolver.0.get());
        assert!(
            matches!(result, Err(ref error) if error.code == RegexErrorCode::Interrupted),
            "a cancellation during the last candidate must discard its match: {result:?}"
        );
        assert!(resolver.0.get());

        let already_cancelled = executor.execute_interruptible(&[], &resolver, 0, &|| true);
        assert!(
            matches!(already_cancelled, Err(ref error) if error.code == RegexErrorCode::Interrupted)
        );
        Ok(())
    }

    #[test]
    fn l4_unmatched_prefilter_does_not_reserve_verified_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let executor = RegexExecutor::compile("unmatched-pattern")?;
        let candidates = [DocId(1), DocId(2), DocId(3), DocId(4)];
        let verified = executor.execute_with_budget(&candidates, &fixture(), 0)?;
        assert!(verified.is_empty());
        assert_eq!(verified.capacity(), 0);
        Ok(())
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
    #[test]
    fn engine_size_limit_is_a_typed_resource_failure() -> Result<(), Box<dyn std::error::Error>> {
        // This single Unicode range expands into enough UTF-8 states to hit
        // the engine's byte ceiling while its logical planning cost fits.
        let plan = RegexExecutor::prepare(r"[\x{80}-\x{10FFFF}]{20000}")?;
        assert!(plan.estimated_states() <= crate::MAX_NFA_STATES);
        let Err(error) = RegexExecutor::compile_prepared(plan) else {
            return Err("fixture must exceed the pinned engine size ceiling".into());
        };
        assert_eq!(error.code, RegexErrorCode::PlanLimitExceeded);
        assert_eq!(
            error.dimension.map(LimitDimension::as_code_str),
            Some("regex-compiled-bytes")
        );
        Ok(())
    }
}
