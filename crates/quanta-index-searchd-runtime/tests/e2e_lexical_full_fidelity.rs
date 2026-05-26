//! E2E-01 — lexical full-fidelity matrix against the live driver.
//!
//! Table-driven. Every row in `SCENARIOS` ingests through the real publish
//! path, seals, then issues a `TextQueryRequest` and asserts against the
//! observed result. The harness supports two row categories; the current
//! live matrix happens to use only `Candidates` and `TypedError` rows:
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

#[path = "common/e2e_harness.rs"]
mod e2e_harness;

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
    TypedError {
        code: &'static str,
    },
    #[expect(
        dead_code,
        reason = "all current lexical full-fidelity rows are green or typed-error; keep closed-loop variant for future regressions"
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
    // ──────── content term ────────
    LexicalScenario {
        id: "content_term_matches_content",
        // The token `alpha_content_needle` lives in both alpha_content
        // (rust) and gamma_py_same (python). The lang filter test below
        // proves discrimination is missing; this row only proves content
        // tokens hit content (and not the path-only beta row).
        query_text: "alpha_content_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_content", "gamma_py_same"],
        },
    },
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
    // ──────── path / file ────────
    LexicalScenario {
        id: "path_query_matches_path",
        // Native LQ path-as-content query on the live simple-leaf path-term
        // surface backed by materialized path authority.
        query_text: "path_only_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["beta_pathonly"],
        },
    },
    LexicalScenario {
        id: "file_filter_narrows_content_hits_by_path",
        query_text: "file:src/lib.rs alpha_content_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_content"],
        },
    },
    // ──────── repo ────────
    LexicalScenario {
        id: "repo_filter_excludes_other_repo",
        // The harness publishes against a single repo (`repo-e2e`). A
        // `repo:repo-other` filter must return zero matches.
        query_text: "repo:repo-other alpha_content_needle",
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
                "alpha_content",
                "beta_pathonly",
                "delta_phrase",
                "epsilon_regex",
                "eta_trigram_bait",
                "gamma_py_same",
                "theta_symbol",
                "zeta_raw",
            ],
        },
    },
    LexicalScenario {
        id: "select_repo_projects_to_first_repo_representative",
        // The lexical harness is single-repo, so `select:repo` must collapse
        // matching text hits to one representative row for that repo.
        query_text: "select:repo alpha_content_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_content"],
        },
    },
    // ──────── lang ────────
    LexicalScenario {
        id: "lang_filter_picks_only_requested_language",
        // `alpha_content_needle` exists in both `src/lib.rs` (rust) and
        // `scripts/helper.py` (python). lang:rust must return alpha only.
        query_text: "lang:rust alpha_content_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_content"],
        },
    },
    // ──────── boolean ────────
    LexicalScenario {
        id: "boolean_and_intersection",
        // Both tokens are in `alpha_content` content only.
        query_text: "alpha_content_needle AND fn",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_content"],
        },
    },
    LexicalScenario {
        id: "boolean_or_union",
        query_text: "alpha_content_needle OR ripens",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_content", "delta_phrase", "gamma_py_same"],
        },
    },
    LexicalScenario {
        id: "boolean_not_exclusion",
        query_text: "alpha_content_needle NOT lib",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_content", "gamma_py_same"],
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
    LexicalScenario {
        id: "case_sensitive_changes_result_set",
        query_text: "case:yes ALPHA_CONTENT_NEEDLE",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    // ──────── count ────────
    LexicalScenario {
        id: "count_cap_returns_top_n",
        // `count:2` must force full recall, then deterministic
        // score/path/line/candidate_id stabilization before truncation.
        query_text: "count:2 needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["beta_pathonly", "eta_trigram_bait"],
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
        id: "regex_only_match_not_reachable_by_token",
        query_text: "/v\\d+\\.\\d+\\.\\d+/",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["epsilon_regex"],
        },
    },
    LexicalScenario {
        id: "regex_trigram_false_positive_rejected",
        // Regex `needle_x[0-9]` should NOT match `needle_xx` (no digit).
        // A naive trigram prefilter would surface eta_trigram_bait; the
        // exact verify must reject it.
        query_text: "/needle_x[0-9]/",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &[] },
    },
    // ──────── raw substring ────────
    LexicalScenario {
        id: "raw_substring_token_boundary_crossing",
        // Raw substring `oo_ba` crosses the tokenizer split on `_`. Token
        // query for "oo_ba" cannot hit; raw substring must. Native LQ
        // spells raw substring as a single-quoted raw string leaf.
        query_text: "'oo_ba'",
        top_k: 10,
        expected: ExpectedOutcome::Candidates { ids: &["zeta_raw"] },
    },
    // ──────── symbol / select / type ────────
    LexicalScenario {
        id: "type_symbol_routes_to_symbol_docs",
        query_text: "type:symbol MyTypeSymbol",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["theta_symbol"],
        },
    },
    LexicalScenario {
        id: "select_symbol_routes_to_symbol_docs",
        query_text: "select:symbol MyTypeSymbol",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["theta_symbol"],
        },
    },
    LexicalScenario {
        id: "select_file_returns_file_only_results",
        query_text: "select:file alpha_content_needle",
        top_k: 10,
        expected: ExpectedOutcome::Candidates {
            ids: &["alpha_content", "gamma_py_same"],
        },
    },
    // ──────── producer-dependent typed unavailable ────────
    LexicalScenario {
        id: "fork_filter_typed_unavailable",
        query_text: "fork:no alpha_content_needle",
        top_k: 10,
        expected: ExpectedOutcome::TypedError {
            code: "LEX_FILTER_FORK_UNAVAILABLE",
        },
    },
    LexicalScenario {
        id: "visibility_filter_typed_unavailable",
        query_text: "visibility:public alpha_content_needle",
        top_k: 10,
        expected: ExpectedOutcome::TypedError {
            code: "LEX_FILTER_VISIBILITY_UNAVAILABLE",
        },
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

/// One assertion outcome — either ok or a row-scoped failure to report.
struct RowReport {
    id: &'static str,
    failure: Option<String>,
}

fn assess(scenario: &LexicalScenario, result: &E2eQueryResult) -> RowReport {
    let observed = observed_corpus_ids(result);
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
            let mut expected: Vec<&'static str> = (*ids).to_vec();
            expected.sort_unstable();
            expected.dedup();
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
        ExpectedOutcome::TypedError { code } => match &result.typed_error {
            Some(err) if err.code == *code => RowReport {
                id: scenario.id,
                failure: None,
            },
            Some(err) => RowReport {
                id: scenario.id,
                failure: Some(format!(
                    "expected typed error code={code}, got code={err_code} message={err_message}",
                    err_code = err.code,
                    err_message = err.message
                )),
            },
            None => RowReport {
                id: scenario.id,
                failure: Some(format!(
                    "expected typed error code={code}, got candidates={observed:?}"
                )),
            },
        },
        ExpectedOutcome::ExpectedFailing {
            owner_ticket,
            reason,
            current_observation,
        } => {
            // Assert against the CURRENT behavior. When the owner ticket
            // lands and the behavior moves toward the future expectation,
            // this assertion goes red and the row must be updated.
            let observed = observed_corpus_ids(result);
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
