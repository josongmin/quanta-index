//! E2E-00 — live DSL matrix inventory.
//!
//! Self-tests for the harness defined in `common/e2e_harness.rs`. Each row
//! ingests through the real publish path, seals a generation, optionally
//! reopens, then asserts against the live IPC response. No in-memory
//! shortcut is permitted: a green here means storage + index + dispatcher
//! all worked together for one matrix cell. Future E2E-01..07 rows live in
//! their own files but use this harness.

#![forbid(unsafe_code)]

#[path = "common/e2e_corpus.rs"]
mod e2e_corpus;
#[path = "common/e2e_harness.rs"]
mod e2e_harness;

use anyhow::Result as AnyResult;
use quanta_index_contract::TextQuerySyntax;

use crate::e2e_corpus::{SMOKE_CORPUS, ingest_all};
use crate::e2e_harness::E2eRuntime;

#[test]
fn harness_smoke_write_seal_reopen_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_all(&mut rt, SMOKE_CORPUS)?;
    let _sealed = rt.seal()?;
    let mut rt = rt.reopen()?;
    // After reopen the harness pin still points at the just-sealed
    // generation, so `query_text` pins to the durable manifest the
    // restarted runtime replays from disk.
    let result = rt.query_text(TextQuerySyntax::Native, "smoke_needle_rust", 10);
    if let Some(error) = result.typed_error {
        return Err(anyhow::anyhow!(
            "expected no typed error, got code={} message={}",
            error.code,
            error.message
        ));
    }
    if result.candidates.is_empty() {
        return Err(anyhow::anyhow!(
            "expected non-empty candidates after write -> seal -> reopen -> query"
        ));
    }
    let ingested_paths: Vec<&str> = SMOKE_CORPUS.iter().map(|row| row.path).collect();
    let matched = result.candidates.iter().any(|cand| {
        ingested_paths
            .iter()
            .any(|p| cand.repo_relative_path.as_str() == *p)
    });
    if !matched {
        let candidate_paths: Vec<&str> = result
            .candidates
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str())
            .collect();
        return Err(anyhow::anyhow!(
            "expected at least one candidate path to match an ingested row; got candidate paths={}",
            candidate_paths.join(", ")
        ));
    }
    Ok(())
}

#[test]
fn harness_smoke_invalid_query_returns_typed_error() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    ingest_all(&mut rt, SMOKE_CORPUS)?;
    let _sealed = rt.seal()?;
    // No generation pin AND no selector → dispatcher should reject with
    // `INVALID_REQUEST` ("text: generation pin required"). This proves the
    // typed-error surface of the harness end-to-end.
    let result = rt.query_text_with_pin(TextQuerySyntax::Native, "smoke_needle_rust", 10, None);
    let Some(typed) = result.typed_error else {
        return Err(anyhow::anyhow!(
            "expected a typed error for missing generation pin"
        ));
    };
    if !typed.code.starts_with("INVALID") {
        return Err(anyhow::anyhow!(
            "expected INVALID_* typed error code, got code={} message={}",
            typed.code,
            typed.message
        ));
    }
    Ok(())
}
