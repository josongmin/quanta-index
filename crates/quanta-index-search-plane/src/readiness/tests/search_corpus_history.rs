#![expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning durability and CAS tests use assertions as test-failure reporting"
)]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use quanta_index_contract::{
    GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneTrackKind,
};
use quanta_index_core::CoreError;
use tempfile::tempdir;

use crate::SearchCorpusLifecycleOwner;
use crate::readiness::TEST_INDEX_BYTES_PER_GENERATION;
use crate::readiness::auxiliary_store::AuxiliaryAuthorityStore;
use crate::readiness::ledger::Ledger;
use crate::readiness::search_corpus_generation::{
    PreparedSearchCorpusGenerationV1, SearchCorpusGenerationV1,
};
use crate::readiness::tests::support::{
    FailAtParentSync, FailNthSyncForParent, TestResult, assert_active_composite_v1,
    corpus_generation, corpus_identity, search_corpus_history_file_names_v1,
    search_corpus_retention,
};
use crate::search_corpus_lifecycle::SearchCorpusPairMutationCoordinator;
use crate::search_corpus_retention::SearchCorpusHistoryRetentionPolicyV1;

#[test]
fn sealed_search_corpus_history_reaps_max_plus_one_and_preserves_predecessor() -> TestResult {
    let dir = tempdir()?;
    let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    let repo = RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy");
    let revision =
        RevisionId::new("rev-corpus").expect("static fixture ID satisfies canonical policy");
    let mut live_ledger = Ledger::new();
    for generation in 17..=19 {
        let manifest_generation = ManifestGeneration::new(generation);
        let digest = format!("digest-{generation}");
        let retention = store.record_sealed_search_corpus(
            &repo,
            &revision,
            manifest_generation,
            digest.as_str(),
        )?;
        live_ledger.apply_search_corpus_history_retention_receipt_v1(
            &repo,
            &revision,
            manifest_generation,
            &retention,
        )?;
        live_ledger.record_historically_sealed_search_corpus(
            &repo,
            &revision,
            manifest_generation,
            digest.as_str(),
        );
    }
    let conflict = store.record_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(18),
        "conflicting-digest",
    );
    let Err(CoreError::Typed { code, .. }) = conflict else {
        return Err("conflicting historical digest unexpectedly overwrote authority".into());
    };
    assert_eq!(
        code,
        quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusAuthorityConflict
    );
    let root_snapshot = store.load_search_corpus_root_snapshot_v1()?;
    assert_eq!(
        root_snapshot.pair_directories.len(),
        1,
        "one repo/revision history must occupy one bounded pair directory independently of the owned staging surface"
    );
    let pair_dir = store.search_corpus_pair_dir(&repo, &revision);
    assert_eq!(
        std::fs::read_dir(pair_dir)?.count(),
        2,
        "count policy must keep only the newest generation and predecessor"
    );
    assert!(
        !store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(17))
            .exists()
    );
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(18))
            .is_file()
    );
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(19))
            .is_file()
    );
    let reaped_same_process = live_ledger.validate_historically_sealed_track_identity(
        &GenerationSnapshot {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(17),
            manifest_digest: "digest-17".to_string(),
        },
        "test",
    );
    assert!(matches!(reaped_same_process, Err(CoreError::Typed { .. })));

    let reopened = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    let mut ledger = Ledger::new();
    reopened.restore_into(&mut ledger)?;
    for track in [
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ] {
        ledger.validate_historically_sealed_track_identity(
            &GenerationSnapshot {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                track,
                manifest_generation: ManifestGeneration::new(18),
                manifest_digest: "digest-18".to_string(),
            },
            "test",
        )?;
    }
    Ok(())
}

#[test]
fn unreconciled_retention_receipt_fails_before_ledger_pruning() -> TestResult {
    let repo = RepoId::new("repo-incomplete-receipt")
        .expect("static fixture ID satisfies canonical policy");
    let revision = RevisionId::new("rev-incomplete-receipt")
        .expect("static fixture ID satisfies canonical policy");
    let mut ledger = Ledger::new();
    ledger.record_historically_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(17),
        "digest-17",
    );
    let receipt = crate::readiness::retention_receipt::SearchCorpusHistoryRetentionReceiptV1 {
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        retained_generations: BTreeSet::from([ManifestGeneration::new(18)]),
        reaped_generations: BTreeSet::new(),
        retained_index_bytes: 0,
        store_reconciled_v1: false,
    };
    assert!(
        ledger
            .apply_search_corpus_history_retention_receipt_v1(
                &repo,
                &revision,
                ManifestGeneration::new(18),
                &receipt,
            )
            .is_err()
    );
    ledger.validate_historically_sealed_track_identity(
        &GenerationSnapshot {
            repo_id: repo,
            revision_id: revision,
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(17),
            manifest_digest: "digest-17".to_string(),
        },
        "test",
    )?;
    Ok(())
}

#[test]
fn sealed_search_corpus_history_enforces_byte_cap_without_losing_predecessor() -> TestResult {
    let dir = tempdir()?;
    let repo = RepoId::new("repo-byte-cap").expect("static fixture ID satisfies canonical policy");
    let revision =
        RevisionId::new("rev-byte-cap").expect("static fixture ID satisfies canonical policy");
    let first_len = TEST_INDEX_BYTES_PER_GENERATION;
    let second_len = TEST_INDEX_BYTES_PER_GENERATION;
    let policy =
        SearchCorpusHistoryRetentionPolicyV1::new(4, first_len + second_len, 64, 64 * 1024 * 1024)?;
    let store = AuxiliaryAuthorityStore::open(dir.path(), policy)?;
    for generation in 18..=20 {
        let _retention_receipt = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(generation),
            format!("digest-{generation}").as_str(),
        )?;
    }
    let pair_dir = store.search_corpus_pair_dir(&repo, &revision);
    assert_eq!(std::fs::read_dir(pair_dir)?.count(), 2);
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(19))
            .is_file()
    );
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(20))
            .is_file()
    );
    Ok(())
}

#[test]
fn sealed_search_corpus_history_rejects_write_before_predecessor_window_overflows() -> TestResult {
    let dir = tempdir()?;
    let repo =
        RepoId::new("repo-byte-exhausted").expect("static fixture ID satisfies canonical policy");
    let revision = RevisionId::new("rev-byte-exhausted")
        .expect("static fixture ID satisfies canonical policy");
    let first_len = TEST_INDEX_BYTES_PER_GENERATION;
    let second_len = TEST_INDEX_BYTES_PER_GENERATION;
    let pair_bytes = first_len
        .checked_add(second_len)
        .and_then(|sum| sum.checked_sub(1))
        .ok_or("test byte cap underflow")?;
    let policy = SearchCorpusHistoryRetentionPolicyV1::new(4, pair_bytes, 64, 64 * 1024 * 1024)?;
    let store = AuxiliaryAuthorityStore::open(dir.path(), policy)?;
    let _initial_retention_receipt = store.record_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(18),
        "digest-18",
    )?;
    let rejected = store.record_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(19),
        "digest-19",
    );
    let Err(CoreError::Typed { code, .. }) = rejected else {
        return Err("byte-exhausted predecessor window unexpectedly admitted".into());
    };
    assert_eq!(
        code,
        quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted
    );
    assert_eq!(
        std::fs::read_dir(store.search_corpus_pair_dir(&repo, &revision))?.count(),
        1,
        "preflight must reject before a durable max+1 write"
    );
    Ok(())
}

#[test]
fn state_root_revision_pair_cap_rejects_growth_without_cross_pair_deletion() -> TestResult {
    let dir = tempdir()?;
    let policy = SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 1, 4 * 1024 * 1024)?;
    let store = AuxiliaryAuthorityStore::open(dir.path(), policy)?;
    let first_repo =
        RepoId::new("repo-global-first").expect("static fixture ID satisfies canonical policy");
    let first_revision =
        RevisionId::new("rev-global-first").expect("static fixture ID satisfies canonical policy");
    let _first_pair_retention_receipt = store.record_sealed_search_corpus(
        &first_repo,
        &first_revision,
        ManifestGeneration::new(1),
        "digest-first",
    )?;

    let second_repo =
        RepoId::new("repo-global-second").expect("static fixture ID satisfies canonical policy");
    let second_revision =
        RevisionId::new("rev-global-second").expect("static fixture ID satisfies canonical policy");
    let rejected = store.record_sealed_search_corpus(
        &second_repo,
        &second_revision,
        ManifestGeneration::new(1),
        "digest-second",
    );
    let Err(CoreError::Typed { code, .. }) = rejected else {
        return Err("state-root revision-pair overflow unexpectedly admitted".into());
    };
    assert_eq!(
        code,
        quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted
    );
    assert!(
        store
            .search_corpus_authority_path(&first_repo, &first_revision, ManifestGeneration::new(1),)
            .is_file(),
        "global admission must not delete another pair without active-pin authority"
    );
    assert!(
        !store
            .search_corpus_pair_dir(&second_repo, &second_revision)
            .exists(),
        "rejected admission must not leave an empty durable pair directory"
    );
    Ok(())
}

#[test]
fn state_root_total_byte_cap_rejects_growth_before_write() -> TestResult {
    let dir = tempdir()?;
    let first_repo =
        RepoId::new("repo-byte-root-first").expect("static fixture ID satisfies canonical policy");
    let first_revision = RevisionId::new("rev-byte-root-first")
        .expect("static fixture ID satisfies canonical policy");
    let second_repo =
        RepoId::new("repo-byte-root-second").expect("static fixture ID satisfies canonical policy");
    let second_revision = RevisionId::new("rev-byte-root-second")
        .expect("static fixture ID satisfies canonical policy");
    let first_len = TEST_INDEX_BYTES_PER_GENERATION;
    let second_len = TEST_INDEX_BYTES_PER_GENERATION;
    let pair_limit = first_len.max(second_len);
    let store = AuxiliaryAuthorityStore::open(
        dir.path(),
        SearchCorpusHistoryRetentionPolicyV1::new(2, pair_limit, 8, pair_limit)?,
    )?;
    let _first_pair_retention_receipt = store.record_sealed_search_corpus(
        &first_repo,
        &first_revision,
        ManifestGeneration::new(1),
        "digest-first",
    )?;
    let rejected = store.record_sealed_search_corpus(
        &second_repo,
        &second_revision,
        ManifestGeneration::new(1),
        "digest-second",
    );
    let Err(CoreError::Typed { code, .. }) = rejected else {
        return Err("state-root byte overflow unexpectedly admitted".into());
    };
    assert_eq!(
        code,
        quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted
    );
    assert!(
        !store
            .search_corpus_pair_dir(&second_repo, &second_revision)
            .exists()
    );
    Ok(())
}

#[test]
fn restore_refuses_cross_pair_gc_when_state_root_pair_cap_shrinks() -> TestResult {
    let dir = tempdir()?;
    let writer = AuxiliaryAuthorityStore::open(
        dir.path(),
        SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 2, 4 * 1024 * 1024)?,
    )?;
    for ordinal in 1..=2 {
        let _retention_receipt = writer.record_sealed_search_corpus(
            &RepoId::new(format!("repo-shrink-{ordinal}"))
                .expect("test fixture ID satisfies canonical policy"),
            &RevisionId::new(format!("rev-shrink-{ordinal}"))
                .expect("test fixture ID satisfies canonical policy"),
            ManifestGeneration::new(1),
            format!("digest-{ordinal}").as_str(),
        )?;
    }
    drop(writer);

    let reopened = AuxiliaryAuthorityStore::open(
        dir.path(),
        SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 1, 4 * 1024 * 1024)?,
    )?;
    let mut ledger = Ledger::new();
    let Err(CoreError::Typed { code, .. }) = reopened.restore_into(&mut ledger) else {
        return Err("restore guessed a cross-pair GC victim".into());
    };
    assert_eq!(
        code,
        quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted
    );
    assert_eq!(
        reopened.load_search_corpus_root_snapshot_v1()?.pairs.len(),
        2,
        "failed restore must preserve every pair when active-pin authority is unavailable"
    );
    Ok(())
}

#[test]
fn restart_repairs_one_over_limit_before_restoring_history() -> TestResult {
    let dir = tempdir()?;
    let repo =
        RepoId::new("repo-restart-gc").expect("static fixture ID satisfies canonical policy");
    let revision =
        RevisionId::new("rev-restart-gc").expect("static fixture ID satisfies canonical policy");
    let writer = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(3)?)?;
    for generation in 17..=19 {
        let _retention_receipt = writer.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(generation),
            format!("digest-{generation}").as_str(),
        )?;
    }
    drop(writer);

    let reopened = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    let mut ledger = Ledger::new();
    reopened.restore_into(&mut ledger)?;
    assert_eq!(
        std::fs::read_dir(reopened.search_corpus_pair_dir(&repo, &revision))?.count(),
        2
    );
    for generation in [18, 19] {
        ledger.validate_historically_sealed_track_identity(
            &GenerationSnapshot {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(generation),
                manifest_digest: format!("digest-{generation}"),
            },
            "test",
        )?;
    }
    Ok(())
}

#[test]
fn restore_fails_closed_on_foreign_pair_entry() -> TestResult {
    let dir = tempdir()?;
    let repo = RepoId::new("repo-foreign").expect("static fixture ID satisfies canonical policy");
    let revision =
        RevisionId::new("rev-foreign").expect("static fixture ID satisfies canonical policy");
    let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    let _retention_receipt = store.record_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(17),
        "digest-17",
    )?;
    std::fs::write(
        store
            .search_corpus_pair_dir(&repo, &revision)
            .join("foreign.tmp"),
        b"foreign",
    )?;
    let mut ledger = Ledger::new();
    let Err(CoreError::Storage(message)) = store.restore_into(&mut ledger) else {
        return Err("foreign history entry unexpectedly ignored".into());
    };
    assert!(message.contains("foreign history entry"));
    Ok(())
}

#[test]
fn startup_reconciles_owned_staging_and_empty_pair_v1() -> TestResult {
    let dir = tempdir()?;
    let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    let staging = store.search_corpus_staging_dir.clone();
    let empty_pair = store.search_corpus_dir.join("a".repeat(64));
    std::fs::write(staging.join("scv1-pair-g17.cbor.tmp-1-1"), b"abandoned")?;
    std::fs::create_dir(&empty_pair)?;
    drop(store);

    let reopened = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    assert!(
        std::fs::read_dir(&reopened.search_corpus_staging_dir)?
            .next()
            .is_none()
    );
    assert!(!empty_pair.exists());
    Ok(())
}

#[test]
fn retention_missing_active_history_preserves_activation_and_empty_history_v1() -> TestResult {
    let dir = tempdir()?;
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        search_corpus_retention(2)?,
        Arc::new(crate::readiness::ScriptedIndexBytesV1),
    )?;
    let catalog = owner.activation_catalog();
    let store = owner.authority_store();
    let active = corpus_generation(1, "digest-active-1")?;
    let coordinator = owner.coordinator();
    {
        let guard = coordinator.lock_pair(active.repo_id(), active.revision_id())?;
        let _activation = catalog.activate_prepared_under_guard_v1(
            &guard,
            &PreparedSearchCorpusGenerationV1::new(active.clone(), None)?,
            None,
        )?;
    }
    let history_before =
        search_corpus_history_file_names_v1(&store, active.repo_id(), active.revision_id())?;
    assert!(history_before.is_empty());

    let rejected = store.record_sealed_search_corpus(
        active.repo_id(),
        active.revision_id(),
        ManifestGeneration::new(2),
        "digest-candidate-2",
    );
    let Err(CoreError::Storage(message)) = rejected else {
        return Err("retention admitted a candidate without durable active history".into());
    };
    assert!(message.contains("active generation is absent from durable history"));
    assert_active_composite_v1(&catalog, &active)?;
    assert_eq!(
        search_corpus_history_file_names_v1(&store, active.repo_id(), active.revision_id(),)?,
        history_before
    );
    Ok(())
}

#[test]
fn retention_active_digest_mismatch_preserves_activation_and_history_v1() -> TestResult {
    let dir = tempdir()?;
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        search_corpus_retention(2)?,
        Arc::new(crate::readiness::ScriptedIndexBytesV1),
    )?;
    let catalog = owner.activation_catalog();
    let store = owner.authority_store();
    let active = corpus_generation(1, "digest-active-1")?;
    let _history = store.record_sealed_search_corpus(
        active.repo_id(),
        active.revision_id(),
        active.manifest_generation(),
        "digest-history-1",
    )?;
    let coordinator = owner.coordinator();
    {
        let guard = coordinator.lock_pair(active.repo_id(), active.revision_id())?;
        let _activation = catalog.activate_prepared_under_guard_v1(
            &guard,
            &PreparedSearchCorpusGenerationV1::new(active.clone(), None)?,
            None,
        )?;
    }
    let history_before =
        search_corpus_history_file_names_v1(&store, active.repo_id(), active.revision_id())?;

    let rejected = store.record_sealed_search_corpus(
        active.repo_id(),
        active.revision_id(),
        ManifestGeneration::new(2),
        "digest-candidate-2",
    );
    let Err(CoreError::Storage(message)) = rejected else {
        return Err("retention admitted an active digest absent from durable history".into());
    };
    assert!(message.contains("active generation is absent from durable history"));
    assert_active_composite_v1(&catalog, &active)?;
    assert_eq!(
        search_corpus_history_file_names_v1(&store, active.repo_id(), active.revision_id(),)?,
        history_before
    );
    assert_eq!(
        store.inspect_sealed_search_corpus(
            active.repo_id(),
            active.revision_id(),
            active.manifest_generation(),
            "digest-history-1",
        )?,
        crate::readiness::search_corpus_history::SealedSearchCorpusAuthorityStateV1::Exact
    );
    Ok(())
}

#[test]
fn retention_required_set_exhaustion_preserves_activation_and_history_v1() -> TestResult {
    let dir = tempdir()?;
    let repo = RepoId::new("repo-required-set-exhausted")
        .expect("static fixture ID satisfies canonical policy");
    let revision = RevisionId::new("rev-required-set-exhausted")
        .expect("static fixture ID satisfies canonical policy");
    let active_digest = "digest-active-1";
    let candidate_digest = "digest-candidate-2";
    let active_len = TEST_INDEX_BYTES_PER_GENERATION;
    let candidate_len = TEST_INDEX_BYTES_PER_GENERATION;
    let policy = SearchCorpusHistoryRetentionPolicyV1::new(
        2,
        active_len.max(candidate_len),
        8,
        8 * 1024 * 1024,
    )?;
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        policy,
        Arc::new(crate::readiness::ScriptedIndexBytesV1),
    )?;
    let catalog = owner.activation_catalog();
    let store = owner.authority_store();
    let active = SearchCorpusGenerationV1::new(
        GenerationSnapshot {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(1),
            manifest_digest: active_digest.to_string(),
        },
        GenerationSnapshot {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: ManifestGeneration::new(1),
            manifest_digest: active_digest.to_string(),
        },
        crate::content_roots_test_support::roots_for_generation(1),
    )?;
    let _history = store.record_sealed_search_corpus(
        &repo,
        &revision,
        active.manifest_generation(),
        active_digest,
    )?;
    let coordinator = owner.coordinator();
    {
        let guard = coordinator.lock_pair(&repo, &revision)?;
        let _activation = catalog.activate_prepared_under_guard_v1(
            &guard,
            &PreparedSearchCorpusGenerationV1::new(active.clone(), None)?,
            None,
        )?;
    }
    let history_before = search_corpus_history_file_names_v1(&store, &repo, &revision)?;

    let rejected = store.record_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(2),
        candidate_digest,
    );
    let Err(CoreError::Typed { code, .. }) = rejected else {
        return Err("required active/candidate set unexpectedly fit below byte cap".into());
    };
    assert_eq!(
        code,
        quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted
    );
    assert_active_composite_v1(&catalog, &active)?;
    assert_eq!(
        search_corpus_history_file_names_v1(&store, &repo, &revision)?,
        history_before
    );
    assert!(
        !store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(2),)
            .exists(),
        "retention preflight must reject before candidate write"
    );
    Ok(())
}

#[test]
fn retention_preserves_rolled_back_active_generation_before_next_activation_v1() -> TestResult {
    let dir = tempdir()?;
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        search_corpus_retention(2)?,
        Arc::new(crate::readiness::ScriptedIndexBytesV1),
    )?;
    let store = owner.authority_store();
    let catalog = owner.activation_catalog();
    let repo = RepoId::new("repo-corpus").expect("static fixture ID satisfies canonical policy");
    let revision =
        RevisionId::new("rev-corpus").expect("static fixture ID satisfies canonical policy");
    for raw_generation in 1..=2 {
        let _receipt = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(raw_generation),
            &format!("digest-{raw_generation}"),
        )?;
    }
    let generation_one = corpus_generation(1, "digest-1")?;
    let generation_two = corpus_generation(2, "digest-2")?;
    let coordinator = owner.coordinator();
    {
        let guard = coordinator.lock_pair(&repo, &revision)?;
        let activation = catalog.activate_prepared_under_guard_v1(
            &guard,
            &PreparedSearchCorpusGenerationV1::new(generation_two, None)?,
            None,
        )?;
        drop(guard);
        let guard = coordinator.lock_pair(&repo, &revision)?;
        let _rollback = catalog.rollback_under_guard_v1(
            &guard,
            &SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active: activation.active,
                target: corpus_identity(&generation_one),
            },
        )?;
    }

    let _receipt = store.record_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(3),
        "digest-3",
    )?;
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(1))
            .is_file()
    );
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(3))
            .is_file()
    );
    assert!(
        !store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(2))
            .exists()
    );
    Ok(())
}

#[test]
fn startup_rejects_foreign_staging_entry_v1() -> TestResult {
    let dir = tempdir()?;
    let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    std::fs::write(
        store.search_corpus_staging_dir.join("foreign.tmp"),
        b"foreign",
    )?;
    drop(store);

    let Err(CoreError::Storage(message)) =
        AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)
    else {
        return Err("foreign staging entry unexpectedly reconciled".into());
    };
    assert!(message.contains("foreign staging entry"));
    Ok(())
}

#[test]
fn startup_rejects_non_hex_pair_directory_v1() -> TestResult {
    let dir = tempdir()?;
    let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    let foreign_pair = store.search_corpus_dir.join("G".repeat(64));
    std::fs::create_dir(&foreign_pair)?;
    drop(store);

    let Err(CoreError::Storage(message)) =
        AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)
    else {
        return Err("non-hex pair directory unexpectedly reconciled".into());
    };
    assert!(message.contains("foreign root entry"));
    assert!(foreign_pair.exists());
    Ok(())
}

#[test]
fn concurrent_history_writes_serialize_gc_and_remain_bounded() -> TestResult {
    let dir = tempdir()?;
    let store = Arc::new(AuxiliaryAuthorityStore::open(
        dir.path(),
        search_corpus_retention(3)?,
    )?);
    let repo =
        RepoId::new("repo-concurrent-gc").expect("static fixture ID satisfies canonical policy");
    let revision =
        RevisionId::new("rev-concurrent-gc").expect("static fixture ID satisfies canonical policy");
    let barrier = Arc::new(Barrier::new(8));
    let mut workers = Vec::new();
    for generation in 1..=8 {
        let worker_store = store.clone();
        let worker_repo = repo.clone();
        let worker_revision = revision.clone();
        let worker_barrier = barrier.clone();
        workers.push(thread::spawn(move || {
            let _barrier_receipt = worker_barrier.wait();
            worker_store.record_sealed_search_corpus(
                &worker_repo,
                &worker_revision,
                ManifestGeneration::new(generation),
                format!("digest-{generation}").as_str(),
            )
        }));
    }
    for worker in workers {
        match worker
            .join()
            .map_err(|_panic_payload| "history GC worker panicked")?
        {
            Ok(_receipt) => {}
            Err(CoreError::Typed {
                code:
                    quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
                ..
            }) => {}
            Err(error) => return Err(format!("unexpected concurrent GC error: {error}").into()),
        }
    }
    assert_eq!(
        std::fs::read_dir(store.search_corpus_pair_dir(&repo, &revision))?.count(),
        3
    );
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(8))
            .is_file()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn search_corpus_authority_refuses_symlink_records() -> TestResult {
    use std::os::unix::fs::symlink;

    let dir = tempdir()?;
    let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    let repo = RepoId::new("repo-symlink").expect("static fixture ID satisfies canonical policy");
    let revision =
        RevisionId::new("rev-symlink").expect("static fixture ID satisfies canonical policy");
    let generation = ManifestGeneration::new(17);
    let record_path = store.search_corpus_authority_path(&repo, &revision, generation);
    let parent = record_path.parent().ok_or("authority path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let attacker_target = dir.path().join("attacker-controlled.cbor");
    std::fs::write(&attacker_target, b"not-authority")?;
    symlink(&attacker_target, &record_path)?;

    let observed = store.inspect_sealed_search_corpus(&repo, &revision, generation, "digest-17");
    let Err(CoreError::Storage(message)) = observed else {
        return Err("authority store followed a symlink record".into());
    };
    assert!(message.contains("read search corpus"));
    Ok(())
}

#[test]
fn durable_pair_names_are_bounded_and_length_delimited() {
    let first = crate::readiness::pair_digest::search_corpus_pair_digest(
        &RepoId::new("repo--with--delimiter")
            .expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
    );
    let second = crate::readiness::pair_digest::search_corpus_pair_digest(
        &RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("with--delimiter--rev")
            .expect("static fixture ID satisfies canonical policy"),
    );
    assert_ne!(first, second);
    assert_eq!(first.len(), 64);

    let long = crate::readiness::activation_catalog::search_corpus_root_file_name(
        &RepoId::new("r".repeat(512)).expect("maximum length fixture repo ID is canonical"),
        &RevisionId::new("v".repeat(512)).expect("maximum length fixture revision ID is canonical"),
    );
    assert_eq!(long.len(), 64 + "--corpus.json".len());
    assert!(RepoId::new("r".repeat(8_192)).is_err());
    assert!(RevisionId::new("v".repeat(8_192)).is_err());
}

#[test]
fn sealed_search_corpus_retry_revalidates_parent_durability() -> TestResult {
    let dir = tempdir()?;
    drop(AuxiliaryAuthorityStore::open(
        dir.path(),
        search_corpus_retention(2)?,
    )?);
    let sync = Arc::new(FailAtParentSync {
        calls: AtomicUsize::new(0),
        // Revalidating the authority root plus five owned directories
        // consumes calls 0..=5. Fail pair-directory durability at call six
        // so the retry must revalidate that existing directory.
        fail_at: 6,
    });
    let store = AuxiliaryAuthorityStore::open_with_parent_sync(
        dir.path(),
        search_corpus_retention(2)?,
        SearchCorpusPairMutationCoordinator::shared(),
        Arc::new(crate::readiness::auxiliary_store::NoActiveSearchCorpusPinsV1),
        Arc::new(crate::readiness::ScriptedIndexBytesV1),
        sync.clone(),
    )?;
    let repo = RepoId::new("repo-retry").expect("static fixture ID satisfies canonical policy");
    let revision =
        RevisionId::new("rev-retry").expect("static fixture ID satisfies canonical policy");
    let first = store.record_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(17),
        "digest-17",
    );
    assert!(matches!(first, Err(CoreError::Storage(_))));

    let _retry_retention_receipt = store.record_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(17),
        "digest-17",
    )?;
    assert!(
        sync.calls.load(Ordering::SeqCst) >= 8,
        "retry must re-fsync the pair-directory parent and immutable record parent"
    );
    Ok(())
}

#[test]
fn sealed_search_corpus_retry_repairs_post_rename_parent_sync_failure() -> TestResult {
    let dir = tempdir()?;
    drop(AuxiliaryAuthorityStore::open(
        dir.path(),
        search_corpus_retention(2)?,
    )?);
    let sync = Arc::new(FailAtParentSync {
        calls: AtomicUsize::new(0),
        // Revalidating the authority root plus five owned directories
        // consumes calls 0..=5, pair-directory durability is call six,
        // and target-parent sync after the immutable rename is call seven.
        fail_at: 7,
    });
    let store = AuxiliaryAuthorityStore::open_with_parent_sync(
        dir.path(),
        search_corpus_retention(2)?,
        SearchCorpusPairMutationCoordinator::shared(),
        Arc::new(crate::readiness::auxiliary_store::NoActiveSearchCorpusPinsV1),
        Arc::new(crate::readiness::ScriptedIndexBytesV1),
        sync.clone(),
    )?;
    let repo =
        RepoId::new("repo-post-rename").expect("static fixture ID satisfies canonical policy");
    let revision =
        RevisionId::new("rev-post-rename").expect("static fixture ID satisfies canonical policy");
    let generation = ManifestGeneration::new(17);
    let first =
        store.record_sealed_search_corpus(&repo, &revision, generation, "digest-post-rename");
    assert!(matches!(first, Err(CoreError::Storage(_))));
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, generation)
            .is_file(),
        "rename must precede the injected parent sync failure"
    );

    let _retry_retention_receipt =
        store.record_sealed_search_corpus(&repo, &revision, generation, "digest-post-rename")?;
    assert!(sync.calls.load(Ordering::SeqCst) >= 4);
    assert_eq!(
        store.inspect_sealed_search_corpus(&repo, &revision, generation, "digest-post-rename",)?,
        crate::readiness::search_corpus_history::SealedSearchCorpusAuthorityStateV1::Exact
    );
    Ok(())
}

#[test]
fn sealed_search_corpus_retry_repairs_staging_parent_sync_failure_v1() -> TestResult {
    let dir = tempdir()?;
    drop(AuxiliaryAuthorityStore::open(
        dir.path(),
        search_corpus_retention(2)?,
    )?);
    let staging_dir = dir.path().join("search-corpus/.staging");
    let sync = Arc::new(FailNthSyncForParent {
        target_parent: staging_dir,
        matching_calls: AtomicUsize::new(0),
        fail_at_matching_call: 0,
    });
    let store = AuxiliaryAuthorityStore::open_with_parent_sync(
        dir.path(),
        search_corpus_retention(2)?,
        SearchCorpusPairMutationCoordinator::shared(),
        Arc::new(crate::readiness::auxiliary_store::NoActiveSearchCorpusPinsV1),
        Arc::new(crate::readiness::ScriptedIndexBytesV1),
        sync.clone(),
    )?;
    let repo =
        RepoId::new("repo-staging-retry").expect("static fixture ID satisfies canonical policy");
    let revision =
        RevisionId::new("rev-staging-retry").expect("static fixture ID satisfies canonical policy");
    let generation = ManifestGeneration::new(17);

    let first =
        store.record_sealed_search_corpus(&repo, &revision, generation, "digest-staging-retry");
    assert!(matches!(first, Err(CoreError::Storage(_))));
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, generation)
            .is_file(),
        "target rename must precede the injected staging-parent sync failure"
    );

    let _reconciled =
        store.record_sealed_search_corpus(&repo, &revision, generation, "digest-staging-retry")?;
    assert_eq!(
        sync.matching_calls.load(Ordering::SeqCst),
        2,
        "exact retry must re-fsync the source staging directory"
    );
    Ok(())
}

#[test]
fn post_delete_fsync_failure_fences_rollback_and_retry_reconciles_authoritative_set() -> TestResult
{
    let dir = tempdir()?;
    let repo =
        RepoId::new("repo-post-delete").expect("static fixture ID satisfies canonical policy");
    let revision =
        RevisionId::new("rev-post-delete").expect("static fixture ID satisfies canonical policy");
    let writer = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(4)?)?;
    let mut ledger = Ledger::new();
    for generation in 1..=4 {
        let generation = ManifestGeneration::new(generation);
        let digest = format!("digest-{}", generation.get());
        let _retention_receipt =
            writer.record_sealed_search_corpus(&repo, &revision, generation, &digest)?;
        ledger.record_historically_sealed_search_corpus(&repo, &revision, generation, &digest);
    }
    let pair_dir = writer.search_corpus_pair_dir(&repo, &revision);
    drop(writer);

    let sync = Arc::new(FailNthSyncForParent {
        target_parent: pair_dir,
        matching_calls: AtomicUsize::new(0),
        // The first pair sync revalidates the existing generation record;
        // the second is the post-delete durability barrier.
        fail_at_matching_call: 1,
    });
    let store = AuxiliaryAuthorityStore::open_with_parent_sync(
        dir.path(),
        search_corpus_retention(2)?,
        SearchCorpusPairMutationCoordinator::shared(),
        Arc::new(crate::readiness::auxiliary_store::NoActiveSearchCorpusPinsV1),
        Arc::new(crate::readiness::ScriptedIndexBytesV1),
        sync.clone(),
    )?;
    ledger.fence_search_corpus_history_v1(&repo, &revision);
    let first =
        store.record_sealed_search_corpus(&repo, &revision, ManifestGeneration::new(4), "digest-4");
    assert!(matches!(first, Err(CoreError::Storage(_))));
    assert_eq!(
        sync.matching_calls.load(Ordering::SeqCst),
        2,
        "failure must occur at the post-delete pair-directory durability barrier"
    );
    assert!(
        !store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(1))
            .exists()
    );
    assert!(
        !store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(2))
            .exists()
    );
    let fenced = ledger.validate_historically_sealed_track_identity(
        &GenerationSnapshot {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(3),
            manifest_digest: "digest-3".to_string(),
        },
        "post-delete retry",
    );
    assert!(matches!(fenced, Err(CoreError::NotReady(_))));

    let reconciled = store.record_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(4),
        "digest-4",
    )?;
    assert!(
        reconciled.reaped_generations().is_empty(),
        "retry observes files already deleted before the failed fsync"
    );
    ledger.apply_search_corpus_history_retention_receipt_v1(
        &repo,
        &revision,
        ManifestGeneration::new(4),
        &reconciled,
    )?;
    let stale = ledger.validate_historically_sealed_track_identity(
        &GenerationSnapshot {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(1),
            manifest_digest: "digest-1".to_string(),
        },
        "post-delete retry",
    );
    assert!(matches!(stale, Err(CoreError::Typed { .. })));
    ledger.validate_historically_sealed_track_identity(
        &GenerationSnapshot {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(3),
            manifest_digest: "digest-3".to_string(),
        },
        "post-delete retry",
    )?;

    drop(store);
    let reopened = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    let mut restored = Ledger::new();
    reopened.restore_into(&mut restored)?;
    restored.validate_historically_sealed_track_identity(
        &GenerationSnapshot {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: ManifestGeneration::new(3),
            manifest_digest: "digest-3".to_string(),
        },
        "post-delete restart",
    )?;
    let reaped_after_restart = restored.validate_historically_sealed_track_identity(
        &GenerationSnapshot {
            repo_id: repo,
            revision_id: revision,
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: ManifestGeneration::new(2),
            manifest_digest: "digest-2".to_string(),
        },
        "post-delete restart",
    );
    assert!(matches!(reaped_after_restart, Err(CoreError::Typed { .. })));
    Ok(())
}
