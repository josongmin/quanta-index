//! Positive selected-row witnesses over the authenticated source text.

use core::ops::Range;
use std::cell::Cell;

use quanta_index_contract::{
    LqExpr, LqFilter, LqLeaf, LqOptions, LqPatternType, PreviewUnavailableReason,
};
use quanta_index_lq_regex::{RegexExecutor, executor::RegexRangeError};
use quanta_index_lq_text_normalizer::{self as normalize, MappedText};

use super::{
    PositiveWitness, SnippetLimits, WitnessBudget, WitnessResult, WitnessStop,
    token_allocation_bound_bytes,
};

/// Bind regex case mode to the same normalized source used by the executor.
pub(crate) fn regex_source_for_options(source: &str, options: &LqOptions) -> String {
    let source = normalize::nfc(source);
    if options.case_mode() == normalize::CaseMode::Sensitive {
        source.into_owned()
    } else {
        format!("(?i){source}")
    }
}

/// Reconstruct selected-row witnesses through the existing matching owners.
///
/// False Boolean branches and NOT retain no positive evidence. This request-
/// local operation never caches whole-query truth under a leaf cache key.
pub(crate) fn selected_witnesses<'a, B: WitnessBudget>(
    budget: &B,
    expr: &LqExpr,
    filters: &[LqFilter],
    options: &LqOptions,
    limits: SnippetLimits,
    path: &str,
    include_path: bool,
    nfc: &MappedText<'_>,
    folded: &MappedText<'_>,
    document: &[normalize::Token],
    regex_executor: &dyn Fn(&str) -> Result<&'a RegexExecutor, B::Error>,
    predicate_truth: &dyn Fn(&LqLeaf) -> Result<bool, B::Error>,
) -> WitnessResult<(bool, bool, Vec<PositiveWitness>), B::Error> {
    let evaluator = WitnessMatcher {
        budget,
        path,
        limits,
        options,
        nfc,
        folded,
        document,
        regex_executor,
        predicate_truth,
        overflow: Cell::new(false),
        unsupported: Cell::new(false),
    };
    let mut witnesses = Vec::new();
    let (mut matched, path_match) = evaluator.evaluate(expr, include_path, 0, &mut witnesses)?;
    if matched {
        for filter in filters {
            if let quanta_index_contract::LqFilter::Content { leaf } = filter
                && !evaluator.leaf(leaf, false, &mut witnesses)?.0
            {
                matched = false;
                witnesses.clear();
                break;
            }
        }
    }
    if matched && evaluator.overflow.get() {
        return Err(WitnessStop::Unavailable(
            PreviewUnavailableReason::WorkBudget,
        ));
    }
    if matched && evaluator.unsupported.get() {
        return Err(WitnessStop::Unavailable(
            PreviewUnavailableReason::UnsupportedRange,
        ));
    }
    Ok((matched, path_match, witnesses))
}

struct WitnessMatcher<'a, 'b, 'c, B: WitnessBudget> {
    budget: &'a B,
    path: &'a str,
    options: &'a LqOptions,
    limits: SnippetLimits,
    nfc: &'a MappedText<'b>,
    folded: &'a MappedText<'b>,
    document: &'a [normalize::Token],
    regex_executor: &'a dyn Fn(&str) -> Result<&'c RegexExecutor, B::Error>,
    predicate_truth: &'a dyn Fn(&LqLeaf) -> Result<bool, B::Error>,
    overflow: Cell<bool>,
    unsupported: Cell<bool>,
}

impl<B: WitnessBudget> WitnessMatcher<'_, '_, '_, B> {
    fn token_bound(&self, bytes: usize) -> WitnessResult<usize, B::Error> {
        token_allocation_bound_bytes(bytes).ok_or(WitnessStop::Unavailable(
            PreviewUnavailableReason::WorkBudget,
        ))
    }

    fn evaluate(
        &self,
        expr: &LqExpr,
        include_path: bool,
        depth: usize,
        out: &mut Vec<PositiveWitness>,
    ) -> WitnessResult<(bool, bool), B::Error> {
        self.budget.charge(1)?;
        if depth > 64 {
            return Err(WitnessStop::Unavailable(
                PreviewUnavailableReason::WorkBudget,
            ));
        }
        match expr {
            LqExpr::Empty => Ok((true, false)),
            LqExpr::Leaf(leaf) => self.leaf(leaf, include_path, out),
            LqExpr::Not(inner) => {
                let saved = out.len();
                let flags = (self.overflow.get(), self.unsupported.get());
                let result = self.evaluate(inner, false, depth.saturating_add(1), out);
                out.truncate(saved);
                self.overflow.set(flags.0);
                self.unsupported.set(flags.1);
                result.map(|(matched, _)| (!matched, false))
            }
            LqExpr::All(children) => {
                let saved = out.len();
                let flags = (self.overflow.get(), self.unsupported.get());
                for child in children {
                    if !self.evaluate(child, false, depth.saturating_add(1), out)?.0 {
                        out.truncate(saved);
                        self.overflow.set(flags.0);
                        self.unsupported.set(flags.1);
                        return Ok((false, false));
                    }
                }
                Ok((true, false))
            }
            LqExpr::Any(children) => {
                let mut matched = false;
                for child in children {
                    let saved = out.len();
                    let flags = (self.overflow.get(), self.unsupported.get());
                    if self.evaluate(child, false, depth.saturating_add(1), out)?.0 {
                        matched = true;
                    } else {
                        out.truncate(saved);
                        self.overflow.set(flags.0);
                        self.unsupported.set(flags.1);
                    }
                }
                Ok((matched, false))
            }
        }
    }

    fn push(
        &self,
        map: &MappedText<'_>,
        range: Range<usize>,
        out: &mut Vec<PositiveWitness>,
    ) -> WitnessResult<(), B::Error> {
        if range.is_empty() || map.text().get(range.clone()).is_none() {
            self.unsupported.set(true);
            return Ok(());
        }
        if out.len() >= self.limits.witnesses {
            self.overflow.set(true);
            return Ok(());
        }
        let original = map
            .source_range(range.clone())
            .map_err(|error| self.budget.mapping_error(error))?;
        let normalized = map
            .normalized_range(range)
            .map_err(|error| self.budget.mapping_error(error))?;
        out.push(PositiveWitness {
            original,
            normalized,
        });
        Ok(())
    }

    fn gather(
        &self,
        map: &MappedText<'_>,
        ranges: impl Iterator<Item = Range<usize>>,
        out: &mut Vec<PositiveWitness>,
    ) -> WitnessResult<bool, B::Error> {
        // One extra range proves an incomplete witness set and declines the
        // optional preview. Never emit a partial highlight list as complete.
        let cap = self
            .limits
            .witnesses
            .saturating_sub(out.len())
            .saturating_add(1);
        let mut matched = false;
        for range in ranges.take(cap) {
            matched = true;
            self.push(map, range, out)?;
            if self.overflow.get() || self.unsupported.get() {
                break;
            }
        }
        Ok(matched)
    }

    fn leaf(
        &self,
        leaf: &LqLeaf,
        include_path: bool,
        out: &mut Vec<PositiveWitness>,
    ) -> WitnessResult<(bool, bool), B::Error> {
        let (text, regex) = match leaf {
            LqLeaf::Keyword(text) | LqLeaf::RawString(text) => {
                (text, self.options.pattern_type == LqPatternType::Regexp)
            }
            LqLeaf::Regex(text) => (text, true),
            LqLeaf::Phrase(text) => (text, false),
            LqLeaf::Predicate { .. } | LqLeaf::StructuralBlock(_) => {
                return (self.predicate_truth)(leaf)
                    .map(|matched| (matched, false))
                    .map_err(WitnessStop::Mandatory);
            }
        };
        if text.len() > self.limits.source_bytes {
            return Err(WitnessStop::Unavailable(
                PreviewUnavailableReason::WorkBudget,
            ));
        }
        self.budget
            .charge(self.nfc.text().len().saturating_add(text.len()))?;
        let _query_memory = self.budget.reserve(self.token_bound(text.len())?)?;
        if regex {
            let executor = (self.regex_executor)(text).map_err(WitnessStop::Mandatory)?;
            let expected = regex_source_for_options(text, self.options);
            if executor.pattern() != expected {
                return Err(WitnessStop::Mandatory(
                    self.budget
                        .integrity("regex executor has wrong pattern/case binding"),
                ));
            }
            let cap = self
                .limits
                .witnesses
                .saturating_sub(out.len())
                .saturating_add(1);
            let found = executor
                .find_ranges_bounded(
                    self.nfc.text().as_bytes(),
                    self.limits.transformed_bytes,
                    cap,
                    &|| self.budget.interrupted(),
                )
                .map_err(|error| match error {
                    RegexRangeError::SourceByteLimit | RegexRangeError::AllocationRefused => {
                        WitnessStop::Unavailable(PreviewUnavailableReason::WorkBudget)
                    }
                    RegexRangeError::Interrupted => {
                        WitnessStop::Mandatory(self.budget.interrupted_error())
                    }
                })?;
            return self
                .gather(self.nfc, found.ranges.into_iter(), out)
                .map(|matched| (matched, false));
        }
        if matches!(leaf, LqLeaf::RawString(_)) {
            return self
                .gather(self.folded, self.folded.find_substrings(text), out)
                .map(|matched| (matched, false));
        }
        let wanted = normalize::query_tokens(text, self.options.case_mode()).map_err(|error| {
            WitnessStop::Mandatory(
                self.budget
                    .integrity(&format!("prepared token query invalid: {error}")),
            )
        })?;
        self.budget
            .charge(self.document.len().saturating_mul(wanted.len()))?;
        if self.gather(
            self.nfc,
            normalize::phrase_ranges(self.document, &wanted),
            out,
        )? {
            return Ok((true, false));
        }
        if include_path && matches!(leaf, LqLeaf::Keyword(_)) {
            if self.path.len() > self.limits.source_bytes {
                return Err(WitnessStop::Unavailable(
                    PreviewUnavailableReason::WorkBudget,
                ));
            }
            self.budget.charge(self.path.len())?;
            let _path_memory = self.budget.reserve(self.token_bound(self.path.len())?)?;
            let path = normalize::tokenize(self.path, self.options.case_mode());
            let present: Vec<_> = path.indexable().cloned().collect();
            self.budget
                .charge(present.len().saturating_mul(wanted.len()))?;
            return Ok((normalize::contains_phrase(&present, &wanted), true));
        }
        Ok((false, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_lq_text_normalizer::MappingError;
    use std::{cell::Cell, rc::Rc};

    #[derive(Debug, Eq, PartialEq)]
    enum TestError {
        Cancelled,
        Unfunded,
        Integrity,
    }

    struct Grant {
        live: Rc<Cell<usize>>,
        bytes: usize,
    }

    impl Drop for Grant {
        fn drop(&mut self) {
            self.live.set(self.live.get() - self.bytes);
        }
    }

    #[derive(Default)]
    struct Budget {
        live: Rc<Cell<usize>>,
        cancelled: Cell<bool>,
        unfunded: Cell<bool>,
    }

    impl WitnessBudget for Budget {
        type Error = TestError;
        type Reservation = Grant;

        fn charge(&self, _work: usize) -> WitnessResult<(), TestError> {
            if self.cancelled.get() {
                Err(WitnessStop::Mandatory(TestError::Cancelled))
            } else if self.unfunded.get() {
                Err(WitnessStop::Mandatory(TestError::Unfunded))
            } else {
                Ok(())
            }
        }

        fn reserve(&self, bytes: usize) -> WitnessResult<Grant, TestError> {
            if self.unfunded.get() {
                return Err(WitnessStop::Mandatory(TestError::Unfunded));
            }
            self.live.set(self.live.get() + bytes);
            Ok(Grant {
                live: self.live.clone(),
                bytes,
            })
        }

        fn mapping_error(&self, error: MappingError) -> WitnessStop<TestError> {
            match error {
                MappingError::Interrupted => WitnessStop::Mandatory(TestError::Cancelled),
                MappingError::SourceMismatch | MappingError::InvalidSpan => {
                    WitnessStop::Mandatory(TestError::Integrity)
                }
                MappingError::ByteLimit
                | MappingError::EntryLimit
                | MappingError::AllocationRefused => {
                    WitnessStop::Unavailable(PreviewUnavailableReason::WorkBudget)
                }
            }
        }

        fn interrupted(&self) -> bool {
            self.cancelled.get()
        }

        fn interrupted_error(&self) -> TestError {
            TestError::Cancelled
        }

        fn integrity(&self, _message: &str) -> TestError {
            TestError::Integrity
        }
    }

    fn evaluate(
        budget: &Budget,
        expr: &LqExpr,
        raw: &str,
        limits: SnippetLimits,
        executor: Option<&RegexExecutor>,
    ) -> WitnessResult<(bool, bool, Vec<PositiveWitness>), TestError> {
        let options = LqOptions::defaults();
        let indexed = normalize::nfc(raw);
        let nfc = MappedText::new(
            raw,
            indexed.as_ref(),
            normalize::CaseMode::Sensitive,
            limits.transformed_bytes,
            limits.map_entries,
            &|| false,
        )
        .expect("fixed raw/NFC map");
        let folded = MappedText::new(
            raw,
            indexed.as_ref(),
            options.case_mode(),
            limits.transformed_bytes,
            limits.map_entries,
            &|| false,
        )
        .expect("fixed folded map");
        let tokenized = normalize::tokenize(indexed.as_ref(), options.case_mode());
        let document: Vec<_> = tokenized.indexable().cloned().collect();
        selected_witnesses(
            budget,
            expr,
            &[],
            &options,
            limits,
            "src/fixed.rs",
            false,
            &nfc,
            &folded,
            &document,
            &|_| executor.ok_or(TestError::Integrity),
            &|_| Err(TestError::Integrity),
        )
    }

    #[test]
    fn false_boolean_branches_leave_only_true_positive_evidence() {
        let budget = Budget::default();
        let keyword = |text: &str| LqExpr::Leaf(LqLeaf::Keyword(text.into()));
        let expr = LqExpr::Any(vec![
            LqExpr::All(vec![keyword("present"), keyword("missing")]),
            LqExpr::Not(Box::new(keyword("present"))),
            keyword("anchor"),
        ]);
        let result = evaluate(
            &budget,
            &expr,
            "present anchor",
            SnippetLimits::default(),
            None,
        );
        let (matched, path, witnesses) = result.unwrap_or_else(|_| panic!("fixed witness refused"));
        assert!(matched);
        assert!(!path);
        assert_eq!(witnesses.len(), 1);
        assert_eq!(witnesses[0].original, 8..14);
        assert_eq!(budget.live.get(), 0);
    }

    #[test]
    fn nfc_range_and_overlapping_raw_ranges_retain_source_bytes() {
        let budget = Budget::default();
        let expr = LqExpr::Leaf(LqLeaf::Keyword("café".into()));
        let (_, _, witnesses) = evaluate(
            &budget,
            &expr,
            "🙂cafe\u{301}",
            SnippetLimits::default(),
            None,
        )
        .unwrap_or_else(|_| panic!("NFC witness refused"));
        assert_eq!(witnesses[0].original, 4..10);
        assert_eq!(witnesses[0].normalized, 4..9);
        let raw = LqExpr::Leaf(LqLeaf::RawString("aba".into()));
        let (_, _, overlaps) = evaluate(&budget, &raw, "ababa", SnippetLimits::default(), None)
            .unwrap_or_else(|_| panic!("overlap witness refused"));
        assert_eq!(
            overlaps
                .iter()
                .map(|w| w.original.clone())
                .collect::<Vec<_>>(),
            vec![0..3, 2..5]
        );
        assert_eq!(budget.live.get(), 0);
    }

    #[test]
    fn regex_executor_case_binding_is_the_same_for_legacy_and_native_callers() {
        let budget = Budget::default();
        let expr = LqExpr::Leaf(LqLeaf::Regex("needle[0-9]+".into()));
        let options = LqOptions::defaults();
        let expected = regex_source_for_options("needle[0-9]+", &options);
        let correct = RegexExecutor::compile(&expected).expect("fixed regex");
        let (_, _, witnesses) = evaluate(
            &budget,
            &expr,
            "NEEDLE42",
            SnippetLimits::default(),
            Some(&correct),
        )
        .unwrap_or_else(|_| panic!("case-bound regex refused"));
        assert_eq!(witnesses[0].original, 0..8);
        let wrong = RegexExecutor::compile("needle[0-9]+").expect("fixed wrong-case regex");
        assert!(matches!(
            evaluate(
                &budget,
                &expr,
                "NEEDLE42",
                SnippetLimits::default(),
                Some(&wrong)
            ),
            Err(WitnessStop::Mandatory(TestError::Integrity))
        ));
        assert_eq!(budget.live.get(), 0);
    }

    #[test]
    fn witness_cap_and_native_typed_budget_refusals_release_reservations() {
        let budget = Budget::default();
        let expr = LqExpr::Leaf(LqLeaf::RawString("a".into()));
        let limits = SnippetLimits {
            witnesses: 1,
            ..SnippetLimits::default()
        };
        assert!(matches!(
            evaluate(&budget, &expr, "aaa", limits, None),
            Err(WitnessStop::Unavailable(
                PreviewUnavailableReason::WorkBudget
            ))
        ));
        assert_eq!(budget.live.get(), 0);
        budget.cancelled.set(true);
        assert!(matches!(
            evaluate(&budget, &expr, "aaa", limits, None),
            Err(WitnessStop::Mandatory(TestError::Cancelled))
        ));
        budget.cancelled.set(false);
        budget.unfunded.set(true);
        assert!(matches!(
            evaluate(&budget, &expr, "aaa", limits, None),
            Err(WitnessStop::Mandatory(TestError::Unfunded))
        ));
        assert_eq!(budget.live.get(), 0);
    }
}
