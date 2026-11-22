//! QI-BB-005 — `count` is a window contract, not a page widener.
//!
//! A pinned old-Q process serves the original five-file corpus through the
//! public SDK. Exact row, count, and continuation oracles remain independent
//! of the implementation being retired.

#![forbid(unsafe_code)]

use std::{error::Error, time::Duration};

use quanta_index_contract::{
    CandidateCountV1, GenerationPin, ManifestGeneration, QueryResultWindowV2, RepoId, RevisionId,
};
use quanta_index_sdk::QuantaIndex;
use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, publish_legacy_source};

use crate::fail_closed_wait::{RealTicker, wait_for};

type TestResult = Result<(), Box<dyn Error>>;

const FILES: u64 = 5;

fn with_published_reference(
    test: impl FnOnce(&QuantaIndex, &GenerationPin) -> TestResult,
) -> TestResult {
    let corpus = (0..FILES)
        .map(|index| {
            (
                format!("src/item_{index}.rs"),
                format!("fn item_{index}() {{ needle }}"),
                index + 1,
            )
        })
        .collect::<Vec<_>>();
    with_published_reference_files(&corpus, test)
}

fn with_published_reference_files(
    corpus: &[(String, String, u64)],
    test: impl FnOnce(&QuantaIndex, &GenerationPin) -> TestResult,
) -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let repo = RepoId::new("repo-e2e")?;
        let revision = RevisionId::new("rev-e2e")?;
        let files = corpus
            .iter()
            .map(|(path, body, id)| (path.as_str(), body.as_str(), *id))
            .collect::<Vec<_>>();
        let (_head, _receipt) =
            publish_legacy_source(&client, &repo, &revision, 1, None, &files, None)?;
        let pin = GenerationPin::new(repo, revision, ManifestGeneration::new(1));
        let _ready = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "pinned old-Q count query backend ready",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| true,
        )?;
        test(&client, &pin)
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}

struct Observed {
    rows: usize,
    paths: Vec<String>,
    window: QueryResultWindowV2,
}

fn text_query(
    client: &QuantaIndex,
    pin: &GenerationPin,
    query_text: &str,
    top_k: u32,
) -> Result<Observed, Box<dyn Error>> {
    let response = client
        .lexical()
        .query()
        .native(query_text)
        .pinned(pin.clone())
        .top_k(top_k)
        .execute()?;
    Ok(Observed {
        rows: response.results.len(),
        paths: response
            .results
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str().to_owned())
            .collect(),
        window: response.window,
    })
}

fn expect(
    what: &str,
    observed: &Observed,
    rows: usize,
    count: CandidateCountV1,
    has_more: bool,
) -> Result<(), Box<dyn Error>> {
    if observed.rows != rows
        || observed.window.candidate_count() != count
        || observed.window.has_more() != Some(has_more)
    {
        return Err(format!(
            "{what}: expected rows={rows} count={count:?} has_more={has_more}, observed rows={} count={:?} has_more={:?}",
            observed.rows,
            observed.window.candidate_count(),
            observed.window.has_more()
        )
        .into());
    }
    Ok(())
}

#[test]
fn count_all_keeps_the_page_and_reports_the_exact_total() -> TestResult {
    with_published_reference(|client, pin| {
        let observed = text_query(client, pin, "count:all needle", 2)?;
        expect(
            "count:all top_k=2",
            &observed,
            2,
            CandidateCountV1::Exact(FILES),
            true,
        )?;
        let whole = text_query(client, pin, "count:all needle", 10)?;
        expect(
            "count:all top_k=10",
            &whole,
            5,
            CandidateCountV1::Exact(FILES),
            false,
        )
    })
}

#[test]
fn a_plain_page_reports_only_what_the_probe_proved() -> TestResult {
    with_published_reference(|client, pin| {
        let observed = text_query(client, pin, "needle", 2)?;
        // Two rows plus one probe row observed: at least three, more exist.
        expect(
            "plain top_k=2",
            &observed,
            2,
            CandidateCountV1::AtLeast(3),
            true,
        )
    })
}

#[test]
fn a_bounded_count_caps_the_page_and_still_reports_the_exact_total() -> TestResult {
    with_published_reference(|client, pin| {
        let observed = text_query(client, pin, "count:3 needle", 10)?;
        expect(
            "count:3 top_k=10",
            &observed,
            3,
            CandidateCountV1::Exact(FILES),
            true,
        )
    })
}

#[test]
fn a_projection_reports_its_collapsed_universe_exactly() -> TestResult {
    with_published_reference(|client, pin| {
        // Five files on five distinct paths collapse to five path rows.
        let observed = text_query(client, pin, "select:path needle", 2)?;
        expect(
            "select:path top_k=2",
            &observed,
            2,
            CandidateCountV1::Exact(FILES),
            true,
        )
    })
}

#[test]
fn select_file_collapses_duplicate_chunks_before_top_k_and_reports_exact_total() -> TestResult {
    let mut corpus = (0..15_u64)
        .map(|index| {
            (
                "src/frequent.rs".to_owned(),
                "needle needle needle needle needle".to_owned(),
                index + 1,
            )
        })
        .collect::<Vec<_>>();
    corpus.extend((0..9_u64).map(|index| {
        (
            format!("src/other_{index:02}.rs"),
            "needle unrelated words here".to_owned(),
            index + 16,
        )
    }));
    with_published_reference_files(&corpus, |client, pin| {
        let chunks = text_query(client, pin, "needle", 2)?;
        if chunks.paths != ["src/frequent.rs", "src/frequent.rs"] {
            return Err(format!(
                "ordinary top-k did not expose the before-collapse duplicate prefix: {:?}",
                chunks.paths
            )
            .into());
        }

        let expected = std::iter::once("src/frequent.rs".to_owned())
            .chain((0..9_u64).map(|index| format!("src/other_{index:02}.rs")))
            .collect::<std::collections::BTreeSet<_>>();
        for query in ["select:file needle", "select:path needle"] {
            let page = text_query(client, pin, query, 2)?;
            expect(query, &page, 2, CandidateCountV1::Exact(10), true)?;
            if page.paths.len() != 2 || page.paths[0] == page.paths[1] {
                return Err(
                    format!("{query} page did not group before top-k: {:?}", page.paths).into(),
                );
            }

            let full = text_query(client, pin, query, 10)?;
            expect(query, &full, 10, CandidateCountV1::Exact(10), false)?;
            let actual = full
                .paths
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>();
            if actual != expected {
                return Err(format!(
                    "{query} full page lost file identities: expected {expected:?}, got {actual:?}"
                )
                .into());
            }
        }
        Ok(())
    })
}

#[test]
fn type_path_collapses_multiple_chunks_before_top_k_and_reports_exact_total() -> TestResult {
    const TERM: &str = "type_path_projection_needle";
    let frequent = [TERM; 5].join(" ");
    let corpus = [
        ("src/lib.rs".to_owned(), frequent.clone(), 1),
        ("src/lib.rs".to_owned(), frequent, 2),
        (
            "src/main.rs".to_owned(),
            format!("{TERM} other words here"),
            3,
        ),
    ];
    with_published_reference_files(&corpus, |client, pin| {
        let raw = text_query(client, pin, TERM, 2)?;
        if raw.paths != ["src/lib.rs", "src/lib.rs"] {
            return Err(format!(
                "expected the leading raw hits to share a path, got {:?}",
                raw.paths
            )
            .into());
        }
        let query = format!("count:all type:path {TERM}");
        let page = text_query(client, pin, &query, 1)?;
        expect(&query, &page, 1, CandidateCountV1::Exact(2), true)?;

        let full = text_query(client, pin, &query, 2)?;
        expect(&query, &full, 2, CandidateCountV1::Exact(2), false)?;
        let expected = ["src/lib.rs", "src/main.rs"]
            .into_iter()
            .map(str::to_owned)
            .collect::<std::collections::BTreeSet<_>>();
        let actual = full
            .paths
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        if actual != expected {
            return Err(format!(
                "{query} must preserve the two unique paths: expected {expected:?}, got {actual:?}"
            )
            .into());
        }
        Ok(())
    })
}
