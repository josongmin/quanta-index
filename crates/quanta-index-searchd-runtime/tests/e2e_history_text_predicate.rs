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

use quanta_index_contract::{
    CandidateCountV1, GenerationPin, HistoryOrderV1, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneHistoryQueryResponse,
};
use quanta_index_ipc::stamp_batch_digest_v1;
use quanta_index_sdk::{QuantaIndex, SdkError};
use quanta_index_searchd_harness::history_fixture::HistoryFixtureSpec;
use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, publish_legacy_source};
use quanta_index_searchd_harness::private_tempdir;

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

fn publish_fixture(client: &QuantaIndex) -> Result<GenerationPin, Box<dyn Error>> {
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let generation = ManifestGeneration::new(1);
    let mut source_files = Vec::with_capacity(FIXTURE.len());
    for (index, message) in FIXTURE {
        let path = path_of(*index);
        // The original harness consumes one request ID for each staged chunk
        // and one for each following History publish: chunk IDs are 1,3,5,7.
        source_files.push((
            path.clone(),
            format!("fn predicate_{index}() {{}}"),
            *index * 2 - 1,
        ));
        // The hunk's touched text is the message, so the diff kind mirrors
        // the commit kind.
        let mut batch = HistoryFixtureSpec {
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
        }
        .build_batch(repo.clone(), revision.clone(), generation)?;
        stamp_batch_digest_v1(&mut batch)?;
        let receipt = client.history().publish_history_borrowed_v1(&batch)?;
        if !receipt.applied || receipt.durable_sequence == 0 {
            return Err(format!("history fixture {index} was not durably published").into());
        }
    }
    // The original harness publishes History before sealing and activating
    // the staged Source generation. Keep that order through the real SDK.
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

/// The commit shas or diff paths a served page holds.
fn rows(page: &SearchPlaneHistoryQueryResponse) -> BTreeSet<String> {
    page.commits
        .iter()
        .map(|candidate| candidate.sha.to_string())
        .chain(
            page.diffs
                .iter()
                .map(|candidate| candidate.repo_relative_path.as_str().to_string()),
        )
        .collect()
}

fn query(
    client: &QuantaIndex,
    pin: &GenerationPin,
    text: &str,
    order: HistoryOrderV1,
) -> Result<SearchPlaneHistoryQueryResponse, SdkError> {
    client
        .history()
        .query()
        .native(text)
        .pinned(pin.clone())
        .top_k(TOP_K)
        .order(order)
        .execute()
}

/// One query under both orders: the same rows and the same exact total.
fn assert_orders_agree(
    client: &QuantaIndex,
    pin: &GenerationPin,
    query: &str,
    expected: &BTreeSet<String>,
) -> TestResult {
    let want_total = CandidateCountV1::Exact(u64::try_from(expected.len())?);
    for order in HistoryOrderV1::ALL {
        let page = self::query(client, pin, query, order)?;
        if page.generation != *pin {
            return Err(format!("{query:?} under {order}: wrong generation").into());
        }
        let observed = rows(&page);
        if observed != *expected {
            return Err(format!(
                "{query:?} under {order}: rows {observed:?}, expected {expected:?}"
            )
            .into());
        }
        if page.window.candidate_count() != want_total {
            return Err(format!(
                "{query:?} under {order}: total {:?}, expected {want_total:?}",
                page.window.candidate_count()
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
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(root.path(), 8)?;
    let result = (|| -> TestResult {
        let client = process.connect()?;
        let pin = publish_fixture(&client)?;
        assert_predicate_cases(&client, &pin)
    })();
    let stopped = process.stop();
    let verified = binary.verify();
    result.and(stopped).and(verified)
}

fn assert_predicate_cases(client: &QuantaIndex, pin: &GenerationPin) -> TestResult {
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
        assert_orders_agree(client, pin, query, &expected)?;
    }

    // A raw string alone: a substring filter under recency (`prefix`
    // included), nothing to score under relevance.
    let recency = query(client, pin, "type:commit 'fix'", HistoryOrderV1::Recency)?;
    if rows(&recency) != shas(&[1, 2, 3, 4]) {
        return Err(format!(
            "a raw string is a substring: `prefix` matches, got {:?}",
            rows(&recency)
        )
        .into());
    }
    match query(client, pin, "type:commit 'fix'", HistoryOrderV1::Relevance) {
        Err(SdkError::Remote { code, .. })
            if code == quanta_index_core::HISTORY_TEXT_QUERY_UNSCORABLE_CODE => {}
        other => {
            return Err(
                format!("a raw string alone is unscorable under relevance, got {other:?}").into(),
            );
        }
    }

    // A literal with no token is refused typed under both orders, before
    // any row is read.
    for order in HistoryOrderV1::ALL {
        match query(client, pin, "type:commit \"👍\"", order) {
            Err(SdkError::Remote { code, .. })
                if code.as_wire_str() == "LEX_TEXT_QUERY_NO_TOKENS" => {}
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
