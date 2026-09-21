//! QI-BB-023 보완 #3 — the history route's two orders evaluate one text
//! predicate, end to end.
//!
//! The fixture is the audit's own scenario: `fix` over `Fix typo`,
//! `prefix` and `fix bug`. A keyword is a folded whole-token match, so
//! both orders count `Fix typo` and `fix bug` and never `prefix`; the
//! exact `window` total and the row set are the same whichever order the
//! request asks for. A raw string beside the keyword narrows both orders
//! the same way; a raw string alone is served as a filter under recency
//! and refused typed under relevance, which has nothing to score.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;

use quanta_index_contract::{CandidateCountV1, HistoryOrderV1, TextQuerySyntax};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::{E2eHistoryFixtureSpec, E2eHistoryResult, E2eRuntime};

type TestResult = Result<(), Box<dyn Error>>;

const TOP_K: u32 = 16;

/// `(index, message)`: the index is the sha and the committer time.
const FIXTURE: &[(u64, &str)] = &[
    (1, "fix bug"),
    (2, "prefix"),
    (3, "Fix typo"),
    (4, "fix the 'x.y' literal"),
];

fn sha_of(index: u64) -> String {
    format!("{index:0>40}")
}

fn path_of(index: u64) -> String {
    format!("src/predicate_{index}.rs")
}

fn ingest_fixture(rt: &mut E2eRuntime) -> TestResult {
    for (index, message) in FIXTURE {
        let path = path_of(*index);
        rt.ingest_text("repo", &path, &format!("fn predicate_{index}() {{}}"))?;
        // The hunk's touched text is the message, so the diff kind mirrors
        // the commit kind.
        rt.ingest_history_fixture_spec(&E2eHistoryFixtureSpec {
            commit_sha: &sha_of(*index),
            file_path: &path,
            author: "alice",
            committer: "alice",
            message,
            author_time_ms: 1_000_u64.saturating_add(*index),
            committer_time_ms: 1_000_u64.saturating_add(*index),
            applied_at_ms: 13,
            ref_name: &format!("refs/heads/predicate-{index}"),
            tag_name: &format!("predicate-{index}"),
            added_text: "",
            removed_text: "",
            touched_text: message,
        })?;
    }
    let _sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

/// The commit shas or diff paths a served page holds.
fn rows(page: &E2eHistoryResult) -> BTreeSet<String> {
    page.commit_ids
        .iter()
        .chain(page.diff_paths.iter())
        .cloned()
        .collect()
}

/// One query under both orders: the same rows and the same exact total.
fn assert_orders_agree(
    rt: &mut E2eRuntime,
    query: &str,
    expected: &BTreeSet<String>,
) -> TestResult {
    let want_total = CandidateCountV1::Exact(u64::try_from(expected.len())?);
    for order in HistoryOrderV1::ALL {
        let page = rt.query_history_page(TextQuerySyntax::Native, query, TOP_K, order, None);
        if let Some(error) = page.typed_error {
            return Err(format!("{query:?} under {order}: refused: {error}").into());
        }
        let observed = rows(&page);
        if observed != *expected {
            return Err(format!(
                "{query:?} under {order}: rows {observed:?}, expected {expected:?}"
            )
            .into());
        }
        let window = page.window.ok_or("a served page carries a window")?;
        if window.candidate_count() != want_total {
            return Err(format!(
                "{query:?} under {order}: total {:?}, expected {want_total:?}",
                window.candidate_count()
            )
            .into());
        }
    }
    Ok(())
}

fn shas(indexes: &[u64]) -> BTreeSet<String> {
    indexes.iter().copied().map(sha_of).collect()
}

fn paths(indexes: &[u64]) -> BTreeSet<String> {
    indexes.iter().copied().map(path_of).collect()
}

#[test]
fn recency_and_relevance_count_the_same_rows_for_the_same_query() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    ingest_fixture(&mut rt)?;
    let cases: [(&str, BTreeSet<String>); 8] = [
        ("type:commit fix", shas(&[1, 3, 4])),
        ("type:commit fix case:no", shas(&[1, 3, 4])),
        ("type:commit fix case:yes", shas(&[1, 4])),
        ("type:commit \"fix bug\"", shas(&[1])),
        ("type:commit fix 'x.y'", shas(&[4])),
        ("type:commit fix NOT 'x.y'", shas(&[1, 3])),
        ("type:diff fix", paths(&[1, 3, 4])),
        ("type:diff fix 'x.y'", paths(&[4])),
    ];
    for (query, expected) in cases {
        assert_orders_agree(&mut rt, query, &expected)?;
    }

    // A raw string alone: a substring filter under recency (`prefix`
    // included), nothing to score under relevance.
    let recency = rt.query_history_page(
        TextQuerySyntax::Native,
        "type:commit 'fix'",
        TOP_K,
        HistoryOrderV1::Recency,
        None,
    );
    if let Some(error) = recency.typed_error {
        return Err(format!("a raw string alone is a filter under recency: {error}").into());
    }
    if rows(&recency) != shas(&[1, 2, 3, 4]) {
        return Err(format!(
            "a raw string is a substring: `prefix` matches, got {:?}",
            rows(&recency)
        )
        .into());
    }
    let relevance = rt.query_history_page(
        TextQuerySyntax::Native,
        "type:commit 'fix'",
        TOP_K,
        HistoryOrderV1::Relevance,
        None,
    );
    match relevance.typed_error {
        Some(error)
            if error.code
                == e2e_harness::E2eErrorCode::Remote(
                    quanta_index_core::HISTORY_TEXT_QUERY_UNSCORABLE_CODE,
                ) => {}
        other => {
            return Err(
                format!("a raw string alone is unscorable under relevance, got {other:?}").into()
            );
        }
    }

    // A literal with no token is refused typed under both orders, before
    // any row is read.
    for order in HistoryOrderV1::ALL {
        let page = rt.query_history_page(
            TextQuerySyntax::Native,
            "type:commit \"👍\"",
            TOP_K,
            order,
            None,
        );
        match page.typed_error {
            Some(error) if error.code.as_str() == "LEX_TEXT_QUERY_NO_TOKENS" => {}
            other => {
                return Err(format!(
                    "a token-less literal is refused typed under {order}, got {other:?}"
                )
                .into());
            }
        }
    }
    Ok(())
}
