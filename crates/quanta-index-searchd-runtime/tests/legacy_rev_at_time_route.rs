//! Pinned old-Q SDK truth for history-backed `rev:at.time` after restart.

#![forbid(unsafe_code)]

use std::error::Error;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusActiveHeadV1, SourceFileKey, SourcePublicationEvent, SymbolCoverage,
    lex::LanguageCode,
};
use quanta_index_sdk::{
    CommitRecord, CommitSha, HistoryBatch, QuantaIndex, SdkError, SearchCorpusBatch,
};
use quanta_index_searchd_harness::{
    fixture_source_scope_v1, legacy_reference::LegacySearchdBinary,
};

use crate::fail_closed_wait::{RealTicker, wait_for};

type TestResult = Result<(), Box<dyn Error>>;
const ANCESTOR_SHA: &str = "1111111111111111111111111111111111111111";
const HEAD_SHA: &str = "2222222222222222222222222222222222222222";

fn repo() -> Result<RepoId, Box<dyn Error>> {
    Ok(RepoId::new("repo-sdk")?)
}

fn revision(sha: &str) -> Result<RevisionId, Box<dyn Error>> {
    Ok(RevisionId::new(sha)?)
}

fn pin(sha: &str, generation: u64) -> Result<GenerationPin, Box<dyn Error>> {
    Ok(GenerationPin::new(
        repo()?,
        revision(sha)?,
        ManifestGeneration::new(generation),
    ))
}

fn now_epoch_ms() -> Result<u64, Box<dyn Error>> {
    Ok(u64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
    )?)
}

fn lexical_batch(
    sha: &str,
    generation: u64,
    event_id: &str,
    expected_base_event_id: Option<&str>,
    path: &str,
    candidate_id: &str,
    text: &str,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let source_repo_id = repo()?;
    let revision_id = revision(sha)?;
    let mut scope = fixture_source_scope_v1(
        SourceFileKey {
            source_repo_id: source_repo_id.clone(),
            repo_relative_path: RepoRelativePath::new(path),
        },
        revision_id.clone(),
        vec![ChunkRecord {
            chunk_id: ChunkId::new(candidate_id),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte: u32::try_from(text.len())?,
            start_line: 1,
            end_line: 1,
            text: text.to_owned().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        Vec::new(),
    )?;
    scope.coverage.symbols = SymbolCoverage::Complete { symbol_count: 0 };
    Ok(SearchCorpusBatch::replace_generation(
        source_repo_id,
        revision_id,
        ManifestGeneration::new(generation),
        format!("manifest:rev-at-time:{path}:{generation}"),
    )
    .source_event(SourcePublicationEvent {
        stream_id: "fixture:sdk-frontdoor".to_owned(),
        event_id: event_id.to_owned(),
        expected_base_event_id: expected_base_event_id.map(str::to_owned),
        payload_sha256: [0; 32],
    })
    .replace_scope(
        scope.coverage,
        scope.source_bytes,
        scope.chunks,
        scope.symbols,
    ))
}

fn history_batch() -> Result<HistoryBatch, Box<dyn Error>> {
    let now = now_epoch_ms()?;
    let ancestor = CommitSha::from_hex(ANCESTOR_SHA)?;
    let head = CommitSha::from_hex(HEAD_SHA)?;
    Ok(
        HistoryBatch::new(repo()?, revision(HEAD_SHA)?, ManifestGeneration::new(42))
            .manifest_digest("manifest:rev-at-time-history-sdk")
            .commit(CommitRecord {
                wire_version: 1,
                sha: ancestor.clone(),
                parents: Vec::new(),
                author_time_ms: now.saturating_sub(63_072_000_000),
                committer_time_ms: now.saturating_sub(63_072_000_000),
                applied_at_ms: now.saturating_sub(63_072_000_000),
                author: "alice".to_owned().into_boxed_str(),
                author_name: None,
                author_email: None,
                committer: "alice".to_owned().into_boxed_str(),
                committer_name: None,
                committer_email: None,
                message: "legacy rev-at-time commit".to_owned().into_boxed_str(),
                is_merge: false,
                tags: Vec::new(),
            })
            .commit(CommitRecord {
                wire_version: 1,
                sha: head.clone(),
                parents: vec![ancestor],
                author_time_ms: now.saturating_sub(12 * 60 * 60 * 1000),
                committer_time_ms: now.saturating_sub(12 * 60 * 60 * 1000),
                applied_at_ms: now.saturating_sub(12 * 60 * 60 * 1000),
                author: "alice".to_owned().into_boxed_str(),
                author_name: None,
                author_email: None,
                committer: "alice".to_owned().into_boxed_str(),
                committer_name: None,
                committer_email: None,
                message: "head rev-at-time commit".to_owned().into_boxed_str(),
                is_merge: false,
                tags: Vec::new(),
            })
            .ref_upsert("HEAD", head),
    )
}

fn expect_query(
    client: &QuantaIndex,
    query: &str,
    expected_pin: &GenerationPin,
    expected_path: Option<&str>,
    expected_id: Option<&str>,
) -> TestResult {
    let head_pin = pin(HEAD_SHA, 42)?;
    let response = wait_for(
        &RealTicker::new(),
        Duration::from_secs(20),
        Duration::from_millis(50),
        "rev:at.time selected generation ready",
        || {
            client
                .lexical()
                .query()
                .sourcegraph(query)
                .pinned(head_pin.clone())
                .top_k(10)
                .execute()
        },
        |response| response.generation == *expected_pin,
        |error| matches!(error, SdkError::Remote { code, .. } if code.as_wire_str() == "NOT_READY"),
    )?;
    let observed = response
        .results
        .iter()
        .map(|row| (row.repo_relative_path.as_str(), row.candidate_id.as_str()))
        .collect::<Vec<_>>();
    let expected = expected_path
        .zip(expected_id)
        .into_iter()
        .collect::<Vec<_>>();
    if observed != expected {
        return Err(format!("{query}: expected {expected:?}, got {observed:?}").into());
    }
    Ok(())
}

#[test]
fn sourcegraph_rev_at_time_rebinds_after_history_restart() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let first = binary.start(&state_root, 8)?;
    let published = (|| -> Result<SearchCorpusActiveHeadV1, Box<dyn Error>> {
        let client = first.connect()?;
        let (ancestor_receipt, ancestor_active) = client.search_corpus().publish_and_activate(
            &lexical_batch(
                ANCESTOR_SHA,
                41,
                "fixture:rev-at-time:ancestor",
                None,
                "src/legacy.rs",
                "chunk-rev-at-time-ancestor",
                "needle_token legacy_choice",
            )?,
            None,
        )?;
        if !ancestor_receipt.applied
            || ancestor_receipt.durable_sequence == 0
            || ancestor_active
                .active
                .generation
                .lexical
                .manifest_generation
                != ManifestGeneration::new(41)
        {
            return Err("ancestor generation did not activate".into());
        }
        let (head_receipt, head_active) = client.search_corpus().publish_and_activate(
            &lexical_batch(
                HEAD_SHA,
                42,
                "fixture:rev-at-time:head",
                Some("fixture:rev-at-time:ancestor"),
                "src/head.rs",
                "chunk-rev-at-time-head",
                "needle_token head_choice",
            )?,
            None,
        )?;
        if !head_receipt.applied
            || head_receipt.durable_sequence == 0
            || head_active.active.generation.lexical.manifest_generation
                != ManifestGeneration::new(42)
        {
            return Err("head generation did not activate".into());
        }
        let history = client.history().publish(&history_batch()?)?;
        if !history.applied || history.durable_sequence == 0 {
            return Err("rev:at.time History was not durably published".into());
        }
        Ok(head_active.active)
    })();
    let stopped = first.stop();
    let expected_head = published?;
    stopped?;
    binary.verify()?;

    let second = binary.start(&state_root, 8)?;
    let queried = (|| -> TestResult {
        let client = second.connect()?;
        if client
            .generations()
            .active_head(repo()?, revision(HEAD_SHA)?)?
            .as_ref()
            != Some(&expected_head)
        {
            return Err("restarted old-Q changed the exact head artifact pin".into());
        }
        let head = pin(HEAD_SHA, 42)?;
        let ancestor = pin(ANCESTOR_SHA, 41)?;
        expect_query(
            &client,
            "rev:at.time(2100-01-01T00:00:00Z) needle_token",
            &head,
            Some("src/head.rs"),
            Some("chunk-rev-at-time-head"),
        )?;
        expect_query(
            &client,
            "rev:at.time(1 year ago) needle_token",
            &ancestor,
            Some("src/legacy.rs"),
            Some("chunk-rev-at-time-ancestor"),
        )?;
        expect_query(
            &client,
            "rev:at.time(yesterday) needle_token",
            &ancestor,
            Some("src/legacy.rs"),
            Some("chunk-rev-at-time-ancestor"),
        )?;
        expect_query(
            &client,
            "rev:at.time(june 25 2017) needle_token",
            &head,
            None,
            None,
        )?;
        let invalid = client
            .lexical()
            .query()
            .sourcegraph("rev:at.time(definitely-not-a-timeref) needle_token")
            .pinned(head)
            .top_k(10)
            .execute();
        match invalid {
            Err(SdkError::Remote { code, message, .. })
                if code.as_wire_str() == "HISTORY_INVALID_TIMEREF"
                    && message.contains("timeref") =>
            {
                Ok(())
            }
            other => Err(format!(
                "invalid rev:at.time must fail with typed code and timeref message: {other:?}"
            )
            .into()),
        }
    })();
    let stopped = second.stop();
    queried.and(stopped)?;
    binary.verify()
}
