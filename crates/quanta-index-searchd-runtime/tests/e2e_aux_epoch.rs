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
    AuxEpochV1, ContinuationTokenV2, GenerationPin, HistoryIngestBatch, HistoryOrderV1,
    ManifestGeneration, RepoId, RevisionId, SearchPlaneHistoryQueryResponse,
};
use quanta_index_core::{AUX_EPOCH_EXPIRED_CODE, REPO_COMMIT_RECENCY_UNAVAILABLE_CODE};
use quanta_index_ipc::stamp_batch_digest_v1;
use quanta_index_sdk::{QuantaIndex, SdkError};
use quanta_index_searchd_harness::history_fixture::HistoryFixtureSpec;
use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, publish_legacy_source};
use quanta_index_searchd_harness::private_tempdir;

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

fn publish_original_commits(client: &QuantaIndex) -> Result<GenerationPin, Box<dyn Error>> {
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let generation = ManifestGeneration::new(1);
    let mut source_files = Vec::with_capacity(usize::try_from(COMMITS)?);
    for index in 1..=COMMITS {
        let path = format!("src/epoch_{index}.rs");
        // The original harness consumes one request ID for each staged chunk
        // and one for each following History publish.
        source_files.push((
            path.clone(),
            format!("fn epoch_{index}() {{}}"),
            index * 2 - 1,
        ));
        let mut batch = HistoryFixtureSpec {
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
        }
        .build_batch(repo.clone(), revision.clone(), generation)?;
        stamp_batch_digest_v1(&mut batch)?;
        let receipt = client.history().publish_history_borrowed_v1(&batch)?;
        if !receipt.applied || receipt.durable_sequence == 0 {
            return Err(format!("history fixture {index} was not durably published").into());
        }
    }
    let files = source_files
        .iter()
        .map(|(path, body, id)| (path.as_str(), body.as_str(), *id))
        .collect::<Vec<_>>();
    let (_head, _receipt) = publish_legacy_source(
        client,
        &repo,
        &revision,
        generation.get(),
        None,
        &files,
        None,
    )?;
    Ok(GenerationPin::new(repo, revision, generation))
}

/// One commit-only history batch into `generation`, as a producer that
/// keeps publishing history for an active generation would send it.
fn late_commit_batch(
    pin: &GenerationPin,
    index: u64,
    committer_time_ms: u64,
) -> Result<HistoryIngestBatch, Box<dyn Error>> {
    Ok(HistoryIngestBatch {
        repo_id: pin.repo_id.clone(),
        revision_id: pin.revision_id.clone(),
        generation: pin.manifest_generation,
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

fn publish_late_commit(
    client: &QuantaIndex,
    pin: &GenerationPin,
    index: u64,
    committer_time_ms: u64,
) -> TestResult {
    let mut batch = late_commit_batch(pin, index, committer_time_ms)?;
    stamp_batch_digest_v1(&mut batch)?;
    let receipt = client.history().publish_history_borrowed_v1(&batch)?;
    if !receipt.applied || receipt.durable_sequence == 0 {
        return Err(format!("late history commit {index} was not durably published").into());
    }
    Ok(())
}

fn query_page(
    client: &QuantaIndex,
    pin: &GenerationPin,
    cursor: Option<ContinuationTokenV2>,
) -> Result<SearchPlaneHistoryQueryResponse, SdkError> {
    let mut query = client
        .history()
        .query()
        .sourcegraph(QUERY)
        .pinned(pin.clone())
        .top_k(PAGE)
        .order(HistoryOrderV1::Recency);
    if let Some(cursor) = cursor {
        query = query.after(cursor);
    }
    query.execute()
}

/// Walk from `cursor` to the end; every page must read `epoch`.
fn walk_from(
    client: &QuantaIndex,
    pin: &GenerationPin,
    mut cursor: Option<ContinuationTokenV2>,
    epoch: AuxEpochV1,
    what: &str,
) -> Result<Vec<String>, Box<dyn Error>> {
    let mut seen = Vec::new();
    for _page in 0..16 {
        let page = query_page(client, pin, cursor)?;
        if page.generation != *pin || page.read_epoch != epoch {
            return Err(format!(
                "{what}: every page of one walk reads its epoch {epoch}, got {:?}",
                page.read_epoch
            )
            .into());
        }
        seen.extend(
            page.commits
                .into_iter()
                .map(|candidate| candidate.sha.to_string()),
        );
        match (page.window.has_more(), page.next_cursor) {
            (Some(true), Some(next)) => cursor = Some(next),
            (Some(false), None) => return Ok(seen),
            (has_more, next) => {
                return Err(
                    format!("{what}: has_more={has_more:?} and cursor={next:?} disagree").into(),
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
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let first = binary.start(root.path(), 8)?;
    let before = (|| -> Result<_, Box<dyn Error>> {
        let client = first.connect()?;
        before_restart(&client)
    })();
    let first_stopped = first.stop();
    let first_verified = binary.verify();
    let (pin, fresh_epoch, cursor) = before?;
    first_stopped?;
    first_verified?;

    let reopened = binary.start(root.path(), 8)?;
    let after = (|| -> TestResult {
        let client = reopened.connect()?;
        after_restart(&client, &pin, fresh_epoch, cursor)
    })();
    let reopened_stopped = reopened.stop();
    let final_verified = binary.verify();
    after?;
    reopened_stopped?;
    final_verified
}

fn before_restart(
    client: &QuantaIndex,
) -> Result<(GenerationPin, AuxEpochV1, ContinuationTokenV2), Box<dyn Error>> {
    let pin = publish_original_commits(client)?;
    let original: BTreeSet<String> = (1..=COMMITS).map(sha_of).collect();
    let late: BTreeSet<String> = (1..=LATE_COMMITS)
        .map(|late_index| sha_of(COMMITS.saturating_add(late_index)))
        .collect();

    // Page one at the current epoch.
    let first = query_page(client, &pin, None)?;
    if first.generation != pin {
        return Err("page one selected a different source generation".into());
    }
    let walk_epoch = first.read_epoch;
    let expected_first: Vec<String> = (1..=COMMITS).rev().take(10).map(sha_of).collect();
    let first_commit_ids = first
        .commits
        .iter()
        .map(|candidate| candidate.sha.to_string())
        .collect::<Vec<_>>();
    if first_commit_ids != expected_first {
        return Err(format!(
            "page one is the ten newest commits, got {:?}",
            first_commit_ids
        )
        .into());
    }
    let cursor = first
        .next_cursor
        .ok_or("thirty commits continue past page one")?;

    // Five commits land between page one and page two, each timed to sort
    // into page two by recency.
    for late_index in 1..=LATE_COMMITS {
        publish_late_commit(
            client,
            &pin,
            COMMITS.saturating_add(late_index),
            late_time_of(late_index),
        )?;
    }

    // The continuation reads the walk's epoch: pages two and three hold
    // exactly the original commits page one did not, none of the late ones.
    let rest = walk_from(
        client,
        &pin,
        Some(cursor.clone()),
        walk_epoch,
        "continuation",
    )?;
    let mut walked = first_commit_ids;
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
    let fresh_first = query_page(client, &pin, None)?;
    if fresh_first.generation != pin {
        return Err("fresh page selected a different source generation".into());
    }
    let fresh_epoch = fresh_first.read_epoch;
    if fresh_epoch <= walk_epoch {
        return Err(format!(
            "five ingests advance the epoch past {walk_epoch}, fresh walk read {fresh_epoch}"
        )
        .into());
    }
    let fresh = walk_from(client, &pin, None, fresh_epoch, "fresh walk")?;
    let fresh_set: BTreeSet<String> = fresh.iter().cloned().collect();
    let all: BTreeSet<String> = original.union(&late).cloned().collect();
    if fresh.len() != usize::try_from(COMMITS.saturating_add(LATE_COMMITS))? || fresh_set != all {
        return Err(format!(
            "a fresh walk sees all thirty-five commits once, got {} rows",
            fresh.len()
        )
        .into());
    }

    Ok((pin, fresh_epoch, cursor))
}

fn after_restart(
    client: &QuantaIndex,
    pin: &GenerationPin,
    fresh_epoch: AuxEpochV1,
    cursor: ContinuationTokenV2,
) -> TestResult {
    // The epoch is persisted with the rows, while cursor retention is not.
    let after_restart = query_page(client, pin, None)?;
    if after_restart.generation != *pin || after_restart.read_epoch != fresh_epoch {
        return Err(format!(
            "the persisted epoch {fresh_epoch} continues after a restart, read {:?}",
            after_restart.read_epoch
        )
        .into());
    }
    match query_page(client, pin, Some(cursor)) {
        Err(SdkError::Remote { code, .. }) if code == AUX_EPOCH_EXPIRED_CODE => {}
        other => {
            return Err(format!(
                "a cursor from before the restart is refused {AUX_EPOCH_EXPIRED_CODE}, got {other:?}"
            )
            .into());
        }
    }

    // The next mutation after the restart takes the next epoch, not one a
    // pre-restart page already named.
    publish_late_commit(
        client,
        pin,
        COMMITS.saturating_add(LATE_COMMITS).saturating_add(1),
        late_time_of(LATE_COMMITS.saturating_add(1)),
    )?;
    let advanced = query_page(client, pin, None)?;
    if advanced.generation != *pin || Some(advanced.read_epoch) != fresh_epoch.checked_next() {
        return Err(format!(
            "the post-restart mutation is epoch {:?}, read {:?}",
            fresh_epoch.checked_next(),
            advanced.read_epoch
        )
        .into());
    }
    Ok(())
}

/// History is available in this view, while commit-recency remains a
/// separately owned authority and must still refuse with its own code.
#[test]
fn an_ingested_history_domain_names_its_epoch_without_commit_recency() -> TestResult {
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(root.path(), 8)?;
    let checked = (|| -> TestResult {
        let client = process.connect()?;
        let repo = RepoId::new("repo-e2e")?;
        let revision = RevisionId::new("rev-e2e")?;
        let generation = ManifestGeneration::new(1);
        let mut history = HistoryFixtureSpec {
            commit_sha: "1111111111111111111111111111111111111111",
            file_path: "src/needle.rs",
            author: "alice",
            committer: "alice",
            message: "needle commit",
            author_time_ms: 1_000,
            committer_time_ms: 1_000,
            applied_at_ms: 13,
            ref_name: "refs/heads/main",
            tag_name: "v1",
            added_text: "needle",
            removed_text: "",
            touched_text: "needle",
        }
        .build_batch(repo.clone(), revision.clone(), generation)?;
        stamp_batch_digest_v1(&mut history)?;
        let receipt = client.history().publish_history_borrowed_v1(&history)?;
        if !receipt.applied || receipt.durable_sequence == 0 {
            return Err("one History mutation was not durably published".into());
        }
        let (_head, _source_receipt) = publish_legacy_source(
            &client,
            &repo,
            &revision,
            generation.get(),
            None,
            &[("src/needle.rs", "fn needle() { let needle = 1; }", 1)],
            None,
        )?;
        let pin = GenerationPin::new(repo, revision, generation);

        let page = client
            .history()
            .query()
            .native("type:commit needle")
            .pinned(pin.clone())
            .top_k(10)
            .order(HistoryOrderV1::Recency)
            .execute()?;
        if page.generation != pin || page.commits.len() != 1 {
            return Err(
                format!("the one commit was not served from the pinned view: {page:?}").into(),
            );
        }
        if page.commits[0].sha.to_string() != "1111111111111111111111111111111111111111" {
            return Err("History served a different commit".into());
        }
        if page.read_epoch.get() != 1 {
            return Err(format!(
                "first History mutation must read epoch 1, got {}",
                page.read_epoch
            )
            .into());
        }

        match client
            .lexical()
            .query()
            .sourcegraph("repo:has.commit.after(2020-01-01) needle")
            .pinned(pin)
            .top_k(10)
            .execute()
        {
            Err(SdkError::Remote { code, .. }) if code == REPO_COMMIT_RECENCY_UNAVAILABLE_CODE => {}
            other => {
                return Err(format!(
                    "History does not supply commit-recency authority; expected {REPO_COMMIT_RECENCY_UNAVAILABLE_CODE}, got {other:?}"
                )
                .into());
            }
        }
        Ok(())
    })();
    let stopped = process.stop();
    let verified = binary.verify();
    checked?;
    stopped?;
    verified
}
