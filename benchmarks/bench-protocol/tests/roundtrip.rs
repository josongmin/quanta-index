//! Round-trip and immutable-run lifecycle tests for `BenchmarkEvidenceV1`.

#![expect(
    clippy::panic_in_result_fn,
    reason = "contract tests use Result-returning setup with assertion-style validation"
)]
use std::error::Error;

use quanta_index_bench_protocol::sample::{sample_evidence, sample_raw_bytes, sample_sealed};
use quanta_index_bench_protocol::{BenchmarkEvidenceV1, ProtocolError, RunStore};

#[test]
fn proof_counts_are_not_relevance_or_performance() -> Result<(), Box<dyn Error>> {
    use quanta_index_bench_protocol::{Payload, ProofPayload};

    let mut record = sample_evidence()?;
    record.verdict.scope = "contract".to_owned();
    record.payload = Payload::Proof(ProofPayload {
        rail: "retrieval-contract".to_owned(),
        selected: 8,
        executed: 8,
        passed: 8,
        failed: 0,
        source_digest: record.source.closure_digest.clone(),
        execution_context_digest: record.source.closure_digest.clone(),
    });
    let sealed = record.clone().seal()?;
    assert_eq!(
        BenchmarkEvidenceV1::open(&sealed.to_canonical_json()?)?,
        sealed
    );
    record.verdict.scope = "quality".to_owned();
    assert!(record.validate().is_err());
    record.verdict.scope = "contract".to_owned();
    if let Payload::Proof(proof) = &mut record.payload {
        proof.executed = 7;
    }
    assert!(record.validate().is_err());
    if let Payload::Proof(proof) = &mut record.payload {
        proof.executed = 8;
        proof.passed = 7;
        proof.failed = 1;
    }
    assert!(record.validate().is_err());
    record.verdict.status = "fail".to_owned();
    record.verdict.reason = Some("one failed test".to_owned());
    assert!(record.validate().is_ok());
    if let Payload::Proof(proof) = &mut record.payload {
        proof.passed = u64::MAX;
        proof.failed = 1;
    }
    assert!(record.validate().is_err());
    Ok(())
}

const RUN_ID: &str = "run-20260926T120000Z-a1b2c3d4";

#[test]
fn sample_round_trips_through_canonical_json() -> Result<(), Box<dyn Error>> {
    let sealed = sample_sealed()?;
    let canonical = sealed.to_canonical_json()?;
    let reopened = BenchmarkEvidenceV1::open(&canonical)?;
    assert_eq!(reopened, sealed);
    assert_eq!(reopened.to_canonical_json()?, canonical);
    assert!(reopened.digest.is_some());
    Ok(())
}

#[test]
fn sealing_is_order_insensitive_but_digest_sensitive() -> Result<(), Box<dyn Error>> {
    let sealed = sample_sealed()?;
    let mut mutated = sample_evidence()?;
    mutated.command.wall_ms = 12346;
    let mutated = mutated.seal()?;
    assert_ne!(mutated.digest, sealed.digest);
    Ok(())
}

#[test]
fn run_store_promotes_and_replays() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let store = RunStore::new(root.path().canonicalize()?);
    let sealed = sample_sealed()?;
    let staged = store.stage(RUN_ID)?;
    let _reference = staged.write_raw("raw/warm-matrix.json", &sample_raw_bytes())?;
    staged.write_evidence(&sealed)?;
    let promotion = store.promote(staged)?;
    assert_eq!(promotion.run_id, RUN_ID);
    assert_eq!(promotion.digest, sealed.digest.clone().unwrap_or_default());

    let loaded = store.load(RUN_ID)?;
    assert_eq!(loaded, sealed);

    let latest = store.read_latest()?;
    let pointer = latest.ok_or("latest pointer was not written")?;
    assert_eq!(pointer.run_id, RUN_ID);

    let baseline = store.admit_baseline("dsl-warm", RUN_ID, 10_000, "blocked-paired")?;
    assert_eq!(baseline.run_id, RUN_ID);

    // A baseline-referenced run is never collected.
    let removed = store.collect(&[])?;
    assert!(
        removed.is_empty(),
        "baseline run was collected: {removed:?}"
    );
    assert!(store.load(RUN_ID).is_ok());
    Ok(())
}

#[test]
fn run_store_refuses_a_repeated_run_id() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let store = RunStore::new(root.path());
    let sealed = sample_sealed()?;
    let staged = store.stage(RUN_ID)?;
    let _reference = staged.write_raw("raw/warm-matrix.json", &sample_raw_bytes())?;
    staged.write_evidence(&sealed)?;
    let _first = store.promote(staged)?;
    match store.stage(RUN_ID) {
        Err(ProtocolError::RunExists(found)) => assert_eq!(found, RUN_ID),
        other => return Err(format!("expected RunExists, got {other:?}").into()),
    }
    Ok(())
}

#[test]
fn crash_before_promotion_leaves_no_admissible_run() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let store = RunStore::new(root.path());
    let sealed = sample_sealed()?;
    let staged = store.stage(RUN_ID)?;
    let _reference = staged.write_raw("raw/warm-matrix.json", &sample_raw_bytes())?;
    staged.write_evidence(&sealed)?;
    // Simulate a crash: the staging directory is abandoned, never promoted.
    let staging = staged.path().to_path_buf();
    assert!(staging.is_dir());
    match store.load(RUN_ID) {
        Err(ProtocolError::MissingRaw(_)) => {}
        other => return Err(format!("expected MissingRaw, got {other:?}").into()),
    }
    assert!(store.read_latest()?.is_none());
    Ok(())
}

#[test]
fn abort_removes_the_staged_run() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let store = RunStore::new(root.path());
    let staged = store.stage(RUN_ID)?;
    let staging = staged.path().to_path_buf();
    staged.abort()?;
    assert!(!staging.exists());
    Ok(())
}

#[test]
fn collect_removes_only_unreferenced_runs() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let store = RunStore::new(root.path().canonicalize()?);
    let sealed = sample_sealed()?;
    let staged = store.stage(RUN_ID)?;
    let _reference = staged.write_raw("raw/warm-matrix.json", &sample_raw_bytes())?;
    staged.write_evidence(&sealed)?;
    let _kept = store.promote(staged)?;
    let _admitted = store.admit_baseline("dsl-warm", RUN_ID, 10_000, "blocked-paired")?;

    let other_id = "run-20260926T130000Z-deadbeef";
    let mut other_evidence = sample_evidence()?;
    other_evidence.run_id = other_id.to_owned();
    let other_evidence = other_evidence.seal()?;
    let staged = store.stage(other_id)?;
    let _reference = staged.write_raw("raw/warm-matrix.json", &sample_raw_bytes())?;
    staged.write_evidence(&other_evidence)?;
    let _other = store.promote(staged)?;

    let removed = store.collect(&[])?;
    assert_eq!(removed, vec![other_id.to_owned()]);
    assert!(store.load(RUN_ID).is_ok());
    Ok(())
}
