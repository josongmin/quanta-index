//! E2E-00 — live DSL matrix inventory.
//!
//! The fixed corpus is published and queried through the SDK against a
//! pinned Legacy reference, including a process restart before the read.

#![forbid(unsafe_code)]

use std::{error::Error, time::Duration};

use quanta_index_contract::{
    GenerationPin, QueryConstraintSetV1, RepoId, RevisionId, SearchPlaneErrorCodeV2,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_sdk::{QuantaIndex, SdkError};
use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, publish_legacy_source};

use crate::fail_closed_wait::{RealTicker, wait_for};

type TestResult = Result<(), Box<dyn Error>>;

const REPO: &str = "repo-e2e";
const SMOKE_CORPUS: &[(&str, &str, u64)] = &[
    ("docs/intro.md", "smoke documentation lives here", 3),
    ("src/lib.rs", "fn smoke_needle_rust() {}", 1),
    ("src/util.py", "def smoke_needle_python(): pass", 2),
];
const SMOKE_NEEDLE_RUST: &str = "smoke_needle_rust";
const SMOKE_NEEDLE_RUST_IDENTITY: &[(&str, &str)] = &[(REPO, "src/lib.rs")];

fn verify_write_seal_reopen_query(client: &QuantaIndex, pin: &GenerationPin) -> TestResult {
    let result = client
        .lexical()
        .query()
        .native(SMOKE_NEEDLE_RUST)
        .pinned(pin.clone())
        .top_k(10)
        .execute()?;
    // Exact identity oracle (TOPT-05 / WA-2): the returned `(repo, path)`
    // set must equal the fixture contract exactly. A different in-corpus
    // row (e.g. the python needle) or a dropped needle row both fail;
    // "at least one row matched something ingested" no longer passes.
    let actual: Vec<(&str, &str)> = result
        .results
        .iter()
        .map(|candidate| {
            (
                candidate.repo_id.as_str(),
                candidate.repo_relative_path.as_str(),
            )
        })
        .collect();
    verify_identity_set(&actual, SMOKE_NEEDLE_RUST_IDENTITY)
}

/// The exact-set comparison behind the smoke oracle, factored pure so the
/// mutation controls below prove it rejects precisely what it names.
fn verify_identity_set(actual: &[(&str, &str)], expected: &[(&str, &str)]) -> TestResult {
    // Compare the full returned row inventory: deduplicating here would
    // turn a duplicated candidate into a false-green exact-set proof.
    let mut actual = actual.to_vec();
    actual.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    if actual != expected {
        return Err(format!(
            "expected exactly the fixture-contract candidate identity set {expected:?}; got {actual:?}"
        )
        .into());
    }
    Ok(())
}

/// Mutation controls: the oracle accepts the contract set and rejects a
/// missing needle row, a wrong in-corpus row, an extra row, and an empty
/// answer — each the precise regression WA-2 names.
#[test]
fn identity_oracle_rejects_missing_wrong_extra_duplicate_and_empty_rows() {
    let expected = SMOKE_NEEDLE_RUST_IDENTITY;
    assert!(
        verify_identity_set(expected, expected).is_ok(),
        "the contract set itself must verify"
    );
    for (what, actual) in [
        ("missing needle row", [].as_slice()),
        (
            "wrong in-corpus row",
            [("repo-e2e", "src/util.py")].as_slice(),
        ),
        (
            "needle plus extra row",
            [("repo-e2e", "src/lib.rs"), ("repo-e2e", "src/util.py")].as_slice(),
        ),
        (
            "duplicate needle row",
            [("repo-e2e", "src/lib.rs"), ("repo-e2e", "src/lib.rs")].as_slice(),
        ),
        ("wrong repo", [("repo-other", "src/lib.rs")].as_slice()),
    ] {
        assert!(
            verify_identity_set(actual, expected).is_err(),
            "the oracle must reject a {what}"
        );
    }
}

fn verify_invalid_query_returns_typed_error(client: &QuantaIndex) -> TestResult {
    let result = client.lexical().query_request(TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: SMOKE_NEEDLE_RUST.to_owned(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 10,
        cursor: None,
    });
    match result {
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::InvalidRequest,
            ..
        }) => Ok(()),
        other => Err(format!(
            "expected a server INVALID_REQUEST for missing generation: {other:?}"
        )
        .into()),
    }
}

#[test]
fn smoke_needle_contract_matches_corpus_contents() -> TestResult {
    let matches = SMOKE_CORPUS
        .iter()
        .filter(|(_, body, _)| body.contains(SMOKE_NEEDLE_RUST))
        .map(|(path, _, _)| (REPO, *path))
        .collect::<Vec<_>>();
    verify_identity_set(&matches, SMOKE_NEEDLE_RUST_IDENTITY)
}

#[test]
fn harness_smoke_scenarios_share_one_reopened_fixture() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let repo = RepoId::new(REPO)?;
    let revision = RevisionId::new("rev-e2e")?;
    let publishing = binary.start(&state_root, 8)?;
    let published = (|| {
        let client = publishing.connect()?;
        publish_legacy_source(&client, &repo, &revision, 1, None, SMOKE_CORPUS, None)
    })();
    let stopped = publishing.stop();
    let (head, _receipt) = published?;
    stopped?;
    let pin = GenerationPin::new(repo, revision, head.generation.lexical.manifest_generation);

    let reopened = binary.start(&state_root, 8)?;
    let result = (|| -> TestResult {
        let client = reopened.connect()?;
        let _ready = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "pinned old-Q matrix backend ready after reopen",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| true,
        )?;
        verify_write_seal_reopen_query(&client, &pin)?;
        verify_invalid_query_returns_typed_error(&client)?;
        Ok(())
    })();
    let stopped = reopened.stop();
    result.and(stopped)?;
    binary.verify()
}
