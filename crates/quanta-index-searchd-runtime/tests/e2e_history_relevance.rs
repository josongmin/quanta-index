//! QI-BB-023 follow-up #1 — the history route's `relevance` order end to
//!
//! end: BM25 over the epoch's text index, checked against an in-test
//! reference; `recency` unchanged beside it; keyset pages that partition
//! the ranking; the same pages after a restart; and a walk in flight that
//! an ingest between its pages does not move.
//!
//! The reference BM25 is computed here from the fixture's messages with
//! the engine's fixed constants (`k1 = 1.2`, `b = 0.75`) and the shared
//! normalizer's boundaries for ASCII text; every message is under 40
//! tokens, where the engine's field-length quantization is exact.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;

use quanta_index_contract::lex::{CommitRecord, CommitSha};
use quanta_index_contract::{
    AuxEpochV1, CandidateCountV1, ContinuationTokenV2, GenerationPin, HistoryIngestBatch,
    HistoryOrderV1, HistoryQueryRequest, ManifestGeneration, QueryConstraintSetV1, RepoId,
    RevisionId, SearchPlaneErrorCodeV2, SearchPlaneHistoryQueryResponse, TextQueryRequest,
    TextQuerySyntax,
};
use quanta_index_ipc::stamp_batch_digest_v1;
use quanta_index_sdk::{QuantaIndex, SdkError};
use quanta_index_searchd_harness::{
    history_fixture::HistoryFixtureSpec,
    legacy_reference::{LegacySearchdBinary, publish_legacy_source, request_when_ready},
    private_tempdir as private_root,
};

type TestResult = Result<(), Box<dyn Error>>;

const K1: f32 = 1.2;
const B: f32 = 0.75;
const PAGE: u32 = 5;
const QUERY: &str = "type:commit needle";
/// The commit ingested between two pages: twelve mentions, denser than
/// any fixture commit, so it leads a fresh relevance walk.
const LATE_MESSAGE: &str =
    "needle needle needle needle needle needle needle needle needle needle needle needle";

/// One fixture commit: its index (sha and time derive from it) and its
/// message.
struct FixtureCommit {
    index: u64,
    message: &'static str,
}

/// Twenty commits mention `needle` with varying frequency and length,
/// two do not.
///
/// Times ascend with the index, so the newest commits are the last ones;
/// the densest mentions sit in the middle, so recency and relevance
/// disagree at the head.
const FIXTURE: &[FixtureCommit] = &[
    FixtureCommit {
        index: 1,
        message: "needle",
    },
    FixtureCommit {
        index: 2,
        message: "a needle and a thread",
    },
    FixtureCommit {
        index: 3,
        message: "needle needle",
    },
    FixtureCommit {
        index: 4,
        message: "this long message mentions the needle once and then goes on about other things",
    },
    FixtureCommit {
        index: 5,
        message: "needle needle needle",
    },
    FixtureCommit {
        index: 6,
        message: "refactor the needle module for clarity",
    },
    FixtureCommit {
        index: 7,
        message: "needle needle needle needle",
    },
    FixtureCommit {
        index: 8,
        message: "fix: needle parsing edge case",
    },
    FixtureCommit {
        index: 9,
        message: "nothing relevant in this one",
    },
    FixtureCommit {
        index: 10,
        message: "needle needle needle needle needle",
    },
    FixtureCommit {
        index: 11,
        message: "docs: describe the needle api",
    },
    FixtureCommit {
        index: 12,
        message: "needle needle in a haystack",
    },
    FixtureCommit {
        index: 13,
        message: "chore: bump needle version",
    },
    FixtureCommit {
        index: 14,
        message: "another commit without the word",
    },
    FixtureCommit {
        index: 15,
        message: "needle needle needle needle needle needle",
    },
    FixtureCommit {
        index: 16,
        message: "test: cover the needle path",
    },
    FixtureCommit {
        index: 17,
        message: "needle tweaks",
    },
    FixtureCommit {
        index: 18,
        message: "a needle, a thread, a needle",
    },
    FixtureCommit {
        index: 19,
        message: "the newest commit but a single needle in a long sentence about many unrelated matters",
    },
    FixtureCommit {
        index: 20,
        message: "needle at the very end of the line",
    },
    FixtureCommit {
        index: 21,
        message: "needle needle needle at the tail",
    },
    FixtureCommit {
        index: 22,
        message: "final commit mentioning the needle only once here",
    },
];

fn sha_of(index: u64) -> String {
    format!("{index:0>40}")
}

fn time_of(index: u64) -> u64 {
    1_000_u64.saturating_add(index.saturating_mul(10))
}

fn publish_history(client: &QuantaIndex, mut batch: HistoryIngestBatch) -> TestResult {
    stamp_batch_digest_v1(&mut batch)?;
    let receipt = client.history().publish_history_borrowed_v1(&batch)?;
    if receipt.generation != batch.generation || !receipt.applied || receipt.durable_sequence == 0 {
        return Err("History batch was not durably applied to the requested generation".into());
    }
    Ok(())
}

fn ingest_fixture(client: &QuantaIndex, generation: &GenerationPin) -> TestResult {
    let mut files = Vec::with_capacity(FIXTURE.len());
    for (offset, commit) in FIXTURE.iter().enumerate() {
        let path = format!("src/relevance_{}.rs", commit.index);
        let text = format!("fn relevance_{}() {{}}", commit.index);
        // The hunk text repeats the term as often as the message, so the
        // diff index ranks like the commit index.
        let needles = commit.message.matches("needle").count();
        let touched = std::iter::repeat_n("needle", needles)
            .collect::<Vec<_>>()
            .join(" ");
        let batch = HistoryFixtureSpec {
            commit_sha: &sha_of(commit.index),
            file_path: &path,
            author: "alice",
            committer: "alice",
            message: commit.message,
            author_time_ms: time_of(commit.index),
            committer_time_ms: time_of(commit.index),
            applied_at_ms: 13,
            ref_name: &format!("refs/heads/relevance-{}", commit.index),
            tag_name: &format!("relevance-{}", commit.index),
            added_text: "added",
            removed_text: "",
            touched_text: if touched.is_empty() {
                "touched"
            } else {
                &touched
            },
        }
        .build_batch(
            generation.repo_id.clone(),
            generation.revision_id.clone(),
            generation.manifest_generation,
        )?;
        publish_history(client, batch)?;
        // The original harness allocated one chunk ID, then one History request ID.
        files.push((path, text, u64::try_from(offset)? * 2 + 1));
    }
    let source_files = files
        .iter()
        .map(|(path, text, id)| (path.as_str(), text.as_str(), *id))
        .collect::<Vec<_>>();
    let _published = publish_legacy_source(
        client,
        &generation.repo_id,
        &generation.revision_id,
        generation.manifest_generation.get(),
        None,
        &source_files,
        None,
    )?;
    Ok(())
}

/// The shared normalizer's boundaries and fold for ASCII text.
fn tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|run| !run.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// The reference relevance ranking of the fixture for `term`:
/// `(sha, score)` by score descending, then time descending, then sha
/// ascending.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::imprecise_flops,
    reason = "the reference computes from integer counts in f32 with the engine's own `ln(1 + x)` idf, so its scores are the engine's bit for bit"
)]
fn reference_ranking(commits: &[(u64, String)], term: &str) -> Vec<(String, f32)> {
    let tokenized: Vec<Vec<String>> = commits
        .iter()
        .map(|(_index, message)| tokens(message))
        .collect();
    let total_docs = tokenized.len() as f32;
    let total_tokens: usize = tokenized.iter().map(Vec::len).sum();
    let average_length = total_tokens as f32 / total_docs;
    let df = tokenized
        .iter()
        .filter(|doc| doc.iter().any(|token| token == term))
        .count() as f32;
    let idf = (1.0_f32 + (total_docs - df + 0.5) / (df + 0.5)).ln();
    let weight = idf * (1.0 + K1);
    let mut ranked: Vec<(u64, f32)> = commits
        .iter()
        .zip(&tokenized)
        .filter_map(|((index, _message), doc)| {
            let occurrences = doc.iter().filter(|token| *token == term).count();
            if occurrences == 0 {
                return None;
            }
            let tf = occurrences as f32;
            let norm = K1 * (1.0 - B + B * doc.len() as f32 / average_length);
            Some((*index, weight * (tf / (tf + norm))))
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| time_of(right.0).cmp(&time_of(left.0)))
            .then_with(|| sha_of(left.0).cmp(&sha_of(right.0)))
    });
    ranked
        .into_iter()
        .map(|(index, score)| (sha_of(index), score))
        .collect()
}

fn fixture_messages() -> Vec<(u64, String)> {
    FIXTURE
        .iter()
        .map(|commit| (commit.index, commit.message.to_string()))
        .collect()
}

struct PinnedHistory<'a> {
    client: &'a QuantaIndex,
    generation: &'a GenerationPin,
}

impl PinnedHistory<'_> {
    fn query_history_page(
        &self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
        order: HistoryOrderV1,
        cursor: Option<ContinuationTokenV2>,
    ) -> Result<SearchPlaneHistoryQueryResponse, SdkError> {
        let request = HistoryQueryRequest {
            text_query: TextQueryRequest {
                syntax,
                query_text: query_text.into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(self.generation.clone()),
                generation_selector: None,
                top_k,
                cursor: None,
            },
            order,
            cursor,
        };
        request_when_ready(|| self.client.history().query_request(request.clone()))
    }

    fn served(
        &self,
        page: Result<SearchPlaneHistoryQueryResponse, SdkError>,
        what: &str,
    ) -> Result<SearchPlaneHistoryQueryResponse, Box<dyn Error>> {
        let page = page.map_err(|error| format!("{what}: history page refused: {error}"))?;
        if page.generation != *self.generation {
            return Err(format!("{what}: history page used another generation").into());
        }
        Ok(page)
    }
}

/// `(sha, score)` per row of a relevance page.
fn scored_rows(
    page: &SearchPlaneHistoryQueryResponse,
    what: &str,
) -> Result<Vec<(String, f32)>, Box<dyn Error>> {
    if page.order != HistoryOrderV1::Relevance {
        return Err(format!(
            "{what}: the page echoes the relevance order, got {:?}",
            page.order
        )
        .into());
    }
    if !page.diffs.is_empty() {
        return Err(format!("{what}: relevance commit page returned diff rows").into());
    }
    page.commits
        .iter()
        .map(|commit| {
            commit
                .score
                .map(|score| (commit.sha.to_hex(), score.get()))
                .ok_or_else(|| format!("{what}: a relevance row carries its score").into())
        })
        .collect()
}

fn assert_rows_match_reference(
    observed: &[(String, f32)],
    expected: &[(String, f32)],
    what: &str,
) -> TestResult {
    let observed_shas: Vec<&String> = observed.iter().map(|(sha, _)| sha).collect();
    let expected_shas: Vec<&String> = expected.iter().map(|(sha, _)| sha).collect();
    if observed_shas != expected_shas {
        return Err(format!(
            "{what}: ranking differs from the reference:\n  daemon    {observed_shas:?}\n  reference {expected_shas:?}"
        )
        .into());
    }
    for ((sha, score), (_, reference)) in observed.iter().zip(expected) {
        let tolerance = reference.abs().max(1.0) * 1e-4;
        if (score - reference).abs() > tolerance {
            return Err(format!(
                "{what}: score of {sha} differs from the reference: daemon {score} reference {reference}"
            )
            .into());
        }
    }
    Ok(())
}

/// What one relevance walk yielded.
struct RelevanceWalk {
    /// Every `(sha, score)` in page order.
    rows: Vec<(String, f32)>,
    /// The first page's continuation, when there was more than one page.
    first_cursor: Option<ContinuationTokenV2>,
    /// The epoch every page read.
    epoch: Option<AuxEpochV1>,
}

/// Walk relevance pages from the start.
fn walk_relevance(
    history: &PinnedHistory<'_>,
    expected_total: u64,
    what: &str,
) -> Result<RelevanceWalk, Box<dyn Error>> {
    let mut cursor: Option<ContinuationTokenV2> = None;
    let mut first_cursor: Option<ContinuationTokenV2> = None;
    let mut epoch: Option<AuxEpochV1> = None;
    let mut walked = Vec::new();
    for _page in 0..16 {
        let page = history.served(
            history.query_history_page(
                TextQuerySyntax::Sourcegraph,
                QUERY,
                PAGE,
                HistoryOrderV1::Relevance,
                cursor.clone(),
            ),
            what,
        )?;
        let window = &page.window;
        let remaining = expected_total.saturating_sub(u64::try_from(walked.len())?);
        if window.candidate_count() != CandidateCountV1::Exact(remaining) {
            return Err(format!(
                "{what}: each page counts exactly the matches after its cursor: {window:?} with {remaining} remaining"
            )
            .into());
        }
        if epoch.is_none() {
            epoch = Some(page.read_epoch);
        } else if Some(page.read_epoch) != epoch {
            return Err(format!("{what}: every page of one walk reads one epoch").into());
        }
        walked.extend(scored_rows(&page, what)?);
        match (window.has_more(), page.next_cursor) {
            (Some(true), Some(next)) => {
                if first_cursor.is_none() {
                    first_cursor = Some(next.clone());
                }
                cursor = Some(next);
            }
            (Some(false), None) => {
                return Ok(RelevanceWalk {
                    rows: walked,
                    first_cursor,
                    epoch,
                });
            }
            (has_more, next) => {
                return Err(
                    format!("{what}: has_more={has_more:?} and cursor={next:?} disagree").into(),
                );
            }
        }
    }
    Err(format!("{what}: pagination did not terminate").into())
}

fn late_commit_batch(generation: &GenerationPin) -> Result<HistoryIngestBatch, Box<dyn Error>> {
    Ok(HistoryIngestBatch {
        repo_id: generation.repo_id.clone(),
        revision_id: generation.revision_id.clone(),
        generation: generation.manifest_generation,
        manifest_digest: None,
        batch_digest: "history-relevance-late".to_string(),
        commits: vec![CommitRecord {
            wire_version: 1,
            sha: CommitSha::from_hex(&sha_of(99))?,
            parents: Vec::new(),
            author_time_ms: time_of(99),
            committer_time_ms: time_of(99),
            applied_at_ms: 14,
            author: "bob".to_string().into_boxed_str(),
            author_name: None,
            author_email: None,
            committer: "bob".to_string().into_boxed_str(),
            committer_name: None,
            committer_email: None,
            // Denser than anything in the fixture: it leads a fresh
            // relevance walk.
            message: LATE_MESSAGE.to_string().into_boxed_str(),
            is_merge: false,
            tags: Vec::new(),
        }],
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    })
}

struct BeforeRestart {
    matching: u64,
    epoch: AuxEpochV1,
    cursor: ContinuationTokenV2,
}

fn verify_before_restart(history: &PinnedHistory<'_>) -> Result<BeforeRestart, Box<dyn Error>> {
    let reference = reference_ranking(&fixture_messages(), "needle");
    let matching = u64::try_from(reference.len())?;
    if matching != 20 {
        return Err(
            format!("twenty fixture commits mention needle, reference has {matching}").into(),
        );
    }

    // Relevance top-3 is the reference's top-3, scored; recency top-3 is
    // the newest three, unscored; the two disagree at the head.
    let top = history.served(
        history.query_history_page(
            TextQuerySyntax::Sourcegraph,
            QUERY,
            3,
            HistoryOrderV1::Relevance,
            None,
        ),
        "relevance top-3",
    )?;
    let top_rows = scored_rows(&top, "relevance top-3")?;
    assert_rows_match_reference(
        &top_rows,
        reference.get(..3).ok_or("three expected")?,
        "relevance top-3",
    )?;
    let top_window = &top.window;
    if top_window.candidate_count() != CandidateCountV1::Exact(matching)
        || top_window.has_more() != Some(true)
    {
        return Err(format!("the relevance window is exact: {top_window:?}").into());
    }
    if top.examined < matching {
        return Err(format!("every matching commit is examined, saw {}", top.examined).into());
    }
    let newest = history.served(
        history.query_history_page(
            TextQuerySyntax::Sourcegraph,
            QUERY,
            3,
            HistoryOrderV1::Recency,
            None,
        ),
        "recency top-3",
    )?;
    let newest_shas = newest
        .commits
        .iter()
        .map(|commit| commit.sha.to_hex())
        .collect::<Vec<_>>();
    if newest_shas != [sha_of(22), sha_of(21), sha_of(20)] {
        return Err(format!("recency serves the newest three: {newest_shas:?}").into());
    }
    if newest.order != HistoryOrderV1::Recency
        || newest.commits.iter().any(|commit| commit.score.is_some())
    {
        return Err("a recency page echoes its order and carries no score".into());
    }
    if top.commits.first().map(|row| row.sha) == newest.commits.first().map(|row| row.sha) {
        return Err("the fixture must make the two orders disagree at the head".into());
    }

    // Paging by five walks every match once, in the reference order.
    let RelevanceWalk {
        rows: walked,
        first_cursor,
        epoch,
    } = walk_relevance(history, matching, "walk")?;
    assert_rows_match_reference(&walked, &reference, "walk")?;
    let distinct: BTreeSet<&String> = walked.iter().map(|(sha, _)| sha).collect();
    if distinct.len() != walked.len() {
        return Err("a commit appeared on two pages".into());
    }
    let first_cursor = first_cursor.ok_or("a twenty-row walk has more than one page")?;
    let epoch = epoch.ok_or("a served page names its epoch")?;

    // The diff index ranks the hunks the same way, scored.
    let diffs = history.served(
        history.query_history_page(
            TextQuerySyntax::Sourcegraph,
            "type:diff needle",
            3,
            HistoryOrderV1::Relevance,
            None,
        ),
        "diff relevance",
    )?;
    if diffs.order != HistoryOrderV1::Relevance
        || diffs.diffs.len() != 3
        || !diffs.commits.is_empty()
        || diffs.diffs.iter().any(|row| row.score.is_none())
    {
        return Err(format!("diff relevance serves scored hunks: {diffs:?}").into());
    }
    let diff_scores: Vec<f32> = diffs
        .diffs
        .iter()
        .filter_map(|row| row.score.map(|score| score.get()))
        .collect();
    if diff_scores
        .windows(2)
        .any(|pair| matches!(pair, [first, second] if first < second))
    {
        return Err(format!("diff scores descend: {diff_scores:?}").into());
    }

    // An ingest between page one and page two neither moves the walk in
    // flight nor changes its scores: the continuation is served at its
    // epoch from that epoch's index. A fresh walk sees the new commit
    // first at the next epoch.
    publish_history(history.client, late_commit_batch(history.generation)?)?;
    let continued = history.served(
        history.query_history_page(
            TextQuerySyntax::Sourcegraph,
            QUERY,
            PAGE,
            HistoryOrderV1::Relevance,
            Some(first_cursor.clone()),
        ),
        "continuation after ingest",
    )?;
    if continued.read_epoch != epoch {
        return Err(format!(
            "the continuation reads epoch {epoch}, got {:?}",
            continued.read_epoch
        )
        .into());
    }
    let continued_rows = scored_rows(&continued, "continuation after ingest")?;
    let page_two = walked.get(5..10).ok_or("ten rows walked")?;
    if continued_rows
        .iter()
        .zip(page_two)
        .any(|((sha, score), (want_sha, want))| {
            sha != want_sha || score.to_bits() != want.to_bits()
        })
        || continued_rows.len() != page_two.len()
    {
        return Err(format!(
            "the continuation is page two of the original walk, bit for bit:\n  got  {continued_rows:?}\n  want {page_two:?}"
        )
        .into());
    }
    let fresh = history.served(
        history.query_history_page(
            TextQuerySyntax::Sourcegraph,
            QUERY,
            1,
            HistoryOrderV1::Relevance,
            None,
        ),
        "fresh walk after ingest",
    )?;
    if fresh.read_epoch != AuxEpochV1::new(epoch.get().saturating_add(1))
        || !matches!(fresh.commits.as_slice(), [row] if row.sha.to_hex() == sha_of(99))
    {
        return Err(format!(
            "a fresh walk reads the next epoch and ranks the dense commit first: {fresh:?}"
        )
        .into());
    }

    Ok(BeforeRestart {
        matching,
        epoch,
        cursor: first_cursor,
    })
}

fn verify_after_restart(history: &PinnedHistory<'_>, before: BeforeRestart) -> TestResult {
    // A restart serves the same pages with the same scores: the index is
    // a durable property of the epoch. The walk in flight is gone with
    // the restart (retention is in-memory) and refused typed.
    let RelevanceWalk {
        rows: after_restart,
        first_cursor: _,
        epoch: restarted_epoch,
    } = walk_relevance(history, before.matching.saturating_add(1), "after restart")?;
    let Some(restarted_epoch) = restarted_epoch else {
        return Err("a served page names its epoch".into());
    };
    if restarted_epoch != AuxEpochV1::new(before.epoch.get().saturating_add(1)) {
        return Err(format!(
            "the persisted epoch is served after a restart, got {restarted_epoch}"
        )
        .into());
    }
    let reference_after: Vec<(String, f32)> = {
        let mut messages = fixture_messages();
        messages.push((99, LATE_MESSAGE.to_string()));
        reference_ranking(&messages, "needle")
    };
    assert_rows_match_reference(&after_restart, &reference_after, "after restart")?;
    let stale = history.query_history_page(
        TextQuerySyntax::Sourcegraph,
        QUERY,
        PAGE,
        HistoryOrderV1::Relevance,
        Some(before.cursor),
    );
    match stale {
        Err(SdkError::Remote { code, .. }) if code == quanta_index_core::AUX_EPOCH_EXPIRED_CODE => {
        }
        other => {
            return Err(format!("a pre-restart cursor is refused expired, got {other:?}").into());
        }
    }

    // A recency cursor cannot continue a relevance walk, nor the reverse.
    let recency = history.served(
        history.query_history_page(
            TextQuerySyntax::Sourcegraph,
            QUERY,
            3,
            HistoryOrderV1::Recency,
            None,
        ),
        "recency for a cursor",
    )?;
    let recency_cursor = recency.next_cursor.ok_or("recency continues")?;
    let mismatch = history.query_history_page(
        TextQuerySyntax::Sourcegraph,
        QUERY,
        3,
        HistoryOrderV1::Relevance,
        Some(recency_cursor),
    );
    match mismatch {
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::HistoryCursorOrderMismatch,
            ..
        }) => {}
        other => {
            return Err(format!(
                "a recency cursor on a relevance walk is refused typed, got {other:?}"
            )
            .into());
        }
    }
    // A raw string has no BM25 score: refused under relevance, a filter
    // under recency.
    let unscorable = history.query_history_page(
        TextQuerySyntax::Native,
        "type:commit 'needle'",
        3,
        HistoryOrderV1::Relevance,
        None,
    );
    match unscorable {
        Err(SdkError::Remote { code, .. })
            if code == quanta_index_core::HISTORY_TEXT_QUERY_UNSCORABLE_CODE => {}
        other => {
            return Err(
                format!("a raw string is unscorable under relevance, got {other:?}").into(),
            );
        }
    }
    let filtered = history.served(
        history.query_history_page(
            TextQuerySyntax::Native,
            "type:commit 'needle'",
            3,
            HistoryOrderV1::Recency,
            None,
        ),
        "raw string under recency",
    )?;
    if filtered.commits.len() != 3 || !filtered.diffs.is_empty() {
        return Err(format!("a raw string filters under recency: {filtered:?}").into());
    }
    Ok(())
}

#[test]
fn relevance_ranks_by_bm25_recency_by_time_and_pages_survive_restarts_and_ingests() -> TestResult {
    let parent = private_root()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let generation = GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    );

    let first = binary.start(&state_root, 8)?;
    let before = (|| {
        let client = first.connect()?;
        ingest_fixture(&client, &generation)?;
        verify_before_restart(&PinnedHistory {
            client: &client,
            generation: &generation,
        })
    })();
    let stopped = first.stop();
    let before = before?;
    stopped?;
    binary.verify()?;

    let second = binary.start(&state_root, 8)?;
    let restarted = (|| {
        let client = second.connect()?;
        verify_after_restart(
            &PinnedHistory {
                client: &client,
                generation: &generation,
            },
            before,
        )
    })();
    let stopped = second.stop();
    restarted.and(stopped)?;
    binary.verify()
}
