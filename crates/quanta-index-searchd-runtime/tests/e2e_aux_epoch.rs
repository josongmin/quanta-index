//! QI-BB-020 W2 — a history page walk never mixes epochs.
//!
//! Every page names the history authority epoch it was cut from, and a
//! continuation is served from exactly that epoch: an ingest that lands
//! between two pages is invisible to the walk (no row repeats, none is
//! skipped), while a fresh walk sees it. The epoch is durable: after a
//! restart the sequence continues from the persisted value, and the old
//! walk's cursor — whose epoch retention does not survive the restart —
//! is refused typed rather than served from the restored snapshot.
//!
//! The oracle is the fixture's own commit set and time order; nothing
//! here reads the daemon's clock or waits on timing.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;

use quanta_index_contract::lex::{CommitRecord, CommitSha};
use quanta_index_contract::{
    AuxEpochV1, HistoryCursor, HistoryIngestBatch, HistoryOrderV1, ManifestGeneration,
    TextQuerySyntax,
};
use quanta_index_core::AUX_EPOCH_EXPIRED_CODE;
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::{E2eHistoryFixtureSpec, E2eHistoryResult, E2eRuntime};

type TestResult = Result<(), Box<dyn Error>>;

/// Commits ingested before the walk starts.
const COMMITS: u64 = 30;
/// Commits ingested between page one and page two.
const LATE_COMMITS: u64 = 5;
const PAGE: u32 = 10;
const QUERY: &str = "type:commit epoch";

/// The sha of commit `index`; shas and committer times both ascend with
/// the index for the original commits.
fn sha_of(index: u64) -> String {
    format!("{index:0>40}")
}

/// Committer time of an original commit: 10 ms apart, newest last.
fn time_of(index: u64) -> u64 {
    1_000_u64.saturating_add(index.saturating_mul(10))
}

/// Committer time of a late commit: between the 15th and 16th original
/// commit, so by recency it belongs on page two of a ten-per-page walk.
fn late_time_of(late_index: u64) -> u64 {
    time_of(15).saturating_add(late_index)
}

fn ingest_original_commits(rt: &mut E2eRuntime) -> Result<ManifestGeneration, Box<dyn Error>> {
    for index in 1..=COMMITS {
        let path = format!("src/epoch_{index}.rs");
        rt.ingest_text("repo", &path, &format!("fn epoch_{index}() {{}}"))?;
        rt.ingest_history_fixture_spec(&E2eHistoryFixtureSpec {
            commit_sha: &sha_of(index),
            file_path: &path,
            author: "alice",
            committer: "alice",
            message: &format!("epoch commit {index}"),
            author_time_ms: time_of(index),
            committer_time_ms: time_of(index),
            applied_at_ms: 13,
            ref_name: &format!("refs/heads/epoch-{index}"),
            tag_name: &format!("epoch-{index}"),
            added_text: "added",
            removed_text: "",
            touched_text: "touched",
        })?;
    }
    let sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(sealed)
}

/// One commit-only history batch into `generation`, as a producer that
/// keeps publishing history for an active generation would send it.
fn late_commit_batch(
    rt: &E2eRuntime,
    generation: ManifestGeneration,
    index: u64,
    committer_time_ms: u64,
) -> Result<HistoryIngestBatch, Box<dyn Error>> {
    Ok(HistoryIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation,
        manifest_digest: None,
        batch_digest: format!("history-late:{index}"),
        commits: vec![CommitRecord {
            wire_version: 1,
            sha: CommitSha::from_hex(&sha_of(index))?,
            parents: Vec::new(),
            author_time_ms: committer_time_ms,
            committer_time_ms,
            applied_at_ms: 14,
            author: "bob".to_string().into_boxed_str(),
            author_name: None,
            author_email: None,
            committer: "bob".to_string().into_boxed_str(),
            committer_name: None,
            committer_email: None,
            message: format!("epoch late commit {index}").into_boxed_str(),
            is_merge: false,
            tags: Vec::new(),
        }],
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    })
}

fn served(page: E2eHistoryResult, what: &str) -> Result<E2eHistoryResult, Box<dyn Error>> {
    if let Some(error) = &page.typed_error {
        return Err(format!("{what}: history page refused: {error}").into());
    }
    Ok(page)
}

/// Walk from `cursor` to the end; every page must read `epoch`.
fn walk_from(
    rt: &mut E2eRuntime,
    mut cursor: Option<HistoryCursor>,
    epoch: AuxEpochV1,
    what: &str,
) -> Result<Vec<String>, Box<dyn Error>> {
    let mut seen = Vec::new();
    for _page in 0..16 {
        let page = served(
            rt.query_history_page(
                TextQuerySyntax::Sourcegraph,
                QUERY,
                PAGE,
                HistoryOrderV1::Recency,
                cursor,
            ),
            what,
        )?;
        if page.read_epoch != Some(epoch) {
            return Err(format!(
                "{what}: every page of one walk reads its epoch {epoch}, got {:?}",
                page.read_epoch
            )
            .into());
        }
        let window = page.window.ok_or("a served page carries a window")?;
        seen.extend(page.commit_ids);
        match (window.has_more(), page.next_cursor) {
            (true, Some(next)) => {
                if next.aux_epoch != epoch {
                    return Err(format!(
                        "{what}: the cursor names the epoch it was cut from, got {next:?}"
                    )
                    .into());
                }
                cursor = Some(next);
            }
            (false, None) => return Ok(seen),
            (has_more, next) => {
                return Err(
                    format!("{what}: has_more={has_more} and cursor={next:?} disagree").into(),
                );
            }
        }
    }
    Err(format!("{what}: pagination did not terminate").into())
}

fn distinct(rows: &[String]) -> BTreeSet<&String> {
    rows.iter().collect()
}

#[test]
fn a_page_walk_never_mixes_epochs_and_the_epoch_survives_a_restart() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let sealed = ingest_original_commits(&mut rt)?;
    let original: BTreeSet<String> = (1..=COMMITS).map(sha_of).collect();
    let late: BTreeSet<String> = (1..=LATE_COMMITS)
        .map(|late_index| sha_of(COMMITS.saturating_add(late_index)))
        .collect();

    // Page one at the current epoch.
    let first = served(
        rt.query_history_page(
            TextQuerySyntax::Sourcegraph,
            QUERY,
            PAGE,
            HistoryOrderV1::Recency,
            None,
        ),
        "page one",
    )?;
    let walk_epoch = first.read_epoch.ok_or("a served page names its epoch")?;
    let expected_first: Vec<String> = (1..=COMMITS).rev().take(10).map(sha_of).collect();
    if first.commit_ids != expected_first {
        return Err(format!(
            "page one is the ten newest commits, got {:?}",
            first.commit_ids
        )
        .into());
    }
    let cursor = first
        .next_cursor
        .ok_or("thirty commits continue past page one")?;
    if cursor.aux_epoch != walk_epoch {
        return Err(format!("the cursor names the epoch page one read, got {cursor:?}").into());
    }

    // Five commits land between page one and page two, each timed to sort
    // into page two by recency.
    for late_index in 1..=LATE_COMMITS {
        let batch = late_commit_batch(
            &rt,
            sealed,
            COMMITS.saturating_add(late_index),
            late_time_of(late_index),
        )?;
        rt.publish_history_batch(batch)?;
    }

    // The continuation reads the walk's epoch: pages two and three hold
    // exactly the original commits page one did not, none of the late ones.
    let rest = walk_from(&mut rt, Some(cursor.clone()), walk_epoch, "continuation")?;
    let mut walked = first.commit_ids;
    walked.extend(rest);
    if walked.len() != usize::try_from(COMMITS)? || distinct(&walked).len() != walked.len() {
        return Err(format!(
            "the walk yields the thirty original commits once each, got {} rows ({} distinct)",
            walked.len(),
            distinct(&walked).len()
        )
        .into());
    }
    let walked_set: BTreeSet<String> = walked.iter().cloned().collect();
    if walked_set != original {
        return Err(
            format!("the walk must be exactly epoch {walk_epoch}'s rows: {walked:?}").into(),
        );
    }
    if walked.iter().any(|sha| late.contains(sha)) {
        return Err("a commit ingested after the walk started leaked into it".into());
    }

    // A fresh walk reads the current epoch and sees all thirty-five.
    let fresh_first = served(
        rt.query_history_page(
            TextQuerySyntax::Sourcegraph,
            QUERY,
            PAGE,
            HistoryOrderV1::Recency,
            None,
        ),
        "fresh page one",
    )?;
    let fresh_epoch = fresh_first
        .read_epoch
        .ok_or("a served page names its epoch")?;
    if fresh_epoch <= walk_epoch {
        return Err(format!(
            "five ingests advance the epoch past {walk_epoch}, fresh walk read {fresh_epoch}"
        )
        .into());
    }
    let fresh = walk_from(&mut rt, None, fresh_epoch, "fresh walk")?;
    let fresh_set: BTreeSet<String> = fresh.iter().cloned().collect();
    let all: BTreeSet<String> = original.union(&late).cloned().collect();
    if fresh.len() != usize::try_from(COMMITS.saturating_add(LATE_COMMITS))? || fresh_set != all {
        return Err(format!(
            "a fresh walk sees all thirty-five commits once, got {} rows",
            fresh.len()
        )
        .into());
    }

    // Restart: the epoch is persisted with the rows, so the current epoch
    // continues; retention is not, so the old cursor is refused typed —
    // never served from the restored snapshot.
    let mut rt = rt.reopen();
    let after_restart = served(
        rt.query_history_page(
            TextQuerySyntax::Sourcegraph,
            QUERY,
            PAGE,
            HistoryOrderV1::Recency,
            None,
        ),
        "page one after restart",
    )?;
    if after_restart.read_epoch != Some(fresh_epoch) {
        return Err(format!(
            "the persisted epoch {fresh_epoch} continues after a restart, read {:?}",
            after_restart.read_epoch
        )
        .into());
    }
    let stale = rt.query_history_page(
        TextQuerySyntax::Sourcegraph,
        QUERY,
        PAGE,
        HistoryOrderV1::Recency,
        Some(cursor),
    );
    match stale.typed_error {
        Some(error) if error.code == AUX_EPOCH_EXPIRED_CODE => {}
        other => {
            return Err(format!(
                "a cursor from before the restart is refused {AUX_EPOCH_EXPIRED_CODE}, got {other:?}"
            )
            .into());
        }
    }

    // The next mutation after the restart takes the next epoch, not one a
    // pre-restart page already named.
    let batch = late_commit_batch(
        &rt,
        sealed,
        COMMITS.saturating_add(LATE_COMMITS).saturating_add(1),
        late_time_of(LATE_COMMITS.saturating_add(1)),
    )?;
    rt.publish_history_batch(batch)?;
    let advanced = served(
        rt.query_history_page(
            TextQuerySyntax::Sourcegraph,
            QUERY,
            PAGE,
            HistoryOrderV1::Recency,
            None,
        ),
        "page one after the post-restart ingest",
    )?;
    if advanced.read_epoch != fresh_epoch.checked_next() {
        return Err(format!(
            "the post-restart mutation is epoch {:?}, read {:?}",
            fresh_epoch.checked_next(),
            advanced.read_epoch
        )
        .into());
    }
    Ok(())
}
