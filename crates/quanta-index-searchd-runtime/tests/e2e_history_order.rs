//! QI-BB-023 — history pages are ordered by recency, not by sha, and a
//! cursor walks every match exactly once, before and after a restart.
//!
//! The fixture makes the two orders disagree on purpose: shas ascend while
//! committer times descend, so a sha-ordered top-k would return the
//! oldest commits. The oracle is the fixture's own time order.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;

use quanta_index_contract::{CandidateCountV1, HistoryCursor, TextQuerySyntax};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::{E2eHistoryFixtureSpec, E2eRuntime};

type TestResult = Result<(), Box<dyn Error>>;

const COMMITS: u64 = 5;
const QUERY: &str = "type:commit order";

/// The sha of commit `index`.
///
/// Shas ascend with the index and so do committer times, so the newest
/// commit has the *largest* sha: a sha-ordered top-k would pick the
/// oldest commits, a recency-ordered one the newest.
fn sha_of(index: u64) -> String {
    format!("{index:0>40}")
}

fn ingest_commits(rt: &mut E2eRuntime) -> TestResult {
    for index in 1..=COMMITS {
        // Both the sha and the time ascend with the index.
        let path = format!("src/order_{index}.rs");
        rt.ingest_text("repo", &path, &format!("fn order_{index}() {{}}"))?;
        rt.ingest_history_fixture_spec(&E2eHistoryFixtureSpec {
            commit_sha: &sha_of(index),
            file_path: &path,
            author: "alice",
            committer: "alice",
            message: &format!("order commit {index}"),
            author_time_ms: 1_000_u64.saturating_add(index),
            committer_time_ms: 1_000_u64.saturating_add(index),
            applied_at_ms: 13,
            ref_name: &format!("refs/heads/order-{index}"),
            tag_name: &format!("order-{index}"),
            added_text: "added",
            removed_text: "",
            touched_text: "touched",
        })?;
    }
    let _sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

/// Walk every page and return the commit ids in page order.
fn walk_pages(rt: &mut E2eRuntime, page_size: u32) -> Result<Vec<String>, Box<dyn Error>> {
    let mut cursor: Option<HistoryCursor> = None;
    let mut seen = Vec::new();
    for _page in 0..16 {
        let page = rt.query_history_page(TextQuerySyntax::Sourcegraph, QUERY, page_size, cursor);
        if let Some(error) = page.typed_error {
            return Err(format!("history page refused: {error}").into());
        }
        let window = page.window.ok_or("a served page carries a window")?;
        if window.candidate_count()
            != CandidateCountV1::Exact(COMMITS.saturating_sub(u64::try_from(seen.len())?))
        {
            return Err(format!(
                "each page counts exactly the matches after its cursor: {window:?} after {} seen",
                seen.len()
            )
            .into());
        }
        seen.extend(page.commit_ids);
        match (window.has_more(), page.next_cursor) {
            (true, Some(next)) => cursor = Some(next),
            (false, None) => return Ok(seen),
            (has_more, next) => {
                return Err(format!("has_more={has_more} and cursor={next:?} disagree").into());
            }
        }
    }
    Err("pagination did not terminate".into())
}

#[test]
fn top_k_returns_the_newest_commits_and_pages_walk_every_match_once() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    ingest_commits(&mut rt)?;
    // Newest first: the largest index has the largest committer time, and
    // the largest sha — the opposite of sha order.
    let expected: Vec<String> = (1..=COMMITS).rev().map(sha_of).collect();

    let top_two = rt.query_history(TextQuerySyntax::Sourcegraph, QUERY, 2);
    if let Some(error) = top_two.typed_error {
        return Err(format!("history must serve: {error}").into());
    }
    if top_two.commit_ids != expected.get(..2).ok_or("two expected")? {
        return Err(format!(
            "top-2 must be the two newest commits {:?}, got {:?}",
            expected.get(..2),
            top_two.commit_ids
        )
        .into());
    }
    if top_two.examined < COMMITS {
        return Err(format!("every commit is examined, saw {}", top_two.examined).into());
    }

    let walked = walk_pages(&mut rt, 2)?;
    if walked != expected {
        return Err(format!("pages must walk every match once in order: {walked:?}").into());
    }
    let distinct: BTreeSet<&String> = walked.iter().collect();
    if distinct.len() != walked.len() {
        return Err("a commit appeared on two pages".into());
    }

    // The order is a function of the records, not of ingest order or
    // process lifetime: a restart serves the same pages.
    let mut rt = rt.reopen();
    let after_restart = walk_pages(&mut rt, 2)?;
    if after_restart != expected {
        return Err(format!("pages after a restart drifted: {after_restart:?}").into());
    }
    Ok(())
}
