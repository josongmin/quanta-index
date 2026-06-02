use std::time::Instant;

use crate::corpus::{Corpus, CorpusRow, ExpectedShape, Gate};
use crate::errors::ConformanceError;
use crate::runner::normalizer_trait::{CandidateShape, ConformanceExecutor, LqQueryNormalizer};

/// Per-row terminal verdict.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// Row exercised end-to-end and matched the expected shape.
    Pass,
    /// Row exercised but the observed result diverged.
    Fail {
        /// Human-readable reason the row failed.
        reason: String,
    },
    /// Row was gated `pending` / `blocked` — execution intentionally
    /// skipped, ticket recorded.
    Pending {
        /// Ticket id from the row's `gating_ticket`.
        ticket: String,
    },
    /// Row's expected error code fired exactly as declared.
    ExpectedError {
        /// The closed-set code the pipeline produced.
        code: ConformanceError,
    },
    /// Pipeline produced an error the row did not declare.
    UnexpectedError {
        /// Observed code rendered as `SCREAMING_SNAKE_CASE`.
        observed: String,
    },
}

/// One row's outcome with attribution back to the source row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RowOutcome {
    /// Row id, copied from the source [`CorpusRow`].
    pub row_id: String,
    /// Verdict the runner reached.
    pub verdict: Verdict,
    /// Wall-clock microseconds spent on this row. Always recorded,
    /// even for [`Verdict::Pending`] rows.
    pub elapsed_us: u64,
}

/// Aggregate counts across the whole report.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReportSummary {
    /// Total rows considered.
    pub total: u32,
    /// Count of [`Verdict::Pass`].
    pub passed: u32,
    /// Count of [`Verdict::Fail`].
    pub failed: u32,
    /// Count of [`Verdict::Pending`].
    pub pending: u32,
    /// Count of [`Verdict::ExpectedError`].
    pub expected_error: u32,
    /// Count of [`Verdict::UnexpectedError`].
    pub unexpected_error: u32,
    /// Sum of `elapsed_us` across all rows.
    pub total_elapsed_us: u64,
}

/// Full report — row-level outcomes plus aggregate summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Report {
    /// Per-row outcomes in corpus order.
    pub rows: Vec<RowOutcome>,
    /// Aggregate counts.
    pub summary: ReportSummary,
}

impl Report {
    /// Whether the report is overall green — i.e. zero failures and
    /// zero unexpected errors. [`Verdict::Pending`] rows are tolerated;
    /// they're the explicit "not yet" signal, not a failure.
    #[must_use]
    pub const fn is_green(&self) -> bool {
        self.summary.failed == 0 && self.summary.unexpected_error == 0
    }
}

/// Run the corpus end-to-end. Pure function over the supplied
/// normalizer and executor traits — no IO.
pub fn run_corpus<N, E>(corpus: &Corpus, normalizer: &N, executor: &E) -> Report
where
    N: LqQueryNormalizer,
    E: ConformanceExecutor<N::Normalized>,
{
    let mut rows: Vec<RowOutcome> = Vec::with_capacity(corpus.rows.len());
    // Saturate the row count into u32 explicitly. The runner does
    // not refuse to run a > 4-billion-row corpus, but the summary's
    // u32 counter caps at u32::MAX — every row still gets a verdict;
    // only the aggregate counter saturates.
    let total = u32::try_from(corpus.rows.len()).map_or(u32::MAX, |n| n);
    let mut summary = ReportSummary {
        total,
        ..ReportSummary::default()
    };

    for row in &corpus.rows {
        let outcome = run_row(row, normalizer, executor);
        match &outcome.verdict {
            Verdict::Pass => summary.passed = summary.passed.saturating_add(1),
            Verdict::Fail { .. } => summary.failed = summary.failed.saturating_add(1),
            Verdict::Pending { .. } => summary.pending = summary.pending.saturating_add(1),
            Verdict::ExpectedError { .. } => {
                summary.expected_error = summary.expected_error.saturating_add(1);
            }
            Verdict::UnexpectedError { .. } => {
                summary.unexpected_error = summary.unexpected_error.saturating_add(1);
            }
        }
        summary.total_elapsed_us = summary.total_elapsed_us.saturating_add(outcome.elapsed_us);
        rows.push(outcome);
    }

    Report { rows, summary }
}

fn run_row<N, E>(row: &CorpusRow, norm: &N, exec: &E) -> RowOutcome
where
    N: LqQueryNormalizer,
    E: ConformanceExecutor<N::Normalized>,
{
    let started = Instant::now();
    let verdict = match &row.gate {
        Gate::Pending { gating_ticket } | Gate::Blocked { gating_ticket } => Verdict::Pending {
            ticket: gating_ticket.clone(),
        },
        Gate::Active => evaluate_active_row(row, norm, exec),
    };
    // Wall-clock micros into u64. On the extraordinarily unlikely
    // overflow (~292 thousand years), the counter saturates — the
    // verdict itself is unaffected.
    let elapsed_us = u64::try_from(started.elapsed().as_micros()).map_or(u64::MAX, |n| n);
    RowOutcome {
        row_id: row.id.clone(),
        verdict,
        elapsed_us,
    }
}

fn evaluate_active_row<N, E>(row: &CorpusRow, norm: &N, exec: &E) -> Verdict
where
    N: LqQueryNormalizer,
    E: ConformanceExecutor<N::Normalized>,
{
    let normalized = match norm.parse_and_normalize(&row.query) {
        Ok(q) => q,
        Err(err) => {
            let observed = norm.classify(&err);
            return classify_error_against_expected(&observed, &row.expected);
        }
    };

    let executed = match exec.execute(&normalized, &row.expected) {
        Ok(shape) => shape,
        Err(err) => {
            let observed = exec.classify(&err);
            return classify_error_against_expected(&observed, &row.expected);
        }
    };

    compare_shape(&executed, &row.expected)
}

fn compare_shape(observed: &CandidateShape, expected: &ExpectedShape) -> Verdict {
    match expected {
        ExpectedShape::Error { code } => Verdict::Fail {
            reason: format!(
                "expected error `{}` but executor returned a successful shape",
                code.as_code_str(),
            ),
        },
        ExpectedShape::Empty => match observed {
            CandidateShape::Empty => Verdict::Pass,
            CandidateShape::Single
            | CandidateShape::Multi { .. }
            | CandidateShape::Paginated { .. } => Verdict::Fail {
                reason: format!("expected empty, observed {observed:?}"),
            },
        },
        ExpectedShape::Single => match observed {
            CandidateShape::Single => Verdict::Pass,
            CandidateShape::Empty
            | CandidateShape::Multi { .. }
            | CandidateShape::Paginated { .. } => Verdict::Fail {
                reason: format!("expected single, observed {observed:?}"),
            },
        },
        ExpectedShape::Multi { min, max } => match observed {
            CandidateShape::Multi { count } => check_multi_bounds(*count, *min, *max),
            CandidateShape::Single => check_multi_bounds(1, *min, *max),
            CandidateShape::Empty => check_multi_bounds(0, *min, *max),
            CandidateShape::Paginated { .. } => Verdict::Fail {
                reason: format!("expected multi, observed {observed:?}"),
            },
        },
        ExpectedShape::Paginated { page_size } => match observed {
            CandidateShape::Paginated {
                page_size: observed_size,
            } if *observed_size == *page_size => Verdict::Pass,
            CandidateShape::Paginated {
                page_size: observed_size,
            } => Verdict::Fail {
                reason: format!(
                    "expected paginated page_size={page_size}, observed {observed_size}",
                ),
            },
            CandidateShape::Empty | CandidateShape::Single | CandidateShape::Multi { .. } => {
                Verdict::Fail {
                    reason: format!("expected paginated, observed {observed:?}"),
                }
            }
        },
    }
}

fn check_multi_bounds(count: u32, min: u32, max: Option<u32>) -> Verdict {
    if count < min {
        return Verdict::Fail {
            reason: format!("expected multi >= {min}, observed {count}"),
        };
    }
    if let Some(maxv) = max
        && count > maxv
    {
        return Verdict::Fail {
            reason: format!("expected multi <= {maxv}, observed {count}"),
        };
    }
    Verdict::Pass
}

fn classify_error_against_expected(
    observed: &ConformanceError,
    expected: &ExpectedShape,
) -> Verdict {
    match expected {
        ExpectedShape::Error { code } if code == observed => Verdict::ExpectedError {
            code: observed.clone(),
        },
        ExpectedShape::Error { code } => Verdict::Fail {
            reason: format!(
                "expected error `{}` but observed `{}`",
                code.as_code_str(),
                observed.as_code_str(),
            ),
        },
        ExpectedShape::Empty
        | ExpectedShape::Single
        | ExpectedShape::Multi { .. }
        | ExpectedShape::Paginated { .. } => Verdict::UnexpectedError {
            observed: observed.as_code_str().to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::corpus::{Corpus, CorpusRow, ExpectedShape, Gate};
    use crate::mocks::{MockExecutor, MockNormalizer, MockResponse};

    fn corpus_of(rows: Vec<CorpusRow>) -> Corpus {
        Corpus {
            rows,
            source_path: PathBuf::from("test.toml"),
        }
    }

    fn active_row(id: &str, query: &str, expected: ExpectedShape) -> CorpusRow {
        CorpusRow {
            id: id.to_owned(),
            query: query.to_owned(),
            gate: Gate::Active,
            expected,
            persona: None,
            engines: Vec::new(),
            filters: Vec::new(),
            syntax: None,
            classification: None,
            runtime_route: None,
            fixture: None,
            expected_ids: None,
            top_k: None,
            runtime_error_code: None,
            runtime_error_message_contains: None,
            expected_engines_touched: Vec::new(),
            expected_summary_substrings: Vec::new(),
            expected_paths: None,
            expected_snippets: None,
            expected_bindings: None,
        }
    }

    /// First outcome's verdict, or assertion failure on empty.
    ///
    /// The fallback branch never returns — `assert!(false, ..)`
    /// panics the test — but Rust still needs a typed value, so we
    /// supply [`Verdict::Pass`] as the type-level filler.
    fn first_verdict(report: &Report) -> Verdict {
        report.rows.first().map_or_else(
            || {
                assert!(false, "expected at least one row outcome");
                Verdict::Pass
            },
            |r| r.verdict.clone(),
        )
    }

    #[test]
    fn pass_when_multi_in_bounds() {
        let row = active_row("R1", "fooBar", ExpectedShape::Multi { min: 1, max: None });
        let corpus = corpus_of(vec![row]);
        let norm = MockNormalizer::default();
        let exec = MockExecutor::new().with(
            "fooBar",
            MockResponse::Ok(CandidateShape::Multi { count: 3 }),
        );
        let report = run_corpus(&corpus, &norm, &exec);
        assert_eq!(first_verdict(&report), Verdict::Pass);
        assert_eq!(report.summary.passed, 1);
        assert!(report.is_green());
    }

    #[test]
    fn fail_when_multi_below_min() {
        let row = active_row("R1", "fooBar", ExpectedShape::Multi { min: 5, max: None });
        let corpus = corpus_of(vec![row]);
        let norm = MockNormalizer::default();
        let exec = MockExecutor::new().with(
            "fooBar",
            MockResponse::Ok(CandidateShape::Multi { count: 2 }),
        );
        let report = run_corpus(&corpus, &norm, &exec);
        assert!(matches!(first_verdict(&report), Verdict::Fail { .. }));
        assert_eq!(report.summary.failed, 1);
        assert!(!report.is_green());
    }

    #[test]
    fn pending_row_short_circuits() {
        let mut row = active_row("R1", "fooBar", ExpectedShape::Single);
        row.gate = Gate::Pending {
            gating_ticket: "LEX-01".to_owned(),
        };
        let corpus = corpus_of(vec![row]);
        let norm = MockNormalizer::default();
        let exec = MockExecutor::new();
        let report = run_corpus(&corpus, &norm, &exec);
        assert_eq!(
            first_verdict(&report),
            Verdict::Pending {
                ticket: "LEX-01".to_owned(),
            }
        );
        assert_eq!(report.summary.pending, 1);
    }

    #[test]
    fn blocked_row_short_circuits() {
        let mut row = active_row("R1", "fooBar", ExpectedShape::Single);
        row.gate = Gate::Blocked {
            gating_ticket: "RT-01".to_owned(),
        };
        let corpus = corpus_of(vec![row]);
        let norm = MockNormalizer::default();
        let exec = MockExecutor::new();
        let report = run_corpus(&corpus, &norm, &exec);
        assert_eq!(
            first_verdict(&report),
            Verdict::Pending {
                ticket: "RT-01".to_owned(),
            }
        );
    }

    #[test]
    fn expected_error_matched_from_normalizer() {
        let row = active_row(
            "R1",
            "@bad",
            ExpectedShape::Error {
                code: ConformanceError::ParseError,
            },
        );
        let corpus = corpus_of(vec![row]);
        let norm = MockNormalizer::default().fail_on("@bad", ConformanceError::ParseError);
        let exec = MockExecutor::new();
        let report = run_corpus(&corpus, &norm, &exec);
        assert_eq!(
            first_verdict(&report),
            Verdict::ExpectedError {
                code: ConformanceError::ParseError,
            }
        );
        assert_eq!(report.summary.expected_error, 1);
    }

    #[test]
    fn expected_error_matched_from_executor() {
        let row = active_row(
            "R1",
            "oversized",
            ExpectedShape::Error {
                code: ConformanceError::OversizedRequest,
            },
        );
        let corpus = corpus_of(vec![row]);
        let norm = MockNormalizer::default();
        let exec = MockExecutor::new().with(
            "oversized",
            MockResponse::Err(ConformanceError::OversizedRequest),
        );
        let report = run_corpus(&corpus, &norm, &exec);
        assert_eq!(
            first_verdict(&report),
            Verdict::ExpectedError {
                code: ConformanceError::OversizedRequest,
            }
        );
    }

    #[test]
    fn unexpected_error_when_executor_fails_on_success_row() {
        let row = active_row("R1", "fooBar", ExpectedShape::Single);
        let corpus = corpus_of(vec![row]);
        let norm = MockNormalizer::default();
        let exec = MockExecutor::new().with(
            "fooBar",
            MockResponse::Err(ConformanceError::TimeoutExceeded),
        );
        let report = run_corpus(&corpus, &norm, &exec);
        assert_eq!(
            first_verdict(&report),
            Verdict::UnexpectedError {
                observed: "TIMEOUT_EXCEEDED".to_owned(),
            }
        );
        assert!(!report.is_green());
    }

    #[test]
    fn unexpected_success_on_error_row() {
        let row = active_row(
            "R1",
            "fooBar",
            ExpectedShape::Error {
                code: ConformanceError::ParseError,
            },
        );
        let corpus = corpus_of(vec![row]);
        let norm = MockNormalizer::default();
        let exec = MockExecutor::new().with("fooBar", MockResponse::Ok(CandidateShape::Single));
        let report = run_corpus(&corpus, &norm, &exec);
        assert!(matches!(first_verdict(&report), Verdict::Fail { .. }));
    }

    #[test]
    fn wrong_error_code_is_fail_not_expected() {
        let row = active_row(
            "R1",
            "x",
            ExpectedShape::Error {
                code: ConformanceError::ParseError,
            },
        );
        let corpus = corpus_of(vec![row]);
        let norm = MockNormalizer::default().fail_on("x", ConformanceError::OversizedRequest);
        let exec = MockExecutor::new();
        let report = run_corpus(&corpus, &norm, &exec);
        assert!(matches!(first_verdict(&report), Verdict::Fail { .. }));
    }
}
