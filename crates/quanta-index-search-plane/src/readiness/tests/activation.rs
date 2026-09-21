#![expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning durability and CAS tests use assertions as test-failure reporting"
)]

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
    AlwaysFailParentSync, FailAtParentSync, TestResult, ToggleParentSyncFailure,
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
    assert_eq!(activation.active.manifest_generation(), ManifestGeneration::new(17));

    let pin = catalog.resolve(
        &RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev-corpus").expect("static fixture ID satisfies canonical policy"),
        SearchPlaneTrackKind::Lexical,
    )?;
    assert_eq!(pin.manifest_generation, ManifestGeneration::new(17));

    let reopened = ActivationCatalog::open(dir.path())?;
    let reopened_pin = reopened.resolve(
        &RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev-corpus").expect("static fixture ID satisfies canonical policy"),
        SearchPlaneTrackKind::Lexical,
    )?;
    assert_eq!(reopened_pin.manifest_generation, ManifestGeneration::new(17));

    let semantic_before = reopened.resolve_record(
        &RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev-corpus").expect("static fixture ID satisfies canonical policy"),
        SearchPlaneTrackKind::Semantic,
    )?;
    assert_eq!(semantic_before.manifest_generation, ManifestGeneration::new(17));

    let rollback = catalog.rollback(&SearchPlaneRollbackSearchCorpusGenerationCasRequest {
        expected_active:
            crate::readiness::search_corpus_generation::search_corpus_generation_into_contract(
                &corpus_generation(17, "digest-17")?,
            ),
        target: crate::readiness::search_corpus_generation::search_corpus_generation_into_contract(
            &corpus_generation(16, "digest-16")?,
        ),
    })?;
    assert_eq!(
        rollback.previous_sealed_active.lexical.manifest_generation,
        ManifestGeneration::new(17)
    );
    assert_eq!(rollback.active.lexical.manifest_generation, ManifestGeneration::new(16));
    let reopened = ActivationCatalog::open(dir.path())?;
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
    assert_eq!(lexical_after.manifest_generation, ManifestGeneration::new(16));
    assert_eq!(semantic_after.manifest_generation, ManifestGeneration::new(16));
    assert_eq!(lexical_after.manifest_digest, "digest-16");
    assert_eq!(semantic_after.manifest_digest, "digest-16");
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
    assert!(single_track_message.contains("lexical and semantic tracks"));

    let mixed_identity = SearchCorpusGenerationV1::new(
        corpus_snapshot(SearchPlaneTrackKind::Lexical, 17, "digest-17"),
        corpus_snapshot(SearchPlaneTrackKind::Semantic, 18, "digest-18"),
        crate::content_roots_test_support::roots_for_generation(17),
    );
    let Err(CoreError::InvalidContract(mixed_identity_message)) = mixed_identity else {
        return Err("mixed corpus generation unexpectedly constructed".into());
    };
    assert!(mixed_identity_message.contains("must match exactly"));

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
        Some(foreign_expected),
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
    assert_eq!(first_receipt.active, first);
    assert_eq!(first_receipt.previous_active, None);

    let root = dir
        .path()
        .join(crate::readiness::activation_catalog::search_corpus_root_file_name(
            first.repo_id(),
            first.revision_id(),
        ));
    assert!(root.is_file(), "composite root must exist before memory receipt");
    let directory_entries = std::fs::read_dir(dir.path())?.collect::<Result<Vec<_>, _>>()?;
    assert!(
        directory_entries
            .iter()
            .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp-")),
        "durable activation must not leave temporary roots behind"
    );

    let second = corpus_generation(18, "digest-18")?;
    let promoted_prepared =
        PreparedSearchCorpusGenerationV1::new(second.clone(), Some(first.clone()))?;
    let promoted = catalog.activate_prepared_search_corpus_generation_v1(&promoted_prepared)?;
    assert_eq!(promoted.active, second);
    assert_eq!(promoted.previous_active, Some(first.clone()));

    let stale_prepared =
        PreparedSearchCorpusGenerationV1::new(corpus_generation(19, "digest-19")?, Some(first))?;
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
fn activation_catalog_concurrent_cas_promotions_select_one_composite_winner() -> TestResult {
    let dir = tempdir()?;
    let catalog = Arc::new(ActivationCatalog::open(dir.path())?);
    let active = corpus_generation(17, "digest-17")?;
    let initial_active = active.clone();
    let _initial_activation = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(active.clone(), None)?,
    )?;

    let first_candidate = corpus_generation(18, "digest-18")?;
    let second_candidate = corpus_generation(19, "digest-19")?;
    let first_prepared =
        PreparedSearchCorpusGenerationV1::new(first_candidate.clone(), Some(active.clone()))?;
    let second_prepared =
        PreparedSearchCorpusGenerationV1::new(second_candidate.clone(), Some(active))?;

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
                    format!("concurrent activation returned unexpected error: {error}").into()
                );
            }
        }
    }
    let winner = winner.ok_or("concurrent activation CAS produced no winner")?;
    assert!(winner == first_candidate || winner == second_candidate);

    let lexical = catalog.resolve_record(
        winner.repo_id(),
        winner.revision_id(),
        SearchPlaneTrackKind::Lexical,
    )?;
    let semantic = catalog.resolve_record(
        winner.repo_id(),
        winner.revision_id(),
        SearchPlaneTrackKind::Semantic,
    )?;
    assert_eq!(lexical.manifest_generation, winner.manifest_generation());
    assert_eq!(semantic.manifest_generation, winner.manifest_generation());
    assert_eq!(lexical.manifest_digest, winner.manifest_digest());
    assert_eq!(semantic.manifest_digest, winner.manifest_digest());
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
    assert_eq!(activation.active, first);

    // This is the post-rename / parent-fsync-failure state. The next
    // process reconstructs from the durable root; this one must never
    // serve its potentially stale in-memory records in the meantime.
    catalog.mark_durability_uncertain_v1();

    let resolve =
        catalog.resolve_record(first.repo_id(), first.revision_id(), SearchPlaneTrackKind::Lexical);
    let Err(CoreError::NotReady(resolve_message)) = resolve else {
        return Err("durability-uncertain catalog unexpectedly served a read".into());
    };
    assert!(resolve_message.contains("reopen the catalog"));

    let second = corpus_generation(18, "digest-18")?;
    let mutate = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(second, Some(first))?,
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
            // Existing-root durability revalidation is call zero and the
            // staging-directory creation fence is call one; the
            // activation-file target-parent sync after rename is call two.
            fail_at: 2,
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

    let persisted =
        dir.path()
            .join(crate::readiness::activation_catalog::search_corpus_root_file_name(
                candidate.repo_id(),
                candidate.revision_id(),
            ));
    assert!(persisted.is_file(), "rename must precede injected sync failure");
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
    let _activation = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(active.clone(), None)?,
    )?;
    let staging = dir.path().join(".staging");
    std::fs::remove_dir(&staging)?;
    std::fs::write(&staging, b"injected non-directory staging path")?;

    let candidate = corpus_generation(18, "digest-18")?;
    let rejected = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(candidate, Some(active.clone()))?,
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
    let _first = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(rollback_target.clone(), None)?,
    )?;
    let active = corpus_generation(18, "digest-18")?;
    let _second = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(active.clone(), Some(rollback_target.clone()))?,
    )?;
    let staging = dir.path().join(".staging");
    std::fs::remove_dir(&staging)?;
    std::fs::write(&staging, b"injected non-directory staging path")?;

    let rejected = catalog.rollback(&SearchPlaneRollbackSearchCorpusGenerationCasRequest {
        expected_active: corpus_identity(&active),
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
    let _first = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(rollback_target.clone(), None)?,
    )?;
    let active = corpus_generation(18, "digest-18")?;
    let _second = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(active.clone(), Some(rollback_target.clone()))?,
    )?;

    sync.fail.store(true, Ordering::SeqCst);
    let rejected = catalog.rollback(&SearchPlaneRollbackSearchCorpusGenerationCasRequest {
        expected_active: corpus_identity(&active),
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
    let persisted = crate::readiness::search_corpus_generation::PersistedSearchCorpusGenerationRootV1::from_generation(&generation);
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
    let persisted = crate::readiness::search_corpus_generation::PersistedSearchCorpusGenerationRootV1::from_generation(&generation);
    let attacker_target = attacker_dir.path().join("attacker-controlled.json");
    std::fs::write(&attacker_target, serde_json::to_vec_pretty(&persisted)?)?;
    let activation_path =
        dir.path()
            .join(crate::readiness::activation_catalog::search_corpus_root_file_name(
                generation.repo_id(),
                generation.revision_id(),
            ));
    symlink(&attacker_target, &activation_path)?;

    let result = ActivationCatalog::open(dir.path());
    let Err(CoreError::Storage(message)) = result else {
        return Err("activation catalog silently ignored a symlink composite root".into());
    };
    assert!(message.contains("not a regular non-symlink file"));
    Ok(())
}
