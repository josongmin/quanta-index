//! E2E-00 — live DSL matrix inventory.
//!
//! Self-tests for the harness defined in the `quanta-index-searchd-harness` crate. Each row
//! ingests through the real publish path, seals a generation, optionally
//! reopens, then asserts against the live IPC response. No in-memory
//! shortcut is permitted: a green here means storage + index + dispatcher
//! all worked together for one matrix cell. Future E2E-01..07 rows live in
//! their own files but use this harness.

#![forbid(unsafe_code)]

use anyhow::Result as AnyResult;
use quanta_index_contract::TextQuerySyntax;

use crate::e2e_corpus::{SMOKE_CORPUS, SMOKE_NEEDLE_RUST, SMOKE_NEEDLE_RUST_IDENTITY, ingest_all};
use crate::e2e_harness::E2eRuntime;

fn verify_write_seal_reopen_query(rt: &mut E2eRuntime) -> AnyResult<()> {
    // After reopen the harness pin still points at the just-sealed
    // generation, so `query_text` pins to the durable manifest the
    // restarted runtime replays from disk.
    let result = rt.query_text(TextQuerySyntax::Native, SMOKE_NEEDLE_RUST, 10);
    if let Some(error) = result.typed_error {
        return Err(anyhow::anyhow!(
            "expected no typed error, got code={} message={}",
            error.code,
            error.message
        ));
    }
    // Exact identity oracle (TOPT-05 / WA-2): the returned `(repo, path)`
    // set must equal the fixture contract exactly. A different in-corpus
    // row (e.g. the python needle) or a dropped needle row both fail;
    // "at least one row matched something ingested" no longer passes.
    let actual: Vec<(&str, &str)> = result
        .candidates
        .iter()
        .map(|candidate| {
            (
                candidate.repo_id.as_str(),
                candidate.repo_relative_path.as_str(),
            )
        })
        .collect();
    verify_identity_set(&actual, SMOKE_NEEDLE_RUST_IDENTITY).map_err(|error| {
        anyhow::anyhow!("write -> seal -> reopen -> query identity mismatch: {error:#}")
    })?;
    Ok(())
}

/// The exact-set comparison behind the smoke oracle, factored pure so the
/// mutation controls below prove it rejects precisely what it names.
fn verify_identity_set(actual: &[(&str, &str)], expected: &[(&str, &str)]) -> AnyResult<()> {
    // Compare the full returned row inventory: deduplicating here would
    // turn a duplicated candidate into a false-green exact-set proof.
    let mut actual = actual.to_vec();
    actual.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    if actual != expected {
        return Err(anyhow::anyhow!(
            "expected exactly the fixture-contract candidate identity set {expected:?}; got {actual:?}"
        ));
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

fn verify_invalid_query_returns_typed_error(rt: &mut E2eRuntime) -> AnyResult<()> {
    // No generation pin AND no selector → dispatcher should reject with
    // `INVALID_REQUEST` ("text: generation pin required"). This proves the
    // typed-error surface of the harness end-to-end.
    let result = rt.query_text_with_pin(TextQuerySyntax::Native, SMOKE_NEEDLE_RUST, 10, None);
    let Some(typed) = result.typed_error else {
        return Err(anyhow::anyhow!(
            "expected a typed error for missing generation pin"
        ));
    };
    if typed.code.as_str() != "INVALID_REQUEST" {
        return Err(anyhow::anyhow!(
            "expected INVALID_REQUEST typed error code, got code={} message={}",
            typed.code,
            typed.message
        ));
    }
    Ok(())
}

#[test]
fn harness_smoke_scenarios_share_one_reopened_fixture() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_all(&mut rt, SMOKE_CORPUS)?;
    let _sealed = rt.seal()?;
    let mut rt = rt.reopen();

    verify_write_seal_reopen_query(&mut rt).map_err(|error| anyhow::anyhow!("query: {error:#}"))?;
    verify_invalid_query_returns_typed_error(&mut rt)
        .map_err(|error| anyhow::anyhow!("typed_error: {error:#}"))?;
    Ok(())
}
