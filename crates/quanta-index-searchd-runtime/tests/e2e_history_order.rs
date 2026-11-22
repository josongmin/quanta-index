//! QI-BB-023 — pinned Legacy SDK History pages follow recency, not SHA,
//! and a cursor walks every match exactly once across a real process restart.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;

use quanta_index_contract::{
    CandidateCountV1, ContinuationTokenV2, GenerationPin, HistoryOrderV1, ManifestGeneration,
    RepoId, RevisionId,
};
use quanta_index_ipc::stamp_batch_digest_v1;
use quanta_index_sdk::QuantaIndex;
use quanta_index_searchd_harness::{
    history_fixture::HistoryFixtureSpec,
    legacy_reference::{LegacySearchdBinary, publish_legacy_source},
    private_tempdir as private_root,
};

type TestResult = Result<(), Box<dyn Error>>;

const COMMITS: u64 = 5;
const QUERY: &str = "type:commit order";
// In ingestion/time order: SHA IDs 3, 1, 5, 2, 4. Sorting SHA IDs cannot
// produce the newest-first order 4, 2, 5, 1, 3.
const SHA_IDS_BY_TIME: [u64; 5] = [3, 1, 5, 2, 4];

fn sha_of(id: u64) -> String {
    format!("{id:0>40}")
}

fn ingest_commits(client: &QuantaIndex, generation: &GenerationPin) -> TestResult {
    let mut files = Vec::new();
    for (offset, sha_id) in SHA_IDS_BY_TIME.into_iter().enumerate() {
        let index = u64::try_from(offset + 1)?;
        let path = format!("src/order_{index}.rs");
        let text = format!("fn order_{index}() {{}}");
        let sha = sha_of(sha_id);
        let message = format!("order commit {index}");
        let ref_name = format!("refs/heads/order-{index}");
        let tag_name = format!("order-{index}");
        let mut batch = HistoryFixtureSpec {
            commit_sha: &sha,
            file_path: &path,
            author: "alice",
            committer: "alice",
            message: &message,
            author_time_ms: 1_000_u64.saturating_add(index),
            committer_time_ms: 1_000_u64.saturating_add(index),
            applied_at_ms: 13,
            ref_name: &ref_name,
            tag_name: &tag_name,
            added_text: "added",
            removed_text: "",
            touched_text: "touched",
        }
        .build_batch(
            generation.repo_id.clone(),
            generation.revision_id.clone(),
            generation.manifest_generation,
        )?;
        stamp_batch_digest_v1(&mut batch)?;
        let receipt = client.history().publish_history_borrowed_v1(&batch)?;
        if !receipt.applied || receipt.durable_sequence == 0 {
            return Err(format!("History commit {index} was not durably published").into());
        }
        // The original harness consumes one request ID for the chunk and
        // one for the following History publish.
        files.push((path, text, index.saturating_mul(2).saturating_sub(1)));
    }
    let scopes = files
        .iter()
        .map(|(path, text, index)| (path.as_str(), text.as_str(), *index))
        .collect::<Vec<_>>();
    let _published = publish_legacy_source(
        client,
        &generation.repo_id,
        &generation.revision_id,
        generation.manifest_generation.get(),
        None,
        &scopes,
        None,
    )?;
    Ok(())
}

/// Walk every page and return the commit ids in page order.
fn walk_pages(
    client: &QuantaIndex,
    generation: &GenerationPin,
    page_size: u32,
) -> Result<Vec<String>, Box<dyn Error>> {
    let mut cursor: Option<ContinuationTokenV2> = None;
    let mut seen = Vec::new();
    for _page in 0..16 {
        let mut query = client
            .history()
            .query()
            .sourcegraph(QUERY)
            .pinned(generation.clone())
            .top_k(page_size)
            .order(HistoryOrderV1::Recency);
        if let Some(next) = cursor.take() {
            query = query.after(next);
        }
        let page = query.execute()?;
        if page.generation != *generation {
            return Err("History page returned another generation".into());
        }
        let window = page.window;
        if window.candidate_count()
            != CandidateCountV1::Exact(COMMITS.saturating_sub(u64::try_from(seen.len())?))
        {
            return Err(format!(
                "each page counts exactly the matches after its cursor: {window:?} after {} seen",
                seen.len()
            )
            .into());
        }
        seen.extend(page.commits.into_iter().map(|commit| commit.sha.to_hex()));
        match (window.has_more(), page.next_cursor) {
            (Some(true), Some(next)) => cursor = Some(next),
            (Some(false), None) => return Ok(seen),
            (has_more, next) => {
                return Err(format!("has_more={has_more:?} and cursor={next:?} disagree").into());
            }
        }
    }
    Err("pagination did not terminate".into())
}

#[test]
fn top_k_returns_the_newest_commits_and_pages_walk_every_match_once() -> TestResult {
    let parent = private_root()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let expected_top_two = [4, 2].map(sha_of);
    let expected: Vec<String> = [4, 2, 5, 1, 3].map(sha_of).into();
    let generation = GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    );

    let first = binary.start(&state_root, 8)?;
    let first_result = (|| -> TestResult {
        let client = first.connect()?;
        ingest_commits(&client, &generation)?;
        let top_two = client
            .history()
            .query()
            .sourcegraph(QUERY)
            .pinned(generation.clone())
            .top_k(2)
            .order(HistoryOrderV1::Recency)
            .execute()?;
        let observed_top_two = top_two
            .commits
            .iter()
            .map(|commit| commit.sha.to_hex())
            .collect::<Vec<_>>();
        if observed_top_two.as_slice() != expected_top_two.as_slice() {
            return Err(format!(
                "top-2 must be the two newest commits {expected_top_two:?}, got {observed_top_two:?}"
            )
            .into());
        }
        if top_two.examined < COMMITS {
            return Err(format!("every commit is examined, saw {}", top_two.examined).into());
        }

        let walked = walk_pages(&client, &generation, 2)?;
        if walked != expected {
            return Err(format!("pages must walk every match once in order: {walked:?}").into());
        }
        let distinct: BTreeSet<&String> = walked.iter().collect();
        if distinct.len() != walked.len() {
            return Err("a commit appeared on two pages".into());
        }
        Ok(())
    })();
    let stopped = first.stop();
    first_result.and(stopped)?;
    binary.verify()?;

    let second = binary.start(&state_root, 8)?;
    let restarted = (|| -> TestResult {
        let client = second.connect()?;
        let after_restart = walk_pages(&client, &generation, 2)?;
        if after_restart != expected {
            return Err(format!("pages after a restart drifted: {after_restart:?}").into());
        }
        Ok(())
    })();
    let stopped = second.stop();
    restarted.and(stopped)?;
    binary.verify()
}
