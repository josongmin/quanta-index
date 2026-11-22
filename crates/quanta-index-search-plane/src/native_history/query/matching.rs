//! Bounded History text predicates over the original query DATA budget.

use std::cell::RefCell;
use std::ops::ControlFlow;

use quanta_index_contract::{LqLeaf, LqOptions};
use quanta_index_core::{
    CoreError,
    index::{MemoryLease, MemoryLifetime, WorkBudget, WorkUsage},
};
use quanta_index_lq_text_normalizer::{
    BoundedNfcError, CaseMode, MAX_TOKEN_BYTES, TextQueryError, TokenCursor, apply_case,
    case_chars, nfc, query_tokens,
};

use super::history_work_error;
use crate::query_dispatcher::routes::history_records::HistoryTextMatcher;

pub(super) struct BoundedHistoryMatcher<'a> {
    budget: &'a mut WorkBudget,
}

impl<'a> BoundedHistoryMatcher<'a> {
    pub(super) fn new(budget: &'a mut WorkBudget) -> Self {
        Self { budget }
    }
}

impl HistoryTextMatcher for BoundedHistoryMatcher<'_> {
    fn begin_row(&mut self) -> Result<(), CoreError> {
        self.budget
            .charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })
            .map_err(|error| history_work_error(self.budget, error))
    }

    fn reserve_diff_text(&mut self, bytes: u64) -> Result<Option<MemoryLease>, CoreError> {
        self.budget
            .charge(WorkUsage {
                bytes,
                ..WorkUsage::default()
            })
            .map_err(|error| history_work_error(self.budget, error))?;
        self.budget
            .reserve_memory(bytes, MemoryLifetime::Temporary)
            .map_err(|error| history_work_error(self.budget, error))
    }

    fn leaf_matches(
        &mut self,
        leaf: &LqLeaf,
        text: &str,
        options: &LqOptions,
    ) -> Result<bool, CoreError> {
        let case = options.case_mode();
        match leaf {
            LqLeaf::Keyword(literal) | LqLeaf::Phrase(literal) => {
                let wanted = query_tokens(literal, case).map_err(|err| CoreError::Typed {
                    code: match err {
                        TextQueryError::NoTokens => {
                            quanta_index_contract::SearchPlaneErrorCodeV2::LexTextQueryNoTokens
                        }
                        TextQueryError::TokenTooLong { .. } => {
                            quanta_index_contract::SearchPlaneErrorCodeV2::LexTextQueryTokenTooLong
                        }
                    },
                    message: format!("history: {err}"),
                })?;
                let mut prefix = vec![0_usize; wanted.len()];
                for i in 1..wanted.len() {
                    let mut matched = prefix[i - 1];
                    while matched > 0 && wanted[i].text != wanted[matched].text {
                        matched = prefix[matched - 1];
                    }
                    if wanted[i].text == wanted[matched].text {
                        matched += 1;
                    }
                    prefix[i] = matched;
                }
                let mut matched = 0_usize;
                let found = visit_history_tokens(text, case, self.budget, |run| {
                    let Some(run) = run else {
                        matched = 0; // An overlong run still occupies a position.
                        return Ok(ControlFlow::Continue(()));
                    };
                    while matched > 0 && run != wanted[matched].text {
                        matched = prefix[matched - 1];
                    }
                    if run == wanted[matched].text {
                        matched += 1;
                    }
                    if matched == wanted.len() {
                        return Ok(ControlFlow::Break(()));
                    }
                    Ok(ControlFlow::Continue(()))
                })?;
                Ok(found.is_break())
            }
            LqLeaf::RawString(literal) => self.matches(literal, text, options),
            LqLeaf::Regex(_) => Err(CoreError::NotImplemented(
                "history: regex leaves are not executable on the current adapter set".into(),
            )),
            LqLeaf::StructuralBlock(_) => Err(CoreError::NotImplemented(
                "history: structural leaves are not executable on this route".into(),
            )),
            LqLeaf::Predicate { .. } => Err(CoreError::NotImplemented(
                "history: predicate leaves are not executable on this route".into(),
            )),
        }
    }

    fn matches(
        &mut self,
        needle: &str,
        haystack: &str,
        options: &LqOptions,
    ) -> Result<bool, CoreError> {
        if needle.is_empty() {
            return Ok(false);
        }
        let case = options.case_mode();
        let normalized_needle = nfc(needle);
        let folded_needle = apply_case(normalized_needle.as_ref(), case);
        let pattern: Vec<char> = folded_needle.chars().collect();
        if pattern.is_empty() {
            return Ok(false);
        }
        let mut prefix = vec![0_usize; pattern.len()];
        for i in 1..pattern.len() {
            let mut matched = prefix[i - 1];
            while matched > 0 && pattern[i] != pattern[matched] {
                matched = prefix[matched - 1];
            }
            if pattern[i] == pattern[matched] {
                matched += 1;
            }
            prefix[i] = matched;
        }
        let cursor = normalized_cursor(haystack, self.budget)?;
        let mut matched = 0_usize;
        let mut checked = 0_usize;
        for ch in case_chars(cursor.text(), case) {
            checked += 1;
            if checked == 4096 {
                self.budget
                    .checkpoint()
                    .map_err(|error| history_work_error(self.budget, error))?;
                checked = 0;
            }
            while matched > 0 && ch != pattern[matched] {
                matched = prefix[matched - 1];
            }
            if ch == pattern[matched] {
                matched += 1;
            }
            if matched == pattern.len() {
                break;
            }
        }
        self.budget
            .checkpoint()
            .map_err(|error| history_work_error(self.budget, error))?;
        Ok(matched == pattern.len())
    }
}

/// Visit NFC token positions until the callback requests a break. `None`
/// preserves a long-run gap and `Some`
/// borrows a folded, indexable term only for this callback invocation. A
/// successful break still checks the original request before returning.
pub(super) fn visit_history_tokens(
    text: &str,
    case: CaseMode,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(Option<&str>) -> Result<ControlFlow<()>, CoreError>,
) -> Result<ControlFlow<()>, CoreError> {
    let mut cursor = normalized_cursor(text, budget)?;
    // Lowercase yields at most three Unicode scalars per input scalar.
    // Runs longer than the term cap remain gaps and never need this buffer.
    let mut folded = [0_u8; MAX_TOKEN_BYTES * 3 * 4];
    let progress = loop {
        let Some(run) = cursor.try_next_run_with_checkpoint(|| {
            budget
                .checkpoint()
                .map_err(|error| history_work_error(budget, error))
        })?
        else {
            break ControlFlow::Continue(());
        };
        let progress = if !run.is_indexable() {
            visit(None)?
        } else if case == CaseMode::Sensitive
            || (run.text.is_ascii() && !run.text.bytes().any(|byte| byte.is_ascii_uppercase()))
        {
            visit(Some(run.text))?
        } else {
            let mut length = 0;
            for scalar in case_chars(run.text, case) {
                let mut bytes = [0_u8; 4];
                let encoded = scalar.encode_utf8(&mut bytes).as_bytes();
                let next = length + encoded.len();
                folded
                    .get_mut(length..next)
                    .ok_or_else(|| super::invalid("History folded term exceeds term backing"))?
                    .copy_from_slice(encoded);
                length = next;
            }
            let term = std::str::from_utf8(&folded[..length])
                .map_err(|error| super::invalid(&error.to_string()))?;
            visit(Some(term))?
        };
        if progress.is_break() {
            break progress;
        }
    };
    budget
        .checkpoint()
        .map_err(|error| history_work_error(budget, error))?;
    Ok(progress)
}

fn normalized_cursor<'a>(
    text: &'a str,
    budget: &mut WorkBudget,
) -> Result<TokenCursor<'a, Option<MemoryLease>>, CoreError> {
    let owner = RefCell::new(budget);
    TokenCursor::try_new_bounded(
        text,
        usize::MAX,
        usize::MAX,
        |bytes| {
            let mut work = owner.borrow_mut();
            work.charge(WorkUsage {
                bytes,
                ..WorkUsage::default()
            })
            .map_err(|error| history_work_error(&**work, error))
        },
        |bytes| {
            let work = owner.borrow();
            work.reserve_memory(
                u64::try_from(bytes).map_err(|_| {
                    CoreError::InvalidContract("History normalization memory overflow".into())
                })?,
                MemoryLifetime::Temporary,
            )
            .map_err(|error| history_work_error(&**work, error))
        },
    )
    .map_err(
        |error: BoundedNfcError<CoreError>| match error.into_checkpoint() {
            Ok(error) => error,
            Err(error) => {
                CoreError::InvalidContract(format!("History normalization failed: {error}"))
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{LqCase, LqOptions};
    use quanta_index_core::index::WorkLimits;
    use quanta_index_lq_text_normalizer::{contains_phrase, contains_substring, tokenize};
    use std::time::{Duration, Instant};

    fn budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                mutations: 100_000,
                bytes: 100_000_000,
            },
            Instant::now() + Duration::from_secs(30),
        )
    }

    #[test]
    fn bounded_history_matcher_keeps_token_gaps_and_nfc_substring_truth() -> Result<(), CoreError> {
        let options = LqOptions::defaults();
        let sensitive = LqOptions {
            case: Some(LqCase::Sensitive),
            ..LqOptions::defaults()
        };
        let phrases = vec![
            ("Fix typo".to_owned(), "fix".to_owned()),
            ("prefix".to_owned(), "fix".to_owned()),
            ("cafe\u{301} needle".to_owned(), "café needle".to_owned()),
            ("a bc a bc".to_owned(), "a bc".to_owned()),
            (format!("a {} b", "x".repeat(257)), "a b".to_owned()),
        ];
        for (text, literal) in phrases {
            let expected = contains_phrase(
                &tokenize(&text, options.case_mode())
                    .indexable()
                    .cloned()
                    .collect::<Vec<_>>(),
                &query_tokens(&literal, options.case_mode())
                    .map_err(|error| CoreError::InvalidContract(error.to_string()))?,
            );
            let mut work = budget();
            let actual = BoundedHistoryMatcher::new(&mut work).leaf_matches(
                &LqLeaf::Phrase(literal),
                &text,
                &options,
            )?;
            assert_eq!(actual, expected, "text={text:?}");
        }
        for (needle, haystack, opts) in [
            ("fé", "cafe\u{301}", &options),
            ("", "anything", &options),
            ("CAFÉ", "café", &options),
            ("CAFÉ", "café", &sensitive),
            ("i\u{307}", "İ", &options),
        ] {
            let expected = contains_substring(nfc(haystack).as_ref(), needle, opts.case_mode());
            let mut work = budget();
            let actual = BoundedHistoryMatcher::new(&mut work).matches(needle, haystack, opts)?;
            assert_eq!(actual, expected, "needle={needle:?} haystack={haystack:?}");
        }
        Ok(())
    }

    #[test]
    fn token_visitation_reports_long_run_gap_and_observes_cancellation() -> Result<(), CoreError> {
        let mut work = budget();
        let mut runs = Vec::new();
        let completion = visit_history_tokens(
            &format!("a {} b", "x".repeat(257)),
            CaseMode::Folded,
            &mut work,
            |run| {
                runs.push(run.map(str::to_owned));
                Ok(ControlFlow::Continue(()))
            },
        )?;
        assert_eq!(completion, ControlFlow::Continue(()));
        assert_eq!(runs, vec![Some("a".into()), None, Some("b".into())]);
        let mut cancelled = budget();
        let cancel = cancelled.request().cancel_handle();
        let text = "a ".repeat(16_384);
        let mut visited = 0;
        assert!(
            visit_history_tokens(&text, CaseMode::Folded, &mut cancelled, |_| {
                visited += 1;
                cancel.cancel();
                Ok(ControlFlow::Continue(()))
            })
            .is_err()
        );
        assert!((1..16_384).contains(&visited));
        assert_eq!(
            cancelled.first_denial(),
            Some(quanta_index_core::index::WorkBudgetDenial::Cancelled)
        );
        let mut early = budget();
        let mut visited = 0;
        let stopped = visit_history_tokens(&text, CaseMode::Folded, &mut early, |_| {
            visited += 1;
            Ok(ControlFlow::Break(()))
        })?;
        assert!(stopped.is_break());
        assert_eq!(visited, 1);
        let mut cancelled_at_break = budget();
        let cancel = cancelled_at_break.request().cancel_handle();
        assert!(
            visit_history_tokens(
                "first second",
                CaseMode::Folded,
                &mut cancelled_at_break,
                |_| {
                    cancel.cancel();
                    Ok(ControlFlow::Break(()))
                },
            )
            .is_err()
        );
        assert_eq!(
            cancelled_at_break.first_denial(),
            Some(quanta_index_core::index::WorkBudgetDenial::Cancelled)
        );
        Ok(())
    }
}
