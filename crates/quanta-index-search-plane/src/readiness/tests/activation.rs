#![expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning durability and CAS tests use assertions as test-failure reporting"
)]

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use quanta_index_contract::{
    GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneTrackKind,
};
use quanta_index_core::CoreError;
use tempfile::tempdir;

use crate::readiness::activation_catalog::ActivationCatalog;
use crate::readiness::auxiliary_store::AuxiliaryAuthorityStore;
use crate::readiness::search_corpus_generation::{
    PreparedSearchCorpusGenerationV1, SearchCorpusGenerationV1,
};
use crate::readiness::tests::support::{
    AlwaysFailParentSync, FailAtParentSync, TestResult, ToggleParentSyncFailure, active_head,
    assert_active_composite_v1, corpus_generation, corpus_identity, corpus_snapshot,
    search_corpus_retention,
};
use crate::search_corpus_lifecycle::SearchCorpusPairMutationCoordinator;

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts persistence/resolution via assert_eq! macros"
)]
fn activation_catalog_persists_composite_root_and_rolls_back_both_tracks_v1() -> TestResult {
    let dir = tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let active = corpus_generation(17, "digest-17")?;
    let prepared = PreparedSearchCorpusGenerationV1::new(active, None)?;
    let activation = catalog.activate_prepared_search_corpus_generation_v1(&prepared)?;
    assert_eq!(
        activation.active.generation.lexical.manifest_generation,
        ManifestGeneration::new(17)
    );
    let repo = RepoId::new("repo-corpus")?;
    let revision = RevisionId::new("rev-corpus")?;
    let (_, first_token) = catalog
        .active_search_corpus_with_token_v1(&repo, &revision)?
        .expect("activated head");
    assert_eq!(first_token.activation_sequence().get(), 1);
    assert_ne!(first_token.root_incarnation(), [0; 16]);

    let pin = catalog.resolve(
        &RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev-corpus").expect("static fixture ID satisfies canonical policy"),
        SearchPlaneTrackKind::Lexical,
    )?;
    assert_eq!(pin.manifest_generation, ManifestGeneration::new(17));

    let reopened = ActivationCatalog::open(dir.path())?;
    let (_, reopened_token) = reopened
        .active_search_corpus_with_token_v1(&repo, &revision)?
        .expect("reopened head");
    assert_eq!(reopened_token, first_token);
    let reopened_pin = reopened.resolve(
        &RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev-corpus").expect("static fixture ID satisfies canonical policy"),
        SearchPlaneTrackKind::Lexical,
    )?;
    assert_eq!(
        reopened_pin.manifest_generation,
        ManifestGeneration::new(17)
    );

    let semantic_before = reopened.resolve_record(
        &RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev-corpus").expect("static fixture ID satisfies canonical policy"),
        SearchPlaneTrackKind::Semantic,
    )?;
    assert_eq!(
        semantic_before.manifest_generation,
        ManifestGeneration::new(17)
    );

    let rollback = catalog.rollback(&SearchPlaneRollbackSearchCorpusGenerationCasRequest {
        expected_active: active_head(&catalog, prepared.candidate())?,
        target: corpus_generation(16, "digest-16")?.to_contract_v1(),
    })?;
    assert_eq!(
        rollback
            .previous_sealed_active
            .generation
            .lexical
            .manifest_generation,
        ManifestGeneration::new(17)
    );
    assert_eq!(
        rollback.active.generation.lexical.manifest_generation,
        ManifestGeneration::new(16)
    );
    let reopened = ActivationCatalog::open(dir.path())?;
    let (_, after_rollback_token) = reopened
        .active_search_corpus_with_token_v1(&repo, &revision)?
        .expect("reopened rollback head");
    assert_eq!(
        after_rollback_token.root_incarnation(),
        first_token.root_incarnation()
    );
    assert_eq!(after_rollback_token.activation_sequence().get(), 2);
    let lexical_after = reopened.resolve_record(
        &RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev-corpus").expect("static fixture ID satisfies canonical policy"),
        SearchPlaneTrackKind::Lexical,
    )?;
    let semantic_after = reopened.resolve_record(
        &RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev-corpus").expect("static fixture ID satisfies canonical policy"),
        SearchPlaneTrackKind::Semantic,
    )?;
    assert_eq!(
        lexical_after.manifest_generation,
        ManifestGeneration::new(16)
    );
    assert_eq!(
        semantic_after.manifest_generation,
        ManifestGeneration::new(16)
    );
    assert_eq!(lexical_after.manifest_digest, "digest-16");
    assert_eq!(semantic_after.manifest_digest, "digest-16");
    Ok(())
}

#[test]
fn controlled_restore_rotates_activation_incarnation_without_rewriting_generation_v1() -> TestResult
{
    let dir = tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let generation = corpus_generation(17, "digest-17")?;
    let prepared = PreparedSearchCorpusGenerationV1::new(generation, None)?;
    drop(catalog.activate_prepared_search_corpus_generation_v1(&prepared)?);
    let repo = RepoId::new("repo-corpus")?;
    let revision = RevisionId::new("rev-corpus")?;
    let before = catalog
        .active_search_corpus_with_token_v1(&repo, &revision)?
        .expect("active head before restore");
    drop(catalog);

    ActivationCatalog::rotate_root_incarnation_for_restore_v1(dir.path())?;
    let reopened = ActivationCatalog::open(dir.path())?;
    let after = reopened
        .active_search_corpus_with_token_v1(&repo, &revision)?
        .expect("active head after restore");
    assert_eq!(after.0, before.0);
    assert_eq!(
        after.1.activation_sequence(),
        before.1.activation_sequence()
    );
    assert_ne!(after.1.root_incarnation(), before.1.root_incarnation());
    Ok(())
}

#[test]
fn active_root_reopen_rejects_missing_incarnation_and_zero_sequence_v1() -> TestResult {
    let dir = tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let generation = corpus_generation(17, "digest-17")?;
    drop(catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(generation.clone(), None)?,
    )?);
    drop(catalog);

    let incarnation_path = dir.path().join(".activation-root-incarnation-v1");
    let incarnation = std::fs::read(&incarnation_path)?;
    std::fs::remove_file(&incarnation_path)?;
    let missing = ActivationCatalog::open(dir.path()).expect_err("active root needs incarnation");
    assert!(missing.to_string().contains("lack root incarnation"));
    std::fs::write(&incarnation_path, incarnation)?;

    let root_path = dir.path().join(
        crate::readiness::activation_catalog::search_corpus_root_file_name(
            generation.repo_id(),
            generation.revision_id(),
        ),
    );
    let mut root: serde_json::Value = serde_json::from_slice(&std::fs::read(&root_path)?)?;
    drop(
        root.as_object_mut()
            .ok_or("activation root must be a JSON object")?
            .insert("activation_sequence".to_string(), serde_json::json!(0)),
    );
    std::fs::write(&root_path, serde_json::to_vec(&root)?)?;
    let zero = ActivationCatalog::open(dir.path()).expect_err("zero sequence must be rejected");
    assert!(
        zero.to_string()
            .contains("activation_sequence must be positive")
    );
    Ok(())
}

#[test]
fn prepared_search_corpus_generation_rejects_single_track_and_mixed_identity() -> TestResult {
    let single_track = SearchCorpusGenerationV1::new(
        corpus_snapshot(SearchPlaneTrackKind::Lexical, 17, "digest-17"),
        corpus_snapshot(SearchPlaneTrackKind::Lexical, 17, "digest-17"),
        crate::content_roots_test_support::roots_for_generation(17),
    );
    let Err(CoreError::InvalidContract(single_track_message)) = single_track else {
        return Err("lexical-only corpus generation unexpectedly constructed".into());
    };
    assert_eq!(
        single_track_message,
        "search-corpus generation: SEMANTIC_TRACK_REQUIRED"
    );

    let mixed_identity = SearchCorpusGenerationV1::new(
        corpus_snapshot(SearchPlaneTrackKind::Lexical, 17, "digest-17"),
        corpus_snapshot(SearchPlaneTrackKind::Semantic, 18, "digest-18"),
        crate::content_roots_test_support::roots_for_generation(17),
    );
    let Err(CoreError::InvalidContract(mixed_identity_message)) = mixed_identity else {
        return Err("mixed corpus generation unexpectedly constructed".into());
    };
    assert_eq!(
        mixed_identity_message,
        "search-corpus generation: GENERATION_MISMATCH"
    );

    let foreign_expected = SearchCorpusGenerationV1::new(
        GenerationSnapshot {
            repo_id: RepoId::new("repo-other")
                .expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-corpus")
                .expect("static fixture ID satisfies canonical policy"),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(17),
            manifest_digest: "digest-17".to_string(),
        },
        GenerationSnapshot {
            repo_id: RepoId::new("repo-other")
                .expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-corpus")
                .expect("static fixture ID satisfies canonical policy"),
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: ManifestGeneration::new(17),
            manifest_digest: "digest-17".to_string(),
        },
        crate::content_roots_test_support::roots_for_generation(17),
    )?;
    let mismatched_expected = PreparedSearchCorpusGenerationV1::new(
        corpus_generation(17, "digest-17")?,
        Some(quanta_index_contract::SearchCorpusActiveHeadV1 {
            generation: foreign_expected.to_contract_v1(),
            activation_token: quanta_index_contract::SearchCorpusActivationTokenV1::new(
                [7; 16],
                NonZeroU64::new(1).expect("fixture sequence is positive"),
            )?,
        }),
    );
    let Err(CoreError::InvalidContract(expected_message)) = mismatched_expected else {
        return Err("foreign expected active unexpectedly constructed".into());
    };
    assert!(expected_message.contains("candidate repo and revision"));
    Ok(())
}

#[test]
fn prepared_search_corpus_activation_is_durable_before_reopen_and_rejects_stale_head() -> TestResult
{
    let dir = tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let first = corpus_generation(17, "digest-17")?;
    let first_prepared = PreparedSearchCorpusGenerationV1::new(first.clone(), None)?;
    let first_receipt = catalog.activate_prepared_search_corpus_generation_v1(&first_prepared)?;
    assert_eq!(first_receipt.active.generation, first.to_contract_v1());
    assert_eq!(first_receipt.previous_active, None);
    let first_head = first_receipt.active;

    let root = dir.path().join(
        crate::readiness::activation_catalog::search_corpus_root_file_name(
            first.repo_id(),
            first.revision_id(),
        ),
    );
    assert!(
        root.is_file(),
        "composite root must exist before memory receipt"
    );
    let directory_entries = std::fs::read_dir(dir.path())?.collect::<Result<Vec<_>, _>>()?;
    assert!(
        directory_entries
            .iter()
            .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp-")),
        "durable activation must not leave temporary roots behind"
    );

    let second = corpus_generation(18, "digest-18")?;
    let promoted_prepared =
        PreparedSearchCorpusGenerationV1::new(second.clone(), Some(first_head.clone()))?;
    let promoted = catalog.activate_prepared_search_corpus_generation_v1(&promoted_prepared)?;
    assert_eq!(promoted.active.generation, second.to_contract_v1());
    assert_eq!(promoted.previous_active, Some(first_head.clone()));

    let stale_prepared = PreparedSearchCorpusGenerationV1::new(
        corpus_generation(19, "digest-19")?,
        Some(first_head),
    )?;
    let stale = catalog.activate_prepared_search_corpus_generation_v1(&stale_prepared);
    let Err(CoreError::Typed { code, .. }) = stale else {
        return Err("stale composite expectation unexpectedly succeeded".into());
    };
    assert_eq!(
        code,
        quanta_index_contract::SearchPlaneErrorCodeV2::CompositeActivationCasConflict
    );

    let reopened = ActivationCatalog::open(dir.path())?;
    let lexical = reopened.resolve_record(
        &RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev-corpus").expect("static fixture ID satisfies canonical policy"),
        SearchPlaneTrackKind::Lexical,
    )?;
    let semantic = reopened.resolve_record(
        &RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev-corpus").expect("static fixture ID satisfies canonical policy"),
        SearchPlaneTrackKind::Semantic,
    )?;
    assert_eq!(lexical.manifest_generation, ManifestGeneration::new(18));
    assert_eq!(lexical.manifest_generation, semantic.manifest_generation);
    assert_eq!(lexical.manifest_digest, semantic.manifest_digest);
    Ok(())
}

#[test]
fn reactivated_generation_rejects_stale_activation_and_rollback_cas_tokens() -> TestResult {
    let dir = tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let a = corpus_generation(17, "digest-17")?;
    let b = corpus_generation(18, "digest-18")?;
    let first = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(a.clone(), None)?,
    )?;
    let stale_a = first.active;
    let first_inventory = catalog.active_inventory_v1()?;
    let second = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(b, Some(stale_a.clone()))?,
    )?;
    let third = catalog.rollback(&SearchPlaneRollbackSearchCorpusGenerationCasRequest {
        expected_active: second.active,
        target: a.to_contract_v1(),
    })?;
    assert_eq!(third.active.generation, stale_a.generation);
    assert_ne!(third.active.activation_token, stale_a.activation_token);
    let third_inventory = catalog.active_inventory_v1()?;
    assert_eq!(first_inventory.0.len(), 1);
    assert_eq!(third_inventory.0.len(), 1);
    assert_eq!(first_inventory.0[0].0, third_inventory.0[0].0);
    assert_ne!(first_inventory.0[0].1, third_inventory.0[0].1);

    let activation = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(
            corpus_generation(19, "digest-19")?,
            Some(stale_a.clone()),
        )?,
    );
    assert!(matches!(
        activation,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::CompositeActivationCasConflict,
            ..
        })
    ));

    let rollback = catalog.rollback(&SearchPlaneRollbackSearchCorpusGenerationCasRequest {
        expected_active: stale_a,
        target: corpus_generation(16, "digest-16")?.to_contract_v1(),
    });
    assert!(matches!(
        rollback,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::RollbackCasConflict,
            ..
        })
    ));
    assert_active_composite_v1(&catalog, &a)
}

#[test]
fn activation_catalog_concurrent_cas_promotions_select_one_composite_winner() -> TestResult {
    let dir = tempdir()?;
    let catalog = Arc::new(ActivationCatalog::open(dir.path())?);
    let active = corpus_generation(17, "digest-17")?;
    let initial_activation = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(active, None)?,
    )?;
    let initial_active = initial_activation.active;

    let first_candidate = corpus_generation(18, "digest-18")?;
    let second_candidate = corpus_generation(19, "digest-19")?;
    let first_prepared = PreparedSearchCorpusGenerationV1::new(
        first_candidate.clone(),
        Some(initial_active.clone()),
    )?;
    let second_prepared = PreparedSearchCorpusGenerationV1::new(
        second_candidate.clone(),
        Some(initial_active.clone()),
    )?;

    // Both contenders are fully prepared before either can enter the
    // catalog. The barrier releases their CAS calls together; winner
    // selection is intentionally unspecified, but split composite heads
    // and multiple successes are not.
    let start = Arc::new(Barrier::new(3));
    let first_catalog = Arc::clone(&catalog);
    let first_start = Arc::clone(&start);
    let first = thread::spawn(move || {
        let _barrier_receipt = first_start.wait();
        first_catalog.activate_prepared_search_corpus_generation_v1(&first_prepared)
    });
    let second_catalog = Arc::clone(&catalog);
    let second_start = Arc::clone(&start);
    let second = thread::spawn(move || {
        let _barrier_receipt = second_start.wait();
        second_catalog.activate_prepared_search_corpus_generation_v1(&second_prepared)
    });
    let _barrier_receipt = start.wait();

    let first_result = first
        .join()
        .map_err(|_join_error| "first activation contender panicked")?;
    let second_result = second
        .join()
        .map_err(|_join_error| "second activation contender panicked")?;

    let mut winner = None;
    for result in [first_result, second_result] {
        match result {
            Ok(receipt) => {
                assert_eq!(receipt.previous_active, Some(initial_active.clone()));
                if winner.replace(receipt.active).is_some() {
                    return Err("concurrent activation CAS admitted multiple winners".into());
                }
            }
            Err(CoreError::Typed { code, .. }) => {
                assert_eq!(
                    code,
                    quanta_index_contract::SearchPlaneErrorCodeV2::CompositeActivationCasConflict
                );
            }
            Err(error) => {
                return Err(
                    format!("concurrent activation returned unexpected error: {error}").into(),
                );
            }
        }
    }
    let winner = winner.ok_or("concurrent activation CAS produced no winner")?;
    assert!(
        winner.generation == first_candidate.to_contract_v1()
            || winner.generation == second_candidate.to_contract_v1()
    );

    let lexical = catalog.resolve_record(
        &winner.generation.lexical.repo_id,
        &winner.generation.lexical.revision_id,
        SearchPlaneTrackKind::Lexical,
    )?;
    let semantic = catalog.resolve_record(
        &winner.generation.lexical.repo_id,
        &winner.generation.lexical.revision_id,
        SearchPlaneTrackKind::Semantic,
    )?;
    assert_eq!(
        lexical.manifest_generation,
        winner.generation.lexical.manifest_generation
    );
    assert_eq!(
        semantic.manifest_generation,
        winner.generation.lexical.manifest_generation
    );
    assert_eq!(
        lexical.manifest_digest,
        winner.generation.lexical.manifest_digest
    );
    assert_eq!(
        semantic.manifest_digest,
        winner.generation.semantic.manifest_digest
    );
    Ok(())
}

#[test]
fn activation_catalog_fails_closed_after_durability_becomes_uncertain() -> TestResult {
    let dir = tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let first = corpus_generation(17, "digest-17")?;
    let activation = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(first.clone(), None)?,
    )?;
    assert_eq!(activation.active.generation, first.to_contract_v1());

    // This is the post-rename / parent-fsync-failure state. The next
    // process reconstructs from the durable root; this one must never
    // serve its potentially stale in-memory records in the meantime.
    catalog.mark_durability_uncertain_v1();

    let resolve = catalog.resolve_record(
        first.repo_id(),
        first.revision_id(),
        SearchPlaneTrackKind::Lexical,
    );
    let Err(CoreError::NotReady(resolve_message)) = resolve else {
        return Err("durability-uncertain catalog unexpectedly served a read".into());
    };
    assert!(resolve_message.contains("reopen the catalog"));

    let second = corpus_generation(18, "digest-18")?;
    let mutate = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(second, Some(activation.active))?,
    );
    let Err(CoreError::NotReady(mutate_message)) = mutate else {
        return Err("durability-uncertain catalog unexpectedly accepted a mutation".into());
    };
    assert!(mutate_message.contains("reopen the catalog"));
    Ok(())
}

#[test]
fn activation_catalog_fences_real_post_rename_parent_sync_failure() -> TestResult {
    let dir = tempdir()?;
    let catalog = ActivationCatalog::open_with_parent_sync(
        dir.path(),
        SearchCorpusPairMutationCoordinator::shared(),
        Arc::new(FailAtParentSync {
            calls: AtomicUsize::new(0),
            // Existing-root revalidation is call zero, staging creation is
            // call one, and root-incarnation persistence is call two. The
            // activation-file target-parent sync after rename is call three.
            fail_at: 3,
        }),
    )?;
    let candidate = corpus_generation(17, "digest-17")?;
    let result = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(candidate.clone(), None)?,
    );
    let Err(CoreError::Storage(message)) = result else {
        return Err("injected parent sync failure unexpectedly activated".into());
    };
    assert!(message.contains("injected parent sync failure"));

    let persisted = dir.path().join(
        crate::readiness::activation_catalog::search_corpus_root_file_name(
            candidate.repo_id(),
            candidate.revision_id(),
        ),
    );
    assert!(
        persisted.is_file(),
        "rename must precede injected sync failure"
    );
    let resolve = catalog.resolve_record(
        candidate.repo_id(),
        candidate.revision_id(),
        SearchPlaneTrackKind::Lexical,
    );
    assert!(matches!(resolve, Err(CoreError::NotReady(_))));

    let reopened = ActivationCatalog::open(dir.path())?;
    let active = reopened.resolve_record(
        candidate.repo_id(),
        candidate.revision_id(),
        SearchPlaneTrackKind::Lexical,
    )?;
    assert_eq!(active.manifest_generation, candidate.manifest_generation());
    Ok(())
}

#[test]
fn activation_staging_write_failure_preserves_complete_active_pointer_v1() -> TestResult {
    let dir = tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let active = corpus_generation(17, "digest-17")?;
    let activation = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(active.clone(), None)?,
    )?;
    let staging = dir.path().join(".staging");
    std::fs::remove_dir(&staging)?;
    std::fs::write(&staging, b"injected non-directory staging path")?;

    let candidate = corpus_generation(18, "digest-18")?;
    let rejected = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(candidate, Some(activation.active))?,
    );
    let Err(CoreError::Storage(message)) = rejected else {
        return Err("activation staging write failure unexpectedly advanced the head".into());
    };
    assert!(message.contains("create staging file"));
    assert_active_composite_v1(&catalog, &active)?;

    std::fs::remove_file(&staging)?;
    std::fs::create_dir(&staging)?;
    let reopened = ActivationCatalog::open(dir.path())?;
    assert_active_composite_v1(&reopened, &active)
}

#[test]
fn rollback_staging_write_failure_preserves_complete_active_pointer_v1() -> TestResult {
    let dir = tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let rollback_target = corpus_generation(17, "digest-17")?;
    let first = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(rollback_target.clone(), None)?,
    )?;
    let active = corpus_generation(18, "digest-18")?;
    let second = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(active.clone(), Some(first.active))?,
    )?;
    let staging = dir.path().join(".staging");
    std::fs::remove_dir(&staging)?;
    std::fs::write(&staging, b"injected non-directory staging path")?;

    let rejected = catalog.rollback(&SearchPlaneRollbackSearchCorpusGenerationCasRequest {
        expected_active: second.active,
        target: corpus_identity(&rollback_target),
    });
    let Err(CoreError::Storage(message)) = rejected else {
        return Err("rollback staging write failure unexpectedly moved the head".into());
    };
    assert!(message.contains("create staging file"));
    assert_active_composite_v1(&catalog, &active)?;

    std::fs::remove_file(&staging)?;
    std::fs::create_dir(&staging)?;
    let reopened = ActivationCatalog::open(dir.path())?;
    assert_active_composite_v1(&reopened, &active)
}

#[test]
fn rollback_parent_sync_failure_fences_serving_and_rehydrates_one_composite_v1() -> TestResult {
    let dir = tempdir()?;
    let sync = Arc::new(ToggleParentSyncFailure {
        fail: AtomicBool::new(false),
    });
    let catalog = ActivationCatalog::open_with_parent_sync(
        dir.path(),
        SearchCorpusPairMutationCoordinator::shared(),
        sync.clone(),
    )?;
    let rollback_target = corpus_generation(17, "digest-17")?;
    let first = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(rollback_target.clone(), None)?,
    )?;
    let active = corpus_generation(18, "digest-18")?;
    let second = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(active.clone(), Some(first.active))?,
    )?;

    sync.fail.store(true, Ordering::SeqCst);
    let rejected = catalog.rollback(&SearchPlaneRollbackSearchCorpusGenerationCasRequest {
        expected_active: second.active,
        target: corpus_identity(&rollback_target),
    });
    assert!(matches!(rejected, Err(CoreError::Storage(_))));
    for track in [
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ] {
        assert!(matches!(
            catalog.resolve_record(active.repo_id(), active.revision_id(), track),
            Err(CoreError::NotReady(_))
        ));
    }

    sync.fail.store(false, Ordering::SeqCst);
    let reopened = ActivationCatalog::open(dir.path())?;
    assert_active_composite_v1(&reopened, &rollback_target)
}

#[test]
fn injected_parent_sync_covers_fresh_catalog_and_authority_directories() -> TestResult {
    let dir = tempdir()?;
    let catalog_root = dir.path().join("fresh-catalog");
    let catalog = ActivationCatalog::open_with_parent_sync(
        &catalog_root,
        SearchCorpusPairMutationCoordinator::shared(),
        Arc::new(AlwaysFailParentSync),
    );
    let Err(CoreError::Storage(catalog_error)) = catalog else {
        return Err("fresh catalog bootstrap bypassed injected parent sync".into());
    };
    assert!(catalog_error.contains("injected parent sync failure"));

    let authority_root = dir.path().join("fresh-authority");
    let authority = AuxiliaryAuthorityStore::open_with_parent_sync(
        &authority_root,
        search_corpus_retention(2)?,
        SearchCorpusPairMutationCoordinator::shared(),
        Arc::new(crate::readiness::auxiliary_store::NoActiveSearchCorpusPinsV1),
        Arc::new(crate::readiness::ScriptedIndexBytesV1),
        Arc::new(AlwaysFailParentSync),
    );
    let Err(CoreError::Storage(authority_error)) = authority else {
        return Err("fresh authority bootstrap bypassed injected parent sync".into());
    };
    assert!(authority_error.contains("injected parent sync failure"));
    Ok(())
}

#[test]
fn activation_catalog_rejects_legacy_per_track_root_before_decode() -> TestResult {
    let dir = tempdir()?;
    let legacy = dir.path().join("repo-corpus--rev-corpus--Lexical.json");
    std::fs::write(&legacy, b"{not-a-composite-root")?;
    let result = ActivationCatalog::open(dir.path());
    let Err(CoreError::Storage(message)) = result else {
        return Err("legacy per-track root unexpectedly opened".into());
    };
    assert!(message.contains("legacy per-track root is unsupported"));
    assert!(message.contains("Lexical.json"));
    Ok(())
}

#[test]
fn activation_catalog_rejects_filename_payload_identity_mismatch() -> TestResult {
    let dir = tempdir()?;
    let generation = corpus_generation(17, "digest-17")?;
    let persisted = crate::readiness::search_corpus_generation::PersistedSearchCorpusGenerationRootV1::from_generation(&generation, NonZeroU64::new(1).expect("positive sequence"));
    let alias = dir.path().join("alias--alias--corpus.json");
    std::fs::write(&alias, serde_json::to_vec_pretty(&persisted)?)?;
    let result = ActivationCatalog::open(dir.path());
    let Err(CoreError::Storage(message)) = result else {
        return Err("filename/payload mismatch unexpectedly opened".into());
    };
    assert!(message.contains("filename/payload identity mismatch"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn activation_catalog_refuses_symlink_composite_root_v1() -> TestResult {
    use std::os::unix::fs::symlink;

    let dir = tempdir()?;
    let attacker_dir = tempdir()?;
    let generation = corpus_generation(17, "digest-17")?;
    let persisted = crate::readiness::search_corpus_generation::PersistedSearchCorpusGenerationRootV1::from_generation(&generation, NonZeroU64::new(1).expect("positive sequence"));
    let attacker_target = attacker_dir.path().join("attacker-controlled.json");
    std::fs::write(&attacker_target, serde_json::to_vec_pretty(&persisted)?)?;
    let activation_path = dir.path().join(
        crate::readiness::activation_catalog::search_corpus_root_file_name(
            generation.repo_id(),
            generation.revision_id(),
        ),
    );
    symlink(&attacker_target, &activation_path)?;

    let result = ActivationCatalog::open(dir.path());
    let Err(CoreError::Storage(message)) = result else {
        return Err("activation catalog silently ignored a symlink composite root".into());
    };
    assert!(message.contains("not a regular non-symlink file"));
    Ok(())
}
