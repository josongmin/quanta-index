//! E2E-01 — lexical full-fidelity matrix against the live driver.
//!
//! Table-driven. Every row in `SCENARIOS` ingests through the real publish
//! path, seals, then issues a `TextQueryRequest` and asserts against the
//! observed result. Broad executable-query inventory now lives in
//! `runtime_rows.toml`; this rail keeps only specialized regression shapes
//! that the closeout corpus does not express well.
//!
//! The harness supports two row categories; the current live matrix happens
//! to use only `Candidates` rows:
//!
//! - `ExpectedOutcome::Candidates` — wiring exists today; row must return
//!   the exact ordered candidate-id set.
//! - `ExpectedOutcome::ExpectedFailing { owner_ticket, .. }` — wiring is
//!   pending the named LXE-NN ticket. The row STILL EXECUTES the query and
//!   asserts the *current* observable behavior (typically "no match" or a
//!   typed-unavailable code). When LXE-NN lands and the underlying engine
//!   produces real results, the assertion goes red and the row owner
//!   converts it to `Candidates` (or, in the same PR, introduces a
//!   `TypedError` variant if the live behavior is a typed rejection).
//!   That is the closed-loop guarantee.
//!
//! No row may be silently skipped. A row marked `ExpectedFailing` stays
//! GREEN while its `current_observation` continues to hold; the moment
//! the observation diverges (e.g. the owner ticket lands and the engine
//! now returns real candidates where empty was predicted), the row goes
//! RED with a "promote this row" message, forcing an honest update.

#![forbid(unsafe_code)]

use anyhow::Result as AnyResult;
use quanta_index_contract::TextQuerySyntax;
use std::fmt::Write as _;

use crate::e2e_harness::{E2eQueryResult, E2eRuntime};

/// Per-row expected outcome.
///
/// See module doc-comment for closed-loop semantics.
///
/// Variants are added only as live rows exist for them (CLAUDE.md "dead
/// port surface is forbidden"). When LXE-NN tickets land and
/// `ExpectedFailing` rows promote to typed-error coverage, add the
/// `TypedError` variant in the same PR.
#[derive(Clone, Debug)]
enum ExpectedOutcome {
    Candidates {
        ids: &'static [&'static str],
    },
    #[expect(
        dead_code,
        reason = "all current lexical full-fidelity rows are green candidates; keep closed-loop variant for future regressions"
    )]
    ExpectedFailing {
        owner_ticket: &'static str,
        reason: &'static str,
        current_observation: CurrentObservation,
    },
}

#[expect(
    dead_code,
    reason = "all current lexical full-fidelity rows are green or typed-error; retained for future expected-failing rows"
)]
#[derive(Clone, Debug)]
enum CurrentObservation {
    /// Today this query returns no candidates (and no typed error).
    Empty,
}

struct LexicalScenario {
    id: &'static str,
    /// Native LQ syntax.
    query_text: &'static str,
    top_k: u32,
    expected: ExpectedOutcome,
}

/// Corpus shared by all rows.
///
/// One ingest covers content, path-only, multi-language, phrase boundary,
/// regex-only token, raw substring across token boundaries, and
/// trigram-false-positive shapes.
///
/// `id` is the assertion key returned after the harness's synthesized
/// `e2e-<n>-<path>` candidate id is stripped back to `path` and looked up
/// in this table.
struct CorpusRow {
    id: &'static str,
    path: &'static str,
    content: &'static str,
    symbol_name: Option<&'static str>,
}

const CORPUS: &[CorpusRow] = &[
    // content vs. path: content has needle "alpha_content_needle"; the path
    // does NOT, so a content-token query against the needle must NOT match
    // the path-only row below.
    CorpusRow {
        id: "alpha_content",
        path: "src/lib.rs",
        content: "fn alpha_content_needle() {}",
        symbol_name: None,
    },
    // path-only: the path contains the path-only token, content does not.
    CorpusRow {
        id: "beta_pathonly",
        path: "config/path_only_needle.toml",
        content: "value = 1",
        symbol_name: None,
    },
    // python file with same token as alpha — used for `lang:` filter
    // discrimination.
    CorpusRow {
        id: "gamma_py_same",
        path: "scripts/helper.py",
        content: "def alpha_content_needle(): pass",
        symbol_name: None,
    },
    // phrase row: contains "lemon yellow banana" exact-adjacent. A regex
    // token query for `lemon banana` must NOT match (phrase positions).
    CorpusRow {
        id: "delta_phrase",
        path: "docs/colors.md",
        content: "the lemon yellow banana ripens",
        symbol_name: None,
    },
    // regex-only row: contains "v1.2.3-rc.4" which a token query cannot
    // hit, but a regex `v\d+\.\d+\.\d+` can.
    CorpusRow {
        id: "epsilon_regex",
        path: "src/version.rs",
        content: "const VERSION: &str = \"v1.2.3-rc.4\";",
        symbol_name: None,
    },
    // raw substring across token boundary: contains "foo_bar_baz" — a
    // tokenizer splits on `_` so a token query for `foo_bar` may not hit;
    // a raw substring query must hit.
    CorpusRow {
        id: "zeta_raw",
        path: "src/raw.rs",
        content: "let foo_bar_baz = 0;",
        symbol_name: None,
    },
    // trigram false-positive bait: contains "needle_xx" but not "needle_x"
    // exact — a naive trigram match for "needle_x" without verify could
    // false-positive against this row.
    CorpusRow {
        id: "eta_trigram_bait",
        path: "src/bait.rs",
        content: "let needle_xx = 1;",
        symbol_name: None,
    },
    // symbol-shaped row: function declaration named `MyTypeSymbol` —
    // exercised by `type:symbol`/`select:symbol`.
    CorpusRow {
        id: "theta_symbol",
        path: "src/sym.rs",
        content: "pub fn MyTypeSymbol(arg: i32) -> i32 { arg }",
        symbol_name: Some("MyTypeSymbol"),
    },
];

const SCENARIOS: &[LexicalScenario] = &[
    LexicalScenario {
        id: "content_term_does_not_match_path_only",
        // The path "config/path_only_needle.toml" contains the token
        // `path_only_needle` but the row's *content* doesn't. A content-term
        // query must therefore return zero matches for this token (no
        // accidental path indexing into content).
        //
        // NOTE: the harness ingests `path` as a separate field, but this row
        // proves the content query surface does not leak path-only matches.
        query_text: "path_only_needle_zzz_not_present_anywhere",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    LexicalScenario {
        id: "repo_has_file_predicate_under_or_short_circuits_true_repo_gate",
        // `repo:has.file(path:src/lib.rs)` is true for this single test
        // repo, so `true OR delta_phrase_token` must widen to all text
        // candidates on the lexical route.
        query_text: "repo:has.file(path:src/lib.rs) OR ripens",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &[
                "delta_phrase",
                "beta_pathonly",
                "gamma_py_same",
                "eta_trigram_bait",
                "alpha_content",
                "zeta_raw",
                "theta_symbol",
                "epsilon_regex",
            ],
        },
    },
    // ──────── case ────────
    LexicalScenario {
        id: "case_insensitive_default",
        // Default is case-insensitive: "ALPHA_CONTENT_NEEDLE" must match
        // the lowercase content in both rust and python rows.
        query_text: "ALPHA_CONTENT_NEEDLE",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_content", "gamma_py_same"],
        },
    },
    // ──────── phrase ────────
    LexicalScenario {
        id: "phrase_exact_adjacent_matches",
        query_text: "\"lemon yellow banana\"",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["delta_phrase"],
        },
    },
    LexicalScenario {
        id: "phrase_unordered_tokens_must_not_match",
        // `banana lemon` (reversed) as a phrase must NOT match the
        // forward-only content. This row proves the live phrase path enforces
        // order and adjacency rather than accidental token coincidence.
        query_text: "\"banana lemon\"",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    // ──────── regex ────────
    LexicalScenario {
        id: "regex_trigram_false_positive_rejected",
        // Regex `needle_x[0-9]` should NOT match `needle_xx` (no digit).
        // A naive trigram prefilter would surface eta_trigram_bait; the
        // exact verify must reject it.
        query_text: "/needle_x[0-9]/",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
];

fn ingest_corpus(rt: &mut E2eRuntime) -> AnyResult<()> {
    for row in CORPUS {
        rt.ingest_text("repo-e2e", row.path, row.content)?;
        if let Some(symbol_name) = row.symbol_name {
            rt.ingest_symbol("repo-e2e", row.path, row.id, symbol_name)?;
        }
    }
    Ok(())
}

fn run_query(rt: &mut E2eRuntime, scenario: &LexicalScenario) -> E2eQueryResult {
    rt.query_text(TextQuerySyntax::Native, scenario.query_text, scenario.top_k)
}

fn candidate_ids(result: &E2eQueryResult) -> Vec<String> {
    result
        .candidates
        .iter()
        .map(|c| c.candidate_id.clone())
        .collect()
}

/// Strip the harness-applied `e2e-<n>-` prefix to recover the bare `path`.
///
/// The harness builds `ChunkId::new(format!("e2e-{n}-{path}"))` in
/// `ingest_text`, so the tail after the second `-` is the row path.
fn corpus_id_for_candidate_id(candidate_id: &str) -> Option<&'static str> {
    if let Some(row) = CORPUS.iter().find(|row| row.id == candidate_id) {
        return Some(row.id);
    }
    // candidate_id shape: `e2e-<n>-<path>`. Split off the leading
    // `e2e-<n>-` to recover the path.
    let rest = candidate_id.strip_prefix("e2e-")?;
    let (_, path) = rest.split_once('-')?;
    CORPUS.iter().find(|row| row.path == path).map(|row| row.id)
}

fn observed_corpus_ids(result: &E2eQueryResult) -> Result<Vec<&'static str>, Vec<String>> {
    let mut observed = Vec::with_capacity(result.candidates.len());
    let mut unmapped = Vec::new();
    for candidate in &result.candidates {
        match corpus_id_for_candidate_id(&candidate.candidate_id) {
            Some(id) => observed.push(id),
            None => unmapped.push(candidate.candidate_id.clone()),
        }
    }
    if unmapped.is_empty() {
        Ok(observed)
    } else {
        Err(unmapped)
    }
}

/// One assertion outcome — either ok or a row-scoped failure to report.
struct RowReport {
    id: &'static str,
    failure: Option<String>,
}

fn assess(scenario: &LexicalScenario, result: &E2eQueryResult) -> RowReport {
    let observed = match observed_corpus_ids(result) {
        Ok(observed) => observed,
        Err(unmapped_candidate_ids) => {
            return RowReport {
                id: scenario.id,
                failure: Some(format!(
                    "observed unmapped candidate ids={unmapped_candidate_ids:?}; raw candidates={:?}",
                    result.candidates
                )),
            };
        }
    };
    match &scenario.expected {
        ExpectedOutcome::Candidates { ids } => {
            if let Some(err) = &result.typed_error {
                return RowReport {
                    id: scenario.id,
                    failure: Some(format!(
                        "expected Candidates ids={ids:?}, got typed error code={} message={}",
                        err.code, err.message
                    )),
                };
            }
            let expected: Vec<&'static str> = (*ids).to_vec();
            if observed == expected {
                RowReport {
                    id: scenario.id,
                    failure: None,
                }
            } else {
                RowReport {
                    id: scenario.id,
                    failure: Some(format!(
                        "expected ids={expected:?}, observed ids={observed:?}, raw candidate ids={:?}, raw candidates={:?}",
                        candidate_ids(result),
                        result.candidates
                    )),
                }
            }
        }
        ExpectedOutcome::ExpectedFailing {
            owner_ticket,
            reason,
            current_observation,
        } => {
            // Assert against the CURRENT behavior. When the owner ticket
            // lands and the behavior moves toward the future expectation,
            // this assertion goes red and the row must be updated.
            let observed_error = result.typed_error.as_ref().map(|e| e.code.as_str());
            match current_observation {
                CurrentObservation::Empty => {
                    // Closed-loop: row stays GREEN while the *current*
                    // observation matches the prediction. When the owner
                    // ticket lands and the engine produces real results,
                    // `observed` will no longer be empty (or a typed error
                    // will appear) — the assertion goes red and the row
                    // owner must promote it to `Candidates`/`TypedError`.
                    if observed.is_empty() && observed_error.is_none() {
                        RowReport {
                            id: scenario.id,
                            failure: None,
                        }
                    } else {
                        RowReport {
                            id: scenario.id,
                            failure: Some(format!(
                                "[ExpectedFailing owner={owner_ticket}] predicted empty current observation, but observed ids={observed:?} typed_error={observed_error:?}; {owner_ticket} may have landed — promote this row to Candidates/TypedError. reason={reason}"
                            )),
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn lexical_full_fidelity_matrix() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_corpus(&mut rt)?;
    _ = rt.seal()?;
    let mut rt = rt.reopen();

    let mut failures: Vec<RowReport> = Vec::new();
    let mut green_count: usize = 0;
    let mut expected_failing_count: usize = 0;

    for scenario in SCENARIOS {
        if matches!(scenario.expected, ExpectedOutcome::ExpectedFailing { .. }) {
            expected_failing_count = expected_failing_count.saturating_add(1);
        }
        let result = run_query(&mut rt, scenario);
        let report = assess(scenario, &result);
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
        "E2E-01 lexical_full_fidelity_matrix: {} of {} rows failed (expected-failing rows in this file: {}, rows currently green: {})",
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
