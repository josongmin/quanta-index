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
//!    behavior:
//!    - `Candidates` rows assert the exact corpus-id set.
//!    - `ExpectedFailing` rows stay GREEN while their
//!      `current_observation` continues to hold; when the owner ticket
//!      lands and behavior diverges, the row flips RED and must be
//!      promoted. The current live matrix happens to have no such rows,
//!      but the closed-loop variant stays in place for regressions.
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueryRoute {
    Text,
    Structural,
}

/// Per-row expected outcome.
///
/// Variants are added only as live rows exist for them (CLAUDE.md "dead
/// port surface is forbidden"). When a parity row graduates to fully-green
/// candidates or a row currently fails with a typed error, add the
/// corresponding variant in the same PR.
#[derive(Clone, Debug)]
enum ExpectedOutcome {
    Candidates {
        ids: &'static [&'static str],
    },
    TypedError {
        code: &'static str,
    },
    /// Closed-loop regression variant. If a future row is intentionally
    /// behind implementation, both syntaxes must exhibit
    /// `current_observation` until the owner ticket lands.
    #[expect(
        dead_code,
        reason = "all current dual-syntax rows are green or typed-error; keep closed-loop variant for future regressions"
    )]
    ExpectedFailing {
        owner_ticket: &'static str,
        reason: &'static str,
        current_observation: CurrentObservation,
    },
}

#[expect(
    dead_code,
    reason = "all current dual-syntax rows are green or typed-error; retained for future expected-failing rows"
)]
#[derive(Clone, Debug)]
enum CurrentObservation {
    Empty,
}

struct ParityScenario {
    route: QueryRoute,
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
    symbol_name: Option<&'static str>,
    structural_identifier: Option<&'static str>,
}

const CORPUS: &[CorpusRow] = &[
    CorpusRow {
        id: "alpha_rust",
        path: "src/lib.rs",
        content: "fn parity_needle_alpha() {}",
        symbol_name: None,
        structural_identifier: Some("parity_needle_alpha"),
    },
    CorpusRow {
        id: "beta_py",
        path: "scripts/helper.py",
        content: "def parity_needle_alpha(): pass",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "gamma_md",
        path: "docs/intro.md",
        content: "parity documentation lives here",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "delta_other_path",
        path: "src/other.rs",
        content: "fn parity_needle_alpha() {}",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "epsilon_regex",
        path: "src/version.rs",
        content: "const VERSION: &str = \"v9.8.7\";",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "zeta_raw",
        path: "src/raw.rs",
        content: "let parity_foo_bar_baz = 1;",
        symbol_name: None,
        structural_identifier: None,
    },
    CorpusRow {
        id: "theta_symbol",
        path: "src/sym.rs",
        content: "pub fn ParityTypeSymbol(arg: i32) -> i32 { arg }",
        symbol_name: Some("ParityTypeSymbol"),
        structural_identifier: None,
    },
];

const SCENARIOS: &[ParityScenario] = &[
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_filter_parity",
        sg_query: "repo:repo-other parity_needle_alpha",
        lq_query: "repo:repo-other parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_filter_parity",
        sg_query: "file:src/lib.rs parity_needle_alpha",
        lq_query: "file:src/lib.rs parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "lang_filter_parity",
        sg_query: "lang:rust parity_needle_alpha",
        lq_query: "lang:rust parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "delta_other_path"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "case_filter_parity",
        sg_query: "case:yes PARITY_NEEDLE_ALPHA",
        lq_query: "case:yes PARITY_NEEDLE_ALPHA",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "count_parity",
        sg_query: "count:1 parity_needle_alpha",
        lq_query: "count:1 parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "fork_typed_unavailable_parity",
        sg_query: "fork:no parity_needle_alpha",
        lq_query: "fork:no parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::TypedError {
            code: "LEX_FILTER_FORK_UNAVAILABLE",
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "visibility_typed_unavailable_parity",
        sg_query: "visibility:public parity_needle_alpha",
        lq_query: "visibility:public parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::TypedError {
            code: "LEX_FILTER_VISIBILITY_UNAVAILABLE",
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "type_file_parity",
        sg_query: "type:file parity_needle_alpha",
        lq_query: "type:file parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "beta_py", "delta_other_path"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "type_symbol_parity",
        sg_query: "type:symbol ParityTypeSymbol",
        lq_query: "type:symbol ParityTypeSymbol",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["theta_symbol"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "select_file_parity",
        sg_query: "select:file parity_needle_alpha",
        lq_query: "select:file parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "beta_py", "delta_other_path"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "select_content_parity",
        sg_query: "select:content parity_needle_alpha",
        lq_query: "select:content parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "beta_py", "delta_other_path"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "select_symbol_parity",
        sg_query: "select:symbol ParityTypeSymbol",
        lq_query: "select:symbol ParityTypeSymbol",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["theta_symbol"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "file_contains_raw_substring_parity",
        // The shared executable raw-substring surface today is
        // `file:contains('...')` on the SG side and a single-quoted raw
        // string leaf on the native side.
        sg_query: "file:contains('oo_ba')",
        lq_query: "'oo_ba'",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &["zeta_raw"] },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "patterntype_regexp_parity",
        sg_query: "patterntype:regexp v\\d+\\.\\d+\\.\\d+",
        lq_query: "/v\\d+\\.\\d+\\.\\d+/",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["epsilon_regex"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "boolean_or_parity",
        sg_query: "parity_needle_alpha OR documentation",
        lq_query: "parity_needle_alpha OR documentation",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "beta_py", "delta_other_path", "gamma_md"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "negation_parity",
        sg_query: "parity_needle_alpha NOT helper",
        lq_query: "parity_needle_alpha NOT helper",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "beta_py", "delta_other_path"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_predicate_parity",
        // Dual-syntax parity only compares live equivalent surfaces.
        // The SG-only `repohasfile:` alias remains covered by bridge
        // parser/translator tests; the active equivalent query here is the
        // shared `repo:has.file(...)` predicate surface.
        sg_query: "repo:has.file(path:src/lib.rs) parity_needle_alpha",
        lq_query: "repo:has.file(path:src/lib.rs) parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "beta_py", "delta_other_path"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_predicate_under_or_parity",
        sg_query: "repo:has.file(path:src/lib.rs) OR documentation",
        lq_query: "repo:has.file(path:src/lib.rs) OR documentation",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &[
                "alpha_rust",
                "beta_py",
                "delta_other_path",
                "epsilon_regex",
                "gamma_md",
                "theta_symbol",
                "zeta_raw",
            ],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "repo_has_file_predicate_under_not_parity",
        sg_query: "parity_needle_alpha NOT repo:has.file(path:src/missing.rs)",
        lq_query: "parity_needle_alpha NOT repo:has.file(path:src/missing.rs)",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust", "beta_py", "delta_other_path"],
        },
    },
    ParityScenario {
        route: QueryRoute::Text,
        id: "select_repo_projection_parity",
        sg_query: "select:repo parity_needle_alpha",
        lq_query: "select:repo parity_needle_alpha",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_happy_path_parity",
        sg_query: r#"repo:repo-e2e path:src/lib.rs lang:rust patterntype:structural "function_item { { identifier :[name] } }""#,
        lq_query: "repo:repo-e2e file:src/lib.rs lang:rust match { function_item { { identifier :[name] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_boolean_or_parity",
        sg_query: r#"patterntype:structural "function_item { { identifier :[name] } }" OR patterntype:structural "trait_item""#,
        lq_query: "match { function_item { { identifier :[name] } } } OR match { trait_item }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_boolean_not_parity",
        sg_query: r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT "trait_item""#,
        lq_query: "match { function_item { { identifier :[name] } } } AND NOT match { trait_item }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_typed_expr_hole_parity",
        sg_query: r#"patterntype:structural "function_item { { :[name.expr] } }""#,
        lq_query: "match { function_item { { :[name.expr] } } }",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_rust"],
        },
    },
    ParityScenario {
        route: QueryRoute::Structural,
        id: "structural_sourcegraph_native_select_rejection_parity",
        sg_query: r#"select:repo patterntype:structural "function_item""#,
        lq_query: "select:repo match { function_item }",
        top_k: 10,
        expected: ExpectedOutcome::TypedError {
            code: "STR_INVALID_REQUEST",
        },
    },
];

fn ingest_corpus(rt: &mut E2eRuntime) -> AnyResult<()> {
    for row in CORPUS {
        rt.ingest_text("repo-e2e", row.path, row.content)?;
        if let Some(identifier) = row.structural_identifier {
            rt.ingest_structural_function_tree(row.path, row.content, identifier)?;
        }
        if let Some(symbol_name) = row.symbol_name {
            rt.ingest_symbol("repo-e2e", row.path, row.id, symbol_name)?;
        }
    }
    Ok(())
}

fn corpus_id_for_candidate_id(candidate_id: &str) -> Option<&'static str> {
    if let Some(row) = CORPUS.iter().find(|row| row.id == candidate_id) {
        return Some(row.id);
    }
    let rest = candidate_id.strip_prefix("e2e-")?;
    let (_, path) = rest.split_once('-')?;
    CORPUS.iter().find(|row| row.path == path).map(|row| row.id)
}

fn observed_corpus_ids(result: &E2eQueryResult) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = result
        .candidate_ids
        .iter()
        .filter_map(|candidate_id| corpus_id_for_candidate_id(candidate_id))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn raw_candidate_ids(result: &E2eQueryResult) -> Vec<String> {
    result.candidate_ids.clone()
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
        ExpectedOutcome::Candidates { ids } => {
            if let Some(observed) = &sg.typed_error_code {
                RowReport {
                    id: scenario.id,
                    failure: Some(format!(
                        "expected Candidates ids={ids:?}, got typed error code={observed}"
                    )),
                }
            } else if sg.corpus_ids == *ids {
                RowReport {
                    id: scenario.id,
                    failure: None,
                }
            } else {
                RowReport {
                    id: scenario.id,
                    failure: Some(format!(
                        "expected Candidates ids={ids:?}, got ids={:?}",
                        sg.corpus_ids
                    )),
                }
            }
        }
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
                    "expected typed error code={code}, got ids={corpus_ids:?}",
                    corpus_ids = sg.corpus_ids
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
        },
    }
}

#[test]
fn dual_syntax_lowering_parity_matrix() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_corpus(&mut rt)?;
    _ = rt.seal()?;
    let mut rt = rt.reopen();

    let mut failures: Vec<RowReport> = Vec::new();
    let mut expected_failing_count: usize = 0;
    let mut green_count: usize = 0;

    for scenario in SCENARIOS {
        if matches!(scenario.expected, ExpectedOutcome::ExpectedFailing { .. }) {
            expected_failing_count = expected_failing_count.saturating_add(1);
        }
        let sg_result = match scenario.route {
            QueryRoute::Text => rt.query_text(
                TextQuerySyntax::Sourcegraph,
                scenario.sg_query,
                scenario.top_k,
            ),
            QueryRoute::Structural => rt.query_structural(
                TextQuerySyntax::Sourcegraph,
                scenario.sg_query,
                scenario.top_k,
            ),
        };
        let lq_result = match scenario.route {
            QueryRoute::Text => {
                rt.query_text(TextQuerySyntax::Native, scenario.lq_query, scenario.top_k)
            }
            QueryRoute::Structural => {
                rt.query_structural(TextQuerySyntax::Native, scenario.lq_query, scenario.top_k)
            }
        };
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
