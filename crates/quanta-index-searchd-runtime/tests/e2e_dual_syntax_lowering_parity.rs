//! E2E-02 — Dual-syntax lowering parity.
//!
//! Two syntaxes (Sourcegraph + Native) must lower to identical lexical IR
//! and produce identical query results when executed against the same
//! generation. There is no external Sourcegraph reference: this matrix
//! exercises self-parity between the two surface syntaxes that both feed
//! into this daemon's single active lexical request lowering path.
//!
//! Each row pairs a Sourcegraph-syntax query and an equivalent native LQ
//! query. The runner issues both against the same sealed corpus and
//! asserts:
//!
//! 1. Both syntaxes return the **same** observed shape (same corpus ids in
//!    the same order, OR the same typed error code).
//! 2. The shape matches the row's `ExpectedOutcome` against the *current*
//!    behavior (closed-loop: rows marked `ExpectedFailing` stay GREEN
//!    while their `current_observation` continues to hold; when the owner
//!    ticket lands and behavior diverges, the row flips RED and must be
//!    promoted).
//!
//! Any divergence between the two syntaxes for a given row is a parity
//! violation regardless of the wiring state of the underlying feature.

#![forbid(unsafe_code)]

#[path = "common/e2e_harness.rs"]
mod e2e_harness;

use anyhow::Result as AnyResult;
use quanta_index_contract::TextQuerySyntax;
use std::fmt::Write as _;

use crate::e2e_harness::{E2eQueryResult, E2eRuntime};

/// Per-row expected outcome.
///
/// Variants are added only as live rows exist for them (CLAUDE.md "dead
/// port surface is forbidden"). When a parity row graduates to fully-green
/// candidates or a row currently fails with a typed error, add the
/// corresponding variant in the same PR.
#[derive(Clone, Debug)]
enum ExpectedOutcome {
    /// Both syntaxes must reject with this typed error code.
    TypedError { code: &'static str },
    /// Wiring pending. Both syntaxes must currently exhibit
    /// `current_observation`. When the owner ticket lands, parity may still
    /// hold but the behavior changes — the assertion goes red and the row
    /// must be updated.
    ExpectedFailing {
        owner_ticket: &'static str,
        reason: &'static str,
        current_observation: CurrentObservation,
    },
}

#[derive(Clone, Debug)]
enum CurrentObservation {
    Empty,
    OverbroadIncludes(&'static [&'static str]),
}

struct ParityScenario {
    id: &'static str,
    sg_query: &'static str,
    lq_query: &'static str,
    top_k: u32,
    expected: ExpectedOutcome,
}

struct CorpusRow {
    id: &'static str,
    path: &'static str,
    content: &'static str,
}

const CORPUS: &[CorpusRow] = &[
    CorpusRow {
        id: "alpha_rust",
        path: "src/lib.rs",
        content: "fn parity_needle_alpha() {}",
    },
    CorpusRow {
        id: "beta_py",
        path: "scripts/helper.py",
        content: "def parity_needle_alpha(): pass",
    },
    CorpusRow {
        id: "gamma_md",
        path: "docs/intro.md",
        content: "parity documentation lives here",
    },
    CorpusRow {
        id: "delta_other_path",
        path: "src/other.rs",
        content: "fn parity_needle_alpha() {}",
    },
    CorpusRow {
        id: "epsilon_regex",
        path: "src/version.rs",
        content: "const VERSION: &str = \"v9.8.7\";",
    },
    CorpusRow {
        id: "zeta_raw",
        path: "src/raw.rs",
        content: "let parity_foo_bar_baz = 1;",
    },
    CorpusRow {
        id: "theta_symbol",
        path: "src/sym.rs",
        content: "pub fn ParityTypeSymbol(arg: i32) -> i32 { arg }",
    },
];

const SCENARIOS: &[ParityScenario] = &[
    ParityScenario {
        id: "repo_filter_parity",
        sg_query: "repo:repo-other parity_needle_alpha",
        lq_query: "repo:repo-other parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-03",
            reason: "repo: filter must exclude all rows when no repo matches; both syntaxes are silently overbroad today",
            current_observation: CurrentObservation::OverbroadIncludes(&[
                "alpha_rust",
                "beta_py",
                "delta_other_path",
            ]),
        },
    },
    ParityScenario {
        id: "file_filter_parity",
        sg_query: "file:src/lib.rs parity_needle_alpha",
        lq_query: "file:src/lib.rs parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-03",
            reason: "file: filter must narrow content hits to src/lib.rs only; today both syntaxes are overbroad",
            current_observation: CurrentObservation::OverbroadIncludes(&[
                "alpha_rust",
                "beta_py",
                "delta_other_path",
            ]),
        },
    },
    ParityScenario {
        id: "lang_filter_parity",
        sg_query: "lang:rust parity_needle_alpha",
        lq_query: "lang:rust parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-03",
            reason: "lang: filter must restrict to rust-only matches; today both syntaxes are overbroad",
            current_observation: CurrentObservation::OverbroadIncludes(&[
                "alpha_rust",
                "beta_py",
                "delta_other_path",
            ]),
        },
    },
    ParityScenario {
        id: "case_filter_parity",
        sg_query: "case:yes PARITY_NEEDLE_ALPHA",
        lq_query: "case:yes PARITY_NEEDLE_ALPHA",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-03",
            reason: "case:yes must drop the case-folded matches; today neither syntax honors case",
            current_observation: CurrentObservation::OverbroadIncludes(&[
                "alpha_rust",
                "beta_py",
                "delta_other_path",
            ]),
        },
    },
    ParityScenario {
        id: "count_parity",
        sg_query: "count:1 parity_needle_alpha",
        lq_query: "count:1 parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-03",
            reason: "count:1 must cap to 1 deterministic top-N result for both syntaxes",
            current_observation: CurrentObservation::OverbroadIncludes(&[
                "alpha_rust",
                "beta_py",
                "delta_other_path",
            ]),
        },
    },
    ParityScenario {
        id: "type_file_parity",
        sg_query: "type:file parity_needle_alpha",
        lq_query: "type:file parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-06",
            reason: "type:file must route to file engine for both syntaxes; today filter is ignored",
            current_observation: CurrentObservation::OverbroadIncludes(&[
                "alpha_rust",
                "beta_py",
                "delta_other_path",
            ]),
        },
    },
    ParityScenario {
        id: "type_symbol_parity",
        sg_query: "type:symbol ParityTypeSymbol",
        lq_query: "type:symbol ParityTypeSymbol",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-06",
            reason: "type:symbol must route to symbol engine for both syntaxes",
            current_observation: CurrentObservation::Empty,
        },
    },
    ParityScenario {
        id: "select_file_parity",
        sg_query: "select:file parity_needle_alpha",
        lq_query: "select:file parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-06",
            reason: "select:file must collapse to per-file aggregation for both syntaxes",
            current_observation: CurrentObservation::OverbroadIncludes(&[
                "alpha_rust",
                "beta_py",
                "delta_other_path",
            ]),
        },
    },
    ParityScenario {
        id: "select_content_parity",
        sg_query: "select:content parity_needle_alpha",
        lq_query: "select:content parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-06",
            reason: "select:content must return content carrier for both syntaxes",
            current_observation: CurrentObservation::OverbroadIncludes(&[
                "alpha_rust",
                "beta_py",
                "delta_other_path",
            ]),
        },
    },
    ParityScenario {
        id: "select_symbol_parity",
        sg_query: "select:symbol ParityTypeSymbol",
        lq_query: "select:symbol ParityTypeSymbol",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-06",
            reason: "select:symbol must narrow to symbol carrier for both syntaxes",
            current_observation: CurrentObservation::Empty,
        },
    },
    ParityScenario {
        id: "patterntype_literal_raw_substring_parity",
        // The SG `patterntype:literal` plus a substring crossing a `_`
        // boundary must route to raw substring; LQ `raw:"foo_bar"` is the
        // equivalent native form.
        sg_query: "patterntype:literal foo_bar",
        lq_query: "raw:\"foo_bar\"",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-04",
            reason: "patterntype:literal must route to raw substring + trigram + verify; today neither syntax executes the raw substring path",
            current_observation: CurrentObservation::Empty,
        },
    },
    ParityScenario {
        id: "patterntype_regexp_parity",
        sg_query: "patterntype:regexp v\\d+\\.\\d+\\.\\d+",
        lq_query: "/v\\d+\\.\\d+\\.\\d+/",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-04",
            reason: "regex must route through lq-trigram + lq-regex verify for both syntaxes; today the escape path at lexical/src/lib.rs:1151 differs",
            current_observation: CurrentObservation::Empty,
        },
    },
    ParityScenario {
        id: "boolean_or_parity",
        sg_query: "parity_needle_alpha or documentation",
        lq_query: "parity_needle_alpha OR documentation",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-02",
            reason: "boolean OR must union for both syntaxes; planner IR pending",
            current_observation: CurrentObservation::Empty,
        },
    },
    ParityScenario {
        id: "negation_parity",
        sg_query: "parity_needle_alpha -helper",
        lq_query: "parity_needle_alpha NOT helper",
        top_k: 10,
        expected: ExpectedOutcome::ExpectedFailing {
            owner_ticket: "LXE-02",
            reason: "negation must exclude matching docs for both syntaxes; planner IR pending",
            current_observation: CurrentObservation::Empty,
        },
    },
    ParityScenario {
        id: "unsupported_sg_repohasfile_typed_error",
        // `repohasfile:` is not in SgFilter (lq-bridge/src/syntax.rs:47+),
        // so the translator must reject with BRIDGE_UNSUPPORTED_FILTER.
        // The equivalent LQ form must also be rejected (no native
        // `repohasfile:` filter exists, so the parser fails or the
        // dispatcher rejects).
        sg_query: "repohasfile:README.md parity_needle_alpha",
        lq_query: "repohasfile:README.md parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::TypedError {
            code: "BRIDGE_UNSUPPORTED_FILTER",
        },
    },
];

fn ingest_corpus(rt: &mut E2eRuntime) -> AnyResult<()> {
    for row in CORPUS {
        rt.ingest_text("repo-e2e", row.path, row.content)?;
    }
    Ok(())
}

fn corpus_id_for_candidate_id(candidate_id: &str) -> Option<&'static str> {
    let rest = candidate_id.strip_prefix("e2e-")?;
    let (_, path) = rest.split_once('-')?;
    CORPUS.iter().find(|row| row.path == path).map(|row| row.id)
}

fn observed_corpus_ids(result: &E2eQueryResult) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = result
        .candidates
        .iter()
        .filter_map(|c| corpus_id_for_candidate_id(&c.candidate_id))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn raw_candidate_ids(result: &E2eQueryResult) -> Vec<String> {
    result
        .candidates
        .iter()
        .map(|c| c.candidate_id.clone())
        .collect()
}

#[derive(Debug)]
struct Observation {
    corpus_ids: Vec<&'static str>,
    typed_error_code: Option<String>,
}

fn observe(result: &E2eQueryResult) -> Observation {
    Observation {
        corpus_ids: observed_corpus_ids(result),
        typed_error_code: result.typed_error.as_ref().map(|e| e.code.clone()),
    }
}

struct RowReport {
    id: &'static str,
    failure: Option<String>,
}

fn assess(
    scenario: &ParityScenario,
    sg_result: &E2eQueryResult,
    lq_result: &E2eQueryResult,
) -> RowReport {
    let sg = observe(sg_result);
    let lq = observe(lq_result);

    // Parity check first — both syntaxes must produce the same observable
    // shape regardless of whether the row is green or expected-failing.
    if sg.corpus_ids != lq.corpus_ids || sg.typed_error_code != lq.typed_error_code {
        return RowReport {
            id: scenario.id,
            failure: Some(format!(
                "[parity violation] sg observed={{ids: {:?}, err: {:?}, raw: {:?}}} vs lq observed={{ids: {:?}, err: {:?}, raw: {:?}}}",
                sg.corpus_ids,
                sg.typed_error_code,
                raw_candidate_ids(sg_result),
                lq.corpus_ids,
                lq.typed_error_code,
                raw_candidate_ids(lq_result),
            )),
        };
    }

    match &scenario.expected {
        ExpectedOutcome::TypedError { code } => match &sg.typed_error_code {
            Some(observed) if observed == code => RowReport {
                id: scenario.id,
                failure: None,
            },
            Some(observed) => RowReport {
                id: scenario.id,
                failure: Some(format!(
                    "expected typed error code={code}, got code={observed}"
                )),
            },
            None => RowReport {
                id: scenario.id,
                failure: Some(format!(
                    "expected typed error code={code}, both syntaxes returned candidates ids={:?}",
                    sg.corpus_ids
                )),
            },
        },
        ExpectedOutcome::ExpectedFailing {
            owner_ticket,
            reason,
            current_observation,
        } => match current_observation {
            CurrentObservation::Empty => {
                // Closed-loop: row stays GREEN while *both* syntaxes still
                // exhibit the predicted empty observation. When the owner
                // ticket lands and the engine produces real candidates,
                // the assertion flips red and the row owner must promote it.
                if sg.corpus_ids.is_empty() && sg.typed_error_code.is_none() {
                    RowReport {
                        id: scenario.id,
                        failure: None,
                    }
                } else {
                    RowReport {
                        id: scenario.id,
                        failure: Some(format!(
                            "[ExpectedFailing owner={owner_ticket}] predicted empty observation but observed ids={:?} typed_error={:?}; {owner_ticket} may have landed — promote this row. reason={reason}",
                            sg.corpus_ids, sg.typed_error_code,
                        )),
                    }
                }
            }
            CurrentObservation::OverbroadIncludes(must_include) => {
                let all_included = must_include.iter().all(|id| sg.corpus_ids.contains(id));
                if all_included {
                    // Predicted overbroad shape observed — stay GREEN.
                    RowReport {
                        id: scenario.id,
                        failure: None,
                    }
                } else {
                    RowReport {
                        id: scenario.id,
                        failure: Some(format!(
                            "[ExpectedFailing owner={owner_ticket}] predicted overbroad shape includes={must_include:?} but observed={:?}; {owner_ticket} may have landed (filter narrowed) — promote this row. reason={reason}",
                            sg.corpus_ids
                        )),
                    }
                }
            }
        },
    }
}

#[test]
#[ignore = "pending LXE-03..06 dual-syntax parity audit matrix; run explicitly while closing that ticket pack"]
fn dual_syntax_lowering_parity_matrix() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_corpus(&mut rt)?;
    _ = rt.seal()?;
    let mut rt = rt.reopen()?;

    let mut failures: Vec<RowReport> = Vec::new();
    let mut expected_failing_count: usize = 0;
    let mut green_count: usize = 0;

    for scenario in SCENARIOS {
        if matches!(scenario.expected, ExpectedOutcome::ExpectedFailing { .. }) {
            expected_failing_count = expected_failing_count.saturating_add(1);
        }
        let sg_result = rt.query_text(
            TextQuerySyntax::Sourcegraph,
            scenario.sg_query,
            scenario.top_k,
        );
        let lq_result = rt.query_text(TextQuerySyntax::Native, scenario.lq_query, scenario.top_k);
        let report = assess(scenario, &sg_result, &lq_result);
        if report.failure.is_some() {
            failures.push(report);
        } else {
            green_count = green_count.saturating_add(1);
        }
    }

    if failures.is_empty() {
        return Ok(());
    }

    let mut buf = String::new();
    writeln!(
        buf,
        "E2E-02 dual_syntax_lowering_parity_matrix: {} of {} rows failed (expected-failing rows: {}, currently green: {})",
        failures.len(),
        SCENARIOS.len(),
        expected_failing_count,
        green_count
    )?;
    for failure in &failures {
        if let Some(message) = &failure.failure {
            writeln!(buf, "  - [{}] {}", failure.id, message)?;
        }
    }
    Err(anyhow::anyhow!("{buf}"))
}
