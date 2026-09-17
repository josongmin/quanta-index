use quanta_index_contract::{ManifestGeneration, SearchPlaneTrackKind};

use crate::readiness::ledger::Ledger;
use crate::readiness::tests::support::{TestResult, generation, repo_id, revision_id};

#[test]
fn seals_are_monotonic_per_track() {
    let mut ledger = Ledger::default();
    ledger.lexical_seal(ManifestGeneration::new(7));
    ledger.lexical_seal(ManifestGeneration::new(3));
    ledger.semantic_seal(ManifestGeneration::new(2));
    ledger.semantic_seal(ManifestGeneration::new(5));

    assert_eq!(ledger.lexical_sealed(), Some(ManifestGeneration::new(7)));
    assert_eq!(ledger.semantic_sealed(), Some(ManifestGeneration::new(5)));
}

#[test]
fn semantic_generation_state_records_exact_digest_and_seal() -> TestResult {
    let mut ledger = Ledger::default();
    ledger.record_track_materialized(
        &repo_id(),
        &revision_id(),
        SearchPlaneTrackKind::Semantic,
        generation(),
        Some("digest-sem-17"),
    );
    ledger.record_track_seal_with_digest(
        &repo_id(),
        &revision_id(),
        SearchPlaneTrackKind::Semantic,
        generation(),
        "digest-sem-17",
    );
    let state = ledger
        .semantic_generation_state(&repo_id(), &revision_id(), generation())
        .ok_or("missing semantic generation state")?;
    if !state.materialized() {
        return Err("semantic generation state did not record materialized".into());
    }
    if !state.sealed() {
        return Err("semantic generation state did not record sealed".into());
    }
    if state.manifest_digest() != "digest-sem-17" {
        return Err("semantic generation state lost manifest digest".into());
    }
    Ok(())
}
