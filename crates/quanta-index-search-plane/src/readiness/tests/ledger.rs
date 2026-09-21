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

/// QI-BB-020 W2: an auxiliary mutation the authority refuses leaves the
/// ledger exactly as it was.
///
/// A generation that did not exist does not come into existence, and one
/// that did keeps its epoch and content.
#[test]
fn a_refused_auxiliary_mutation_leaves_the_ledger_as_it_was() -> TestResult {
    use quanta_index_contract::channel::LexicalChannelOp;
    use quanta_index_contract::{AuxEpochV1, HistoryIngestBatch, UpsertRef};
    use quanta_index_core::CoreError;

    let now = std::time::Instant::now();
    let mut ledger = Ledger::default();
    let dangling_ref = LexicalChannelOp::UpsertRef(UpsertRef {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: generation(),
        name: "refs/heads/dangling".to_string().into_boxed_str(),
        sha: [9_u8; 20],
    });
    // On a generation that does not exist yet: nothing is created.
    match ledger.apply_lexical_authority_op(&dangling_ref, now) {
        Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::HistoryRefNotFound,
                ),
            ..
        }) => {}
        other => {
            return Err(
                format!("a ref to an unknown commit is refused typed, got {other:?}").into(),
            );
        }
    }
    if ledger
        .history_state(&repo_id(), &revision_id(), generation())
        .is_some()
    {
        return Err("a refused mutation must not create the generation".into());
    }
    // On a generation that exists: epoch and content are untouched.
    ledger.apply_history_batch(
        &HistoryIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            manifest_digest: None,
            batch_digest: "seed".to_string(),
            commits: Vec::new(),
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: Vec::new(),
        },
        now,
    )?;
    let before = ledger
        .history_read_at(&repo_id(), &revision_id(), generation(), None, now)?
        .ok_or("the seeded generation exists")?;
    if before.epoch != AuxEpochV1::new(1) {
        return Err(format!("the seed is epoch 1, read {:?}", before.epoch).into());
    }
    if ledger
        .apply_lexical_authority_op(&dangling_ref, now)
        .is_ok()
    {
        return Err("the dangling ref is still refused".into());
    }
    let after = ledger
        .history_read_at(&repo_id(), &revision_id(), generation(), None, now)?
        .ok_or("the seeded generation still exists")?;
    if after.epoch != before.epoch || after.retained != before.retained {
        return Err(format!(
            "a refused mutation must not advance the epoch: {:?} -> {:?}",
            before.epoch, after.epoch
        )
        .into());
    }
    if after.state.refs_materialized() {
        return Err("a refused mutation must not leave its partial effect behind".into());
    }
    Ok(())
}
