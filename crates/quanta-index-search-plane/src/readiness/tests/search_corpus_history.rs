#![expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning durability and CAS tests use assertions as test-failure reporting"
)]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use quanta_index_contract::{
    GenerationSnapshot, IngestOperationKindV1, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneTrackKind,
    SourcePublicationEvent,
};
use quanta_index_core::{
    CoreError, DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticCounterV1,
    DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1,
    DiagnosticRootKindV1, DiagnosticStageV1, DiagnosticTerminalV1, IdempotencyKeyV1,
    RequestBudgetV1, SealedGenerationBytesV1, SourceEventBindingV1,
    SourcePublicationCatalogPort as _,
};
use tempfile::tempdir;

use quanta_index_core::domains::generation::RetentionMeasurementPolicy;

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
use crate::search_corpus_retention::{
    SearchCorpusHistoryRetentionPolicyV1, SearchCorpusIndexBytesPort,
};

#[derive(Debug)]
struct AbsentIndexBytes;

#[derive(Debug)]
struct FixedDiagnosticClock;

impl DiagnosticClockV1 for FixedDiagnosticClock {
    fn now_ns(&self) -> u64 {
        1
    }
}

#[derive(Debug, Default)]
struct RecordingDiagnosticSink(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for RecordingDiagnosticSink {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        self.0.lock().expect("diagnostic sink lock").push(event);
        true
    }
}

#[derive(Debug)]
struct PhysicalIndexBytes(Arc<AtomicU64>);

#[derive(Debug)]
struct SingleCandidateIndexBytes {
    generation: ManifestGeneration,
    measurements: Arc<AtomicUsize>,
}

#[derive(Debug)]
struct CountingRestoreIndexBytes {
    measured_sets: Arc<Mutex<Vec<BTreeSet<ManifestGeneration>>>>,
}

impl SearchCorpusIndexBytesPort for CountingRestoreIndexBytes {
    fn measurement_policy(&self) -> RetentionMeasurementPolicy {
        RetentionMeasurementPolicy::DEFAULT
    }

    fn measure_index_bytes(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        self.measured_sets
            .lock()
            .map_err(|error| CoreError::Storage(format!("restore measurement lock: {error}")))?
            .push(generations.clone());
        let count = u64::try_from(generations.len())
            .map_err(|error| CoreError::InvalidContract(format!("generation count: {error}")))?;
        let bytes = TEST_INDEX_BYTES_PER_GENERATION
            .checked_mul(count)
            .ok_or_else(|| CoreError::InvalidContract("restore bytes overflow".into()))?;
        Ok(SealedGenerationBytesV1 {
            bytes,
            absent: BTreeSet::new(),
        })
    }

    fn measure_state_root_index_bytes(
        &self,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<Option<u64>, CoreError> {
        Ok(Some(3 * TEST_INDEX_BYTES_PER_GENERATION))
    }
}

impl SearchCorpusIndexBytesPort for SingleCandidateIndexBytes {
    fn measurement_policy(&self) -> RetentionMeasurementPolicy {
        RetentionMeasurementPolicy::DEFAULT
    }

    fn measure_index_bytes(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        if *generations != BTreeSet::from([self.generation]) {
            return Err(CoreError::InvalidContract(
                "single-candidate fixture measured an unexpected retained set".into(),
            ));
        }
        let _previous = self.measurements.fetch_add(1, Ordering::SeqCst);
        Ok(SealedGenerationBytesV1 {
            bytes: 10,
            absent: BTreeSet::new(),
        })
    }

    fn measure_state_root_index_bytes(
        &self,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<Option<u64>, CoreError> {
        Ok(Some(10))
    }
}

impl SearchCorpusIndexBytesPort for PhysicalIndexBytes {
    fn measurement_policy(&self) -> RetentionMeasurementPolicy {
        RetentionMeasurementPolicy::DEFAULT
    }

    fn measure_index_bytes(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        _generations: &BTreeSet<ManifestGeneration>,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        Ok(SealedGenerationBytesV1 {
            bytes: 10,
            absent: BTreeSet::new(),
        })
    }

    fn measure_state_root_index_bytes(
        &self,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<Option<u64>, CoreError> {
        Ok(Some(self.0.load(Ordering::SeqCst)))
    }
}

struct EmptyGenerationFixture;
impl quanta_index_core::GenerationNamespaceReadPort for EmptyGenerationFixture {
    fn generation_namespace_is_empty(
        &self,
        _repo: &RepoId,
        _revision: &RevisionId,
    ) -> Result<bool, CoreError> {
        Ok(true)
    }
}

fn configure_empty_generation_fixture(catalog: &crate::ActivationCatalog) -> Result<(), CoreError> {
    catalog.configure_generation_namespace_reads_v1(
        Arc::new(EmptyGenerationFixture),
        Arc::new(EmptyGenerationFixture),
    )
}

impl SearchCorpusIndexBytesPort for AbsentIndexBytes {
    fn measurement_policy(&self) -> RetentionMeasurementPolicy {
        RetentionMeasurementPolicy::DEFAULT
    }

    fn measure_index_bytes(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        Ok(SealedGenerationBytesV1 {
            bytes: 0,
            absent: generations.clone(),
        })
    }
}

#[derive(Debug)]
struct PresenceIndexBytes(Arc<AtomicBool>);

impl SearchCorpusIndexBytesPort for PresenceIndexBytes {
    fn measurement_policy(&self) -> RetentionMeasurementPolicy {
        RetentionMeasurementPolicy::DEFAULT
    }

    fn measure_index_bytes(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        Ok(SealedGenerationBytesV1 {
            bytes: 0,
            absent: if self.0.load(Ordering::SeqCst) {
                BTreeSet::new()
            } else {
                generations.clone()
            },
        })
    }
}

#[derive(Debug)]
struct MissingGenerationIndexBytes(Arc<AtomicU64>);

impl SearchCorpusIndexBytesPort for MissingGenerationIndexBytes {
    fn measurement_policy(&self) -> RetentionMeasurementPolicy {
        RetentionMeasurementPolicy::DEFAULT
    }

    fn measure_index_bytes(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        let missing = self.0.load(Ordering::SeqCst);
        Ok(SealedGenerationBytesV1 {
            bytes: 0,
            absent: generations
                .iter()
                .copied()
                .filter(|generation| generation.get() == missing)
                .collect(),
        })
    }
}

#[derive(Clone, Copy, Debug)]
enum RestoreMeasurementFault {
    Sidecar,
    Identity,
    UnsupportedFormat,
    Storage,
}

#[derive(Debug)]
struct FaultedRestoreIndexBytes {
    fault: RestoreMeasurementFault,
    physical: Option<u64>,
}

impl SearchCorpusIndexBytesPort for FaultedRestoreIndexBytes {
    fn measurement_policy(&self) -> RetentionMeasurementPolicy {
        RetentionMeasurementPolicy::DEFAULT
    }

    fn measure_index_bytes(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        _generations: &BTreeSet<ManifestGeneration>,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        let code = match self.fault {
            RestoreMeasurementFault::Sidecar => {
                quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt
            }
            RestoreMeasurementFault::Identity => {
                quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch
            }
            RestoreMeasurementFault::UnsupportedFormat => {
                quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported
            }
            RestoreMeasurementFault::Storage => {
                return Err(CoreError::Storage("injected physical read failure".into()));
            }
        };
        Err(CoreError::Typed {
            code,
            message: "injected historical content mismatch".into(),
        })
    }

    fn measure_state_root_index_bytes(
        &self,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<Option<u64>, CoreError> {
        Ok(self.physical)
    }
}

fn history_for_faulted_restore(
    fault: RestoreMeasurementFault,
    physical: Option<u64>,
    pair_max: u64,
    total_max: u64,
) -> Result<
    (
        tempfile::TempDir,
        AuxiliaryAuthorityStore,
        RepoId,
        RevisionId,
    ),
    Box<dyn std::error::Error>,
> {
    let directory = tempdir()?;
    let repo = RepoId::new("faulted-restore-repo")?;
    let revision = RevisionId::new("faulted-restore-revision")?;
    let writer = AuxiliaryAuthorityStore::open(directory.path(), search_corpus_retention(2)?)?;
    for number in 1..=2 {
        let _receipt = writer.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(number),
            &format!("digest-{number}"),
        )?;
    }
    drop(writer);
    let reader = AuxiliaryAuthorityStore::open_with_lifecycle_v1(
        directory.path(),
        SearchCorpusHistoryRetentionPolicyV1::new(2, pair_max, 64, total_max)?,
        SearchCorpusPairMutationCoordinator::shared(),
        Arc::new(crate::readiness::auxiliary_store::NoActiveSearchCorpusPinsV1),
        Arc::new(FaultedRestoreIndexBytes { fault, physical }),
    )?;
    Ok((directory, reader, repo, revision))
}

#[test]
fn integrity_fault_restore_preserves_history_under_proven_physical_upper_bound() -> TestResult {
    for fault in [
        RestoreMeasurementFault::Sidecar,
        RestoreMeasurementFault::Identity,
    ] {
        let (_directory, store, repo, revision) =
            history_for_faulted_restore(fault, Some(300), 400, 800)?;
        let mut ledger = Ledger::new();
        store.restore_search_corpus_history_into(&mut ledger)?;
        for number in 1..=2 {
            let generation = ManifestGeneration::new(number);
            assert!(
                store
                    .search_corpus_authority_path(&repo, &revision, generation)
                    .is_file(),
                "content fault reaped historical generation {number}"
            );
            ledger.validate_historically_sealed_track_identity(
                &GenerationSnapshot {
                    repo_id: repo.clone(),
                    revision_id: revision.clone(),
                    track: SearchPlaneTrackKind::Lexical,
                    manifest_generation: generation,
                    manifest_digest: format!("digest-{number}"),
                },
                "integrity-fault restore",
            )?;
        }
        assert!(matches!(
            store.record_sealed_search_corpus(
                &repo,
                &revision,
                ManifestGeneration::new(3),
                "digest-3"
            ),
            Err(CoreError::Typed { .. })
        ));
        assert!(
            !store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(3))
                .exists()
        );
    }
    Ok(())
}

#[test]
fn integrity_fault_restore_refuses_unproved_or_over_limit_physical_bound() -> TestResult {
    for (physical, pair_max, total_max, expected_code) in [
        (
            Some(401),
            400,
            800,
            quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
        ),
        (
            Some(501),
            600,
            500,
            quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
        ),
        (
            None,
            400,
            800,
            quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        ),
    ] {
        let (_directory, store, repo, revision) = history_for_faulted_restore(
            RestoreMeasurementFault::Sidecar,
            physical,
            pair_max,
            total_max,
        )?;
        let result = store.restore_search_corpus_history_into(&mut Ledger::new());
        assert!(
            matches!(&result, Err(CoreError::Typed { code, .. }) if *code == expected_code),
            "invalid physical upper bound was accepted: {result:?}"
        );
        for number in 1..=2 {
            assert!(
                store
                    .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(number))
                    .is_file()
            );
        }
    }
    Ok(())
}

#[test]
fn restore_never_converts_storage_failure_to_content_fallback() -> TestResult {
    let (_directory, store, repo, revision) =
        history_for_faulted_restore(RestoreMeasurementFault::Storage, Some(300), 400, 800)?;
    let result = store.restore_search_corpus_history_into(&mut Ledger::new());
    assert!(matches!(&result, Err(CoreError::Storage(_))), "{result:?}");
    for number in 1..=2 {
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(number))
                .is_file()
        );
    }
    Ok(())
}

#[test]
fn restore_does_not_convert_unsupported_persisted_format_to_content_fallback() -> TestResult {
    let (_directory, store, repo, revision) = history_for_faulted_restore(
        RestoreMeasurementFault::UnsupportedFormat,
        Some(300),
        400,
        800,
    )?;
    let result = store.restore_search_corpus_history_into(&mut Ledger::new());
    assert!(matches!(
        &result,
        Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
            ..
        })
    ));
    for number in 1..=2 {
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(number))
                .is_file(),
            "unsupported format erased historical generation {number}"
        );
    }
    Ok(())
}

#[test]
fn quarantine_root_lock_excludes_durable_admission_until_physical_absence() -> TestResult {
    let dir = tempdir()?;
    let present = Arc::new(AtomicBool::new(true));
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        search_corpus_retention(2)?,
        Arc::new(PresenceIndexBytes(Arc::clone(&present))),
    )?;
    let store = owner.authority_store();
    let repo = RepoId::new("root-lock-repo")?;
    let revision = RevisionId::new("revision")?;
    let generation = ManifestGeneration::new(7);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();

    thread::scope(|scope| -> TestResult {
        let removing_store = Arc::clone(&store);
        let removing_present = Arc::clone(&present);
        let removal = scope.spawn(move || {
            removing_store.with_record_keys_during_quarantine(|keys| {
                if !keys.is_empty() {
                    return Err(CoreError::Storage(
                        "fresh quarantine fixture unexpectedly has durable records".into(),
                    ));
                }
                entered_tx
                    .send(())
                    .map_err(|error| CoreError::Storage(error.to_string()))?;
                release_rx
                    .recv()
                    .map_err(|error| CoreError::Storage(error.to_string()))?;
                removing_present.store(false, Ordering::SeqCst);
                Ok(())
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(5))?;
        let admitting_store = Arc::clone(&store);
        let admission = scope.spawn(move || {
            let _started = started_tx.send(());
            let result = admitting_store.record_sealed_search_corpus(
                &repo,
                &revision,
                generation,
                "digest-7",
            );
            let refused_absent = matches!(
                result,
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationConflict,
                    ..
                })
            );
            let _sent = result_tx.send(refused_absent);
        });
        started_rx.recv_timeout(Duration::from_secs(5))?;
        let premature = result_rx.recv_timeout(Duration::from_millis(100));
        release_tx.send(())?;
        removal
            .join()
            .map_err(|panic| format!("removal thread panicked: {panic:?}"))??;
        admission
            .join()
            .map_err(|panic| format!("admission thread panicked: {panic:?}"))?;
        if !matches!(premature, Err(mpsc::RecvTimeoutError::Timeout)) {
            return Err(format!(
                "admission ran before the quarantine root lock settled: {premature:?}"
            )
            .into());
        }
        if !result_rx.recv_timeout(Duration::from_secs(5))? {
            return Err("a physically absent generation acquired durable authority".into());
        }
        if store
            .search_corpus_authority_path(
                &RepoId::new("root-lock-repo")?,
                &RevisionId::new("revision")?,
                generation,
            )
            .exists()
        {
            return Err("rejected admission persisted an authority record".into());
        }
        Ok(())
    })
}

#[test]
fn absent_physical_candidate_cannot_acquire_a_durable_record() -> TestResult {
    let dir = tempdir()?;
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        search_corpus_retention(2)?,
        Arc::new(AbsentIndexBytes),
    )?;
    let store = owner.authority_store();
    let repo = RepoId::new("absent-candidate")?;
    let revision = RevisionId::new("revision")?;
    let generation = ManifestGeneration::new(7);
    let result = store.record_sealed_search_corpus(&repo, &revision, generation, "digest-7");
    assert!(matches!(
        result,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationConflict,
            ..
        })
    ));
    assert!(
        !store
            .search_corpus_authority_path(&repo, &revision, generation)
            .exists()
    );
    Ok(())
}

#[test]
fn first_candidate_is_measured_once_before_durable_admission() -> TestResult {
    let dir = tempdir()?;
    let generation = ManifestGeneration::new(7);
    let measurements = Arc::new(AtomicUsize::new(0));
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        search_corpus_retention(2)?,
        Arc::new(SingleCandidateIndexBytes {
            generation,
            measurements: Arc::clone(&measurements),
        }),
    )?;
    let store = owner.authority_store();
    let repo = RepoId::new("single-candidate")?;
    let revision = RevisionId::new("revision")?;
    let _receipt = store.record_sealed_search_corpus(&repo, &revision, generation, "digest-7")?;
    assert_eq!(measurements.load(Ordering::SeqCst), 1);
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, generation)
            .is_file()
    );
    Ok(())
}

#[test]
fn restore_reuses_complete_single_candidate_measurement() -> TestResult {
    let dir = tempdir()?;
    let generation = ManifestGeneration::new(7);
    let writer = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    let repo = RepoId::new("restore-single-candidate")?;
    let revision = RevisionId::new("revision")?;
    let _receipt = writer.record_sealed_search_corpus(&repo, &revision, generation, "digest-7")?;
    drop(writer);

    let measurements = Arc::new(AtomicUsize::new(0));
    let reader = AuxiliaryAuthorityStore::open_with_lifecycle_v1(
        dir.path(),
        search_corpus_retention(2)?,
        SearchCorpusPairMutationCoordinator::shared(),
        Arc::new(crate::readiness::auxiliary_store::NoActiveSearchCorpusPinsV1),
        Arc::new(SingleCandidateIndexBytes {
            generation,
            measurements: Arc::clone(&measurements),
        }),
    )?;
    let mut restored = Ledger::new();
    reader.restore_search_corpus_history_into(&mut restored)?;
    assert_eq!(measurements.load(Ordering::SeqCst), 1);
    restored.validate_historically_sealed_track_identity(
        &GenerationSnapshot {
            repo_id: repo,
            revision_id: revision,
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: generation,
            manifest_digest: "digest-7".to_string(),
        },
        "restore fast path",
    )?;
    Ok(())
}

#[test]
fn restore_over_pair_count_limit_still_plans_and_reaps() -> TestResult {
    let dir = tempdir()?;
    let repo = RepoId::new("restore-over-pair-count")?;
    let revision = RevisionId::new("revision")?;
    let writer = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(3)?)?;
    for number in 1..=3 {
        let _receipt = writer.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(number),
            &format!("digest-{number}"),
        )?;
    }
    drop(writer);

    let measured_sets = Arc::new(Mutex::new(Vec::new()));
    let reader = AuxiliaryAuthorityStore::open_with_lifecycle_v1(
        dir.path(),
        search_corpus_retention(2)?,
        SearchCorpusPairMutationCoordinator::shared(),
        Arc::new(crate::readiness::auxiliary_store::NoActiveSearchCorpusPinsV1),
        Arc::new(CountingRestoreIndexBytes {
            measured_sets: Arc::clone(&measured_sets),
        }),
    )?;
    let mut restored = Ledger::new();
    reader.restore_search_corpus_history_into(&mut restored)?;
    assert_eq!(
        *measured_sets.lock().map_err(|error| error.to_string())?,
        vec![
            BTreeSet::from([
                ManifestGeneration::new(1),
                ManifestGeneration::new(2),
                ManifestGeneration::new(3),
            ]),
            BTreeSet::from([ManifestGeneration::new(2), ManifestGeneration::new(3)]),
        ],
        "over-limit restore must measure all before planning the retained subset"
    );
    assert!(
        !reader
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(1))
            .exists()
    );
    for number in 2..=3 {
        restored.validate_historically_sealed_track_identity(
            &GenerationSnapshot {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(number),
                manifest_digest: format!("digest-{number}"),
            },
            "over-limit restore",
        )?;
    }
    Ok(())
}

#[test]
fn absent_retained_generation_cannot_displace_a_healthy_rollback_record() -> TestResult {
    let dir = tempdir()?;
    let missing = Arc::new(AtomicU64::new(0));
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        search_corpus_retention(2)?,
        Arc::new(MissingGenerationIndexBytes(Arc::clone(&missing))),
    )?;
    let store = owner.authority_store();
    let repo = RepoId::new("absent-retained")?;
    let revision = RevisionId::new("revision")?;
    for number in 1..=2 {
        let _receipt = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(number),
            &format!("digest-{number}"),
        )?;
    }

    // Quarantine removed one track of g2, but the durable history still
    // names g2. A g3 admission must not let that ghost consume the predecessor
    // slot and reap the physically present g1 authority.
    missing.store(2, Ordering::SeqCst);
    let refused =
        store.record_sealed_search_corpus(&repo, &revision, ManifestGeneration::new(3), "digest-3");
    assert!(matches!(
        refused,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationConflict,
            ..
        })
    ));
    for number in 1..=2 {
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(number))
                .is_file(),
            "refused admission reaped historical generation {number}"
        );
    }
    assert!(
        !store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(3))
            .exists(),
        "refused admission persisted g3"
    );
    let retry =
        store.record_sealed_search_corpus(&repo, &revision, ManifestGeneration::new(2), "digest-2");
    assert!(matches!(
        retry,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationConflict,
            ..
        })
    ));
    let mut restored = Ledger::new();
    store.restore_search_corpus_history_into(&mut restored)?;
    for number in 1..=2 {
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(number))
                .is_file(),
            "bootstrap reaped historical generation {number} while another track was missing"
        );
        restored.validate_historically_sealed_track_identity(
            &GenerationSnapshot {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(number),
                manifest_digest: format!("digest-{number}"),
            },
            "unreconciled restore",
        )?;
    }
    Ok(())
}

fn pending_source_binding(
    repo: &RepoId,
    revision: &RevisionId,
    generation: u64,
) -> SourceEventBindingV1 {
    SourceEventBindingV1 {
        event: SourcePublicationEvent {
            stream_id: "pending-stream".into(),
            event_id: format!("event-{generation}"),
            expected_base_event_id: None,
            payload_sha256: [1; 32],
        },
        target: GenerationSnapshot {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: format!("digest-{generation}"),
        },
        journal_key: IdempotencyKeyV1 {
            kind: IngestOperationKindV1::SearchCorpus,
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation: ManifestGeneration::new(generation),
            batch_digest: "a".repeat(64),
        },
    }
}

#[test]
fn unresolved_source_target_blocks_later_seal_even_if_retained() -> TestResult {
    let dir = tempdir()?;
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        search_corpus_retention(2)?,
        Arc::new(crate::readiness::ScriptedIndexBytesV1),
    )?;
    let store = owner.authority_store();
    let catalog = owner.activation_catalog();
    let repo = RepoId::new("source-pending")?;
    configure_empty_generation_fixture(&catalog)?;
    let revision = RevisionId::new("revision")?;
    let pending = pending_source_binding(&repo, &revision, 1);
    let _reserved = catalog.reserve_source_event(&pending)?;
    let _receipt = store.record_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(1),
        "digest-1",
    )?;
    let rejected =
        store.record_sealed_search_corpus(&repo, &revision, ManifestGeneration::new(2), "digest-2");
    assert!(matches!(rejected, Err(CoreError::NotReady(_))));
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(1))
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
fn unresolved_source_without_authority_blocks_other_stream_retention() -> TestResult {
    let dir = tempdir()?;
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        search_corpus_retention(2)?,
        Arc::new(crate::readiness::ScriptedIndexBytesV1),
    )?;
    let store = owner.authority_store();
    let catalog = owner.activation_catalog();
    let repo = RepoId::new("source-unrecorded")?;
    configure_empty_generation_fixture(&catalog)?;
    let revision = RevisionId::new("revision")?;
    let pending = pending_source_binding(&repo, &revision, 1);
    let _reserved = catalog.reserve_source_event(&pending)?;
    let result =
        store.record_sealed_search_corpus(&repo, &revision, ManifestGeneration::new(2), "digest-2");
    assert!(matches!(result, Err(CoreError::NotReady(_))));
    assert!(
        !store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(2))
            .exists()
    );
    Ok(())
}

#[test]
fn newer_unrecorded_source_does_not_block_older_original_recovery() -> TestResult {
    let dir = tempdir()?;
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        search_corpus_retention(2)?,
        Arc::new(crate::readiness::ScriptedIndexBytesV1),
    )?;
    let store = owner.authority_store();
    let catalog = owner.activation_catalog();
    let repo = RepoId::new("source-older-recovery")?;
    configure_empty_generation_fixture(&catalog)?;
    let revision = RevisionId::new("revision")?;
    let newer = pending_source_binding(&repo, &revision, 2);
    let mut older = pending_source_binding(&repo, &revision, 1);
    older.event.stream_id = "earlier-stream".into();
    let _older = catalog.reserve_source_event(&older)?;
    let _newer = catalog.reserve_source_event(&newer)?;
    let _receipt = store.record_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(1),
        "digest-1",
    )?;
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(1))
            .is_file()
    );
    Ok(())
}

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
fn retention_trace_counts_measurements_and_completed_reaps() -> TestResult {
    let dir = tempdir()?;
    let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
    let repo = RepoId::new("trace-retention")?;
    let revision = RevisionId::new("revision")?;
    let sink = Arc::new(RecordingDiagnosticSink::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([2; 16]),
        Arc::new(FixedDiagnosticClock),
        sink.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
    let child = context.child_context(&root);
    for generation in 1..=3 {
        let _receipt = store.record_sealed_search_corpus_observed(
            &repo,
            &revision,
            ManifestGeneration::new(generation),
            &format!("digest-{generation}"),
            Some(&child),
        )?;
    }
    root.finish(DiagnosticTerminalV1::Ok)?;

    let events = sink.0.lock().expect("diagnostic sink lock").clone();
    let count_stage = |wanted| {
        events
            .iter()
            .filter(|event| {
                matches!(event.kind, DiagnosticEventKindV1::SpanBegin { stage, .. } if stage == wanted)
            })
            .count()
    };
    for stage in [
        DiagnosticStageV1::RetentionPlan,
        DiagnosticStageV1::RetentionMeasure,
        DiagnosticStageV1::RetentionWrite,
        DiagnosticStageV1::RetentionReap,
    ] {
        assert_eq!(count_stage(stage), 3, "missing owner stage: {stage:?}");
    }
    let counters: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let DiagnosticEventKindV1::Counter { key, value, .. } = event.kind {
                return Some((key, value));
            }
            None
        })
        .collect();
    assert!(
        counters
            .iter()
            .any(|(key, value)| { *key == DiagnosticCounterV1::MeasureCalls && *value > 0 })
    );
    assert_eq!(
        counters
            .iter()
            .filter_map(|(key, value)| {
                (*key == DiagnosticCounterV1::RemovedRoots).then_some(*value)
            })
            .collect::<Vec<_>>(),
        vec![0, 0, 1]
    );
    assert_eq!(
        counters
            .iter()
            .filter_map(|(key, value)| {
                (*key == DiagnosticCounterV1::RetainedPairs).then_some(*value)
            })
            .collect::<Vec<_>>(),
        vec![1, 1, 1]
    );
    Ok(())
}

#[test]
fn retention_trace_keeps_failed_measurement_before_durable_write() -> TestResult {
    let dir = tempdir()?;
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        search_corpus_retention(2)?,
        Arc::new(AbsentIndexBytes),
    )?;
    let store = owner.authority_store();
    let repo = RepoId::new("trace-absent")?;
    let revision = RevisionId::new("revision")?;
    let sink = Arc::new(RecordingDiagnosticSink::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([3; 16]),
        Arc::new(FixedDiagnosticClock),
        sink.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
    let child = context.child_context(&root);
    let refused = store.record_sealed_search_corpus_observed(
        &repo,
        &revision,
        ManifestGeneration::new(1),
        "digest-1",
        Some(&child),
    );
    assert!(matches!(
        refused,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationConflict,
            ..
        })
    ));
    root.finish(DiagnosticTerminalV1::Error { code: 1 })?;

    let events = sink.0.lock().expect("diagnostic sink lock").clone();
    let plan = events.iter().find_map(|event| {
        if let DiagnosticEventKindV1::SpanBegin {
            span_id,
            stage: DiagnosticStageV1::RetentionPlan,
            ..
        } = event.kind
        {
            return Some(span_id);
        }
        None
    });
    let plan = plan.expect("retention plan began");
    assert!(events.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::Counter {
            span_id,
            key: DiagnosticCounterV1::MeasureCalls,
            value: 1,
        } if span_id == plan
    )));
    assert!(events.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::SpanEnd {
            span_id,
            outcome: DiagnosticTerminalV1::Error { code: 1 },
        } if span_id == plan
    )));
    assert!(!events.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::SpanBegin {
            stage: DiagnosticStageV1::RetentionWrite,
            ..
        }
    )));
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
fn physical_root_budget_counts_pending_garbage_until_collection() -> TestResult {
    let dir = tempdir()?;
    let physical_bytes = Arc::new(AtomicU64::new(101));
    let owner = SearchCorpusLifecycleOwner::open(
        dir.path(),
        SearchCorpusHistoryRetentionPolicyV1::new(2, 100, 8, 100)?,
        Arc::new(PhysicalIndexBytes(Arc::clone(&physical_bytes))),
    )?;
    let store = owner.authority_store();
    let repo = RepoId::new("physical-root-budget")?;
    let revision = RevisionId::new("revision")?;
    let generation = ManifestGeneration::new(1);
    let rejected = store.record_sealed_search_corpus(&repo, &revision, generation, "digest");
    assert!(matches!(
        rejected,
        Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
            ..
        })
    ));
    assert!(!store.search_corpus_pair_dir(&repo, &revision).exists());

    // A retirement plan cannot free physical budget. Only the storage owner's
    // post-collection observation permits the exact same publication to retry.
    physical_bytes.store(100, Ordering::SeqCst);
    let _receipt = store.record_sealed_search_corpus(&repo, &revision, generation, "digest")?;
    assert!(
        store
            .search_corpus_authority_path(&repo, &revision, generation)
            .is_file()
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

fn assert_candidate_absent_nofollow_v1(path: &std::path::Path) -> TestResult {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!("candidate unexpectedly exists: {}", path.display()).into()),
        Err(error) => Err(error.into()),
    }
}

#[derive(Debug)]
enum AdmissionMeasurementFaultV1 {
    Success,
    BusyOnce,
    Quota,
    Storage,
    Cancel,
    BusyForever,
    WaitForOtherPair {
        first_thread: thread::ThreadId,
        trigger: Mutex<Option<mpsc::Sender<()>>>,
        other_done: Arc<AtomicBool>,
    },
}

#[derive(Debug)]
struct AdmissionRetryIndexBytesV1 {
    policy: RetentionMeasurementPolicy,
    fault: AdmissionMeasurementFaultV1,
    pair_probes: AtomicUsize,
    root_probes: AtomicUsize,
    original_budget: Mutex<Option<RequestBudgetV1>>,
    candidate_path: Mutex<Option<std::path::PathBuf>>,
}

impl AdmissionRetryIndexBytesV1 {
    fn new(fault: AdmissionMeasurementFaultV1, duration: Duration) -> Result<Self, CoreError> {
        Ok(Self {
            policy: RetentionMeasurementPolicy::DEFAULT
                .with_max_physical_scan_duration(duration)?,
            fault,
            pair_probes: AtomicUsize::new(0),
            root_probes: AtomicUsize::new(0),
            original_budget: Mutex::new(None),
            candidate_path: Mutex::new(None),
        })
    }

    fn bind_candidate_path(&self, path: std::path::PathBuf) -> Result<(), CoreError> {
        let mut candidate = self.candidate_path.lock().map_err(|error| {
            CoreError::Storage(format!("retry fixture candidate path lock: {error}"))
        })?;
        *candidate = Some(path);
        drop(candidate);
        Ok(())
    }

    fn assert_bound_candidate_not_durable(&self) -> Result<(), CoreError> {
        let path = self
            .candidate_path
            .lock()
            .map_err(|error| {
                CoreError::Storage(format!("retry fixture candidate path lock: {error}"))
            })?
            .clone();
        let Some(path) = path.as_ref() else {
            return Ok(());
        };
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(CoreError::InvalidContract(
                "retry fixture candidate was durable before measurement completed".into(),
            )),
            Err(error) => Err(CoreError::Storage(format!(
                "retry fixture inspect candidate {}: {error}",
                path.display()
            ))),
        }
    }
}

impl SearchCorpusIndexBytesPort for AdmissionRetryIndexBytesV1 {
    fn measurement_policy(&self) -> RetentionMeasurementPolicy {
        self.policy
    }

    fn measure_index_bytes(
        &self,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
        _generations: &BTreeSet<ManifestGeneration>,
        budget: &RequestBudgetV1,
        _diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        budget.checkpoint("retry-fixture:pair")?;
        let _previous = self.pair_probes.fetch_add(1, Ordering::SeqCst);
        Ok(SealedGenerationBytesV1 {
            bytes: 10,
            absent: BTreeSet::new(),
        })
    }

    fn measure_state_root_index_bytes(
        &self,
        budget: &RequestBudgetV1,
        _diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<Option<u64>, CoreError> {
        budget.checkpoint("retry-fixture:root")?;
        let check_original = !matches!(
            &self.fault,
            AdmissionMeasurementFaultV1::WaitForOtherPair { first_thread, .. }
                if thread::current().id() != *first_thread
        );
        if check_original {
            let mut original = self.original_budget.lock().map_err(|error| {
                CoreError::Storage(format!("retry fixture original budget lock: {error}"))
            })?;
            if let Some(previous) = original.as_ref() {
                if !previous.same_request_as(budget) || previous.deadline() != budget.deadline() {
                    return Err(CoreError::InvalidContract(
                        "retry fixture received a renewed measurement budget".into(),
                    ));
                }
            } else {
                *original = Some(budget.clone());
            }
        }
        let _previous = self.root_probes.fetch_add(1, Ordering::SeqCst);
        // Every prewrite physical-root probe must still see no candidate.
        self.assert_bound_candidate_not_durable()?;
        let busy = || CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
            message: "injected transient physical measurement contention".into(),
        };
        match &self.fault {
            AdmissionMeasurementFaultV1::Success => Ok(Some(10)),
            AdmissionMeasurementFaultV1::BusyOnce => {
                if self.root_probes.load(Ordering::SeqCst) == 1 {
                    Err(busy())
                } else {
                    Ok(Some(10))
                }
            }
            AdmissionMeasurementFaultV1::Quota => Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
                message: "injected fixed quota refusal".into(),
            }),
            AdmissionMeasurementFaultV1::Storage => {
                Err(CoreError::Storage("injected physical snapshot failure".into()))
            }
            AdmissionMeasurementFaultV1::Cancel => {
                budget.cancel_handle().cancel();
                budget.checkpoint("retry-fixture:cancel")?;
                Err(CoreError::InvalidContract("cancel checkpoint unexpectedly passed".into()))
            }
            AdmissionMeasurementFaultV1::BusyForever => Err(busy()),
            AdmissionMeasurementFaultV1::WaitForOtherPair {
                first_thread,
                trigger,
                other_done,
            } => {
                if thread::current().id() != *first_thread {
                    return Ok(Some(10));
                }
                if self.root_probes.load(Ordering::SeqCst) == 1 {
                    let sender = trigger.lock().map_err(|error| {
                        CoreError::Storage(format!("retry fixture trigger lock: {error}"))
                    })?.take().ok_or_else(|| {
                        CoreError::InvalidContract("retry fixture trigger was already sent".into())
                    })?;
                    sender.send(()).map_err(|error| {
                        CoreError::Storage(format!("retry fixture trigger send: {error}"))
                    })?;
                    return Err(busy());
                }
                if other_done.load(Ordering::SeqCst) {
                    Ok(Some(10))
                } else {
                    Err(busy())
                }
            }
        }
    }
}

fn open_retry_authority_v1(
    root: &std::path::Path,
    bytes: Arc<AdmissionRetryIndexBytesV1>,
) -> Result<Arc<AuxiliaryAuthorityStore>, CoreError> {
    open_retry_authority_with_policy_v1(root, bytes, search_corpus_retention(2)?)
}

fn open_retry_authority_with_policy_v1(
    root: &std::path::Path,
    bytes: Arc<AdmissionRetryIndexBytesV1>,
    policy: SearchCorpusHistoryRetentionPolicyV1,
) -> Result<Arc<AuxiliaryAuthorityStore>, CoreError> {
    Ok(Arc::new(AuxiliaryAuthorityStore::open_with_lifecycle_v1(
        root,
        policy,
        SearchCorpusPairMutationCoordinator::shared(),
        Arc::new(crate::readiness::auxiliary_store::NoActiveSearchCorpusPinsV1),
        bytes,
    )?))
}

#[test]
fn transient_measurement_busy_replans_before_durable_record() -> TestResult {
    let directory = tempdir()?;
    let bytes = Arc::new(AdmissionRetryIndexBytesV1::new(
        AdmissionMeasurementFaultV1::BusyOnce,
        Duration::from_secs(2),
    )?);
    let store = open_retry_authority_v1(directory.path(), Arc::clone(&bytes))?;
    let repo = RepoId::new("retry-busy-once")?;
    let revision = RevisionId::new("revision")?;
    let generation = ManifestGeneration::new(1);
    let path = store.search_corpus_authority_path(&repo, &revision, generation);
    bytes.bind_candidate_path(path.clone())?;
    let receipt = store.record_sealed_search_corpus(&repo, &revision, generation, "digest-1")?;
    assert_eq!(bytes.root_probes.load(Ordering::SeqCst), 2);
    assert!(bytes.pair_probes.load(Ordering::SeqCst) >= 2);
    assert!(receipt.retains(generation));
    assert!(std::fs::symlink_metadata(&path)?.is_file());
    assert_eq!(
        std::fs::read_dir(store.search_corpus_pair_dir(&repo, &revision))?.count(),
        1
    );
    let raw: serde_json::Value = quanta_index_ipc::decode_cbor_payload(&std::fs::read(&path)?)?;
    assert_eq!(
        raw,
        serde_json::json!({
            "schema_version": 1,
            "repo_id": "retry-busy-once",
            "revision_id": "revision",
            "generation": 1,
            "manifest_digest": "digest-1"
        })
    );
    Ok(())
}

#[test]
fn existing_record_busy_reconciles_without_replacing_its_inode() -> TestResult {
    use std::os::unix::fs::MetadataExt as _;

    let directory = tempdir()?;
    let first_bytes = Arc::new(AdmissionRetryIndexBytesV1::new(
        AdmissionMeasurementFaultV1::Success,
        Duration::from_secs(2),
    )?);
    let first_store = open_retry_authority_v1(directory.path(), first_bytes)?;
    let repo = RepoId::new("retry-existing-record")?;
    let revision = RevisionId::new("revision")?;
    let generation = ManifestGeneration::new(1);
    let path = first_store.search_corpus_authority_path(&repo, &revision, generation);
    let _first_receipt =
        first_store.record_sealed_search_corpus(&repo, &revision, generation, "digest-1")?;
    let original_bytes = std::fs::read(&path)?;
    let original: serde_json::Value = quanta_index_ipc::decode_cbor_payload(&original_bytes)?;
    assert_eq!(
        original,
        serde_json::json!({
            "schema_version": 1,
            "repo_id": "retry-existing-record",
            "revision_id": "revision",
            "generation": 1,
            "manifest_digest": "digest-1"
        })
    );
    let original_meta = std::fs::symlink_metadata(&path)?;
    let original_inode = (original_meta.dev(), original_meta.ino());
    drop(first_store);

    let retry_bytes = Arc::new(AdmissionRetryIndexBytesV1::new(
        AdmissionMeasurementFaultV1::BusyOnce,
        Duration::from_secs(2),
    )?);
    let store = open_retry_authority_v1(directory.path(), Arc::clone(&retry_bytes))?;
    let receipt = store.record_sealed_search_corpus(&repo, &revision, generation, "digest-1")?;
    assert!(receipt.retains(generation));
    assert_eq!(retry_bytes.root_probes.load(Ordering::SeqCst), 2);
    assert!(retry_bytes.pair_probes.load(Ordering::SeqCst) >= 2);
    assert!(
        retry_bytes
            .original_budget
            .lock()
            .map_err(|error| error.to_string())?
            .as_ref()
            .is_some_and(|budget| budget.interruption().is_none())
    );
    let after_meta = std::fs::symlink_metadata(&path)?;
    assert_eq!((after_meta.dev(), after_meta.ino()), original_inode);
    assert_eq!(std::fs::read(&path)?, original_bytes);
    assert_eq!(
        std::fs::read_dir(store.search_corpus_pair_dir(&repo, &revision))?.count(),
        1
    );

    let conflicting =
        store.record_sealed_search_corpus(&repo, &revision, generation, "digest-conflict");
    assert!(matches!(
        conflicting,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusAuthorityConflict,
            ..
        })
    ));
    assert_eq!(retry_bytes.root_probes.load(Ordering::SeqCst), 2);
    let final_meta = std::fs::symlink_metadata(&path)?;
    assert_eq!((final_meta.dev(), final_meta.ino()), original_inode);
    assert_eq!(std::fs::read(&path)?, original_bytes);
    assert_eq!(
        std::fs::read_dir(store.search_corpus_pair_dir(&repo, &revision))?.count(),
        1
    );
    drop(store);

    let quota_bytes = Arc::new(AdmissionRetryIndexBytesV1::new(
        AdmissionMeasurementFaultV1::Quota,
        Duration::from_secs(2),
    )?);
    let quota_store = open_retry_authority_v1(directory.path(), Arc::clone(&quota_bytes))?;
    let quota_result =
        quota_store.record_sealed_search_corpus(&repo, &revision, generation, "digest-1");
    assert!(matches!(
        quota_result,
        Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
            ..
        })
    ));
    assert_eq!(quota_bytes.root_probes.load(Ordering::SeqCst), 1);
    let quota_meta = std::fs::symlink_metadata(&path)?;
    assert_eq!((quota_meta.dev(), quota_meta.ino()), original_inode);
    assert_eq!(std::fs::read(&path)?, original_bytes);
    assert_eq!(
        std::fs::read_dir(quota_store.search_corpus_pair_dir(&repo, &revision))?.count(),
        1
    );
    Ok(())
}

#[test]
fn fixed_retention_quota_is_not_retried() -> TestResult {
    let directory = tempdir()?;
    let bytes = Arc::new(AdmissionRetryIndexBytesV1::new(
        AdmissionMeasurementFaultV1::Quota,
        Duration::from_secs(2),
    )?);
    let store = open_retry_authority_v1(directory.path(), Arc::clone(&bytes))?;
    let repo = RepoId::new("retry-fixed-quota")?;
    let revision = RevisionId::new("revision")?;
    let generation = ManifestGeneration::new(1);
    let path = store.search_corpus_authority_path(&repo, &revision, generation);
    bytes.bind_candidate_path(path.clone())?;
    let result = store.record_sealed_search_corpus(&repo, &revision, generation, "digest-1");
    assert!(matches!(
        result,
        Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
            ..
        })
    ));
    assert_eq!(bytes.root_probes.load(Ordering::SeqCst), 1);
    assert_candidate_absent_nofollow_v1(&path)?;
    Ok(())
}

#[test]
fn persistent_measurement_busy_uses_one_original_deadline() -> TestResult {
    let directory = tempdir()?;
    let bytes = Arc::new(AdmissionRetryIndexBytesV1::new(
        AdmissionMeasurementFaultV1::BusyForever,
        Duration::from_millis(100),
    )?);
    let store = open_retry_authority_v1(directory.path(), Arc::clone(&bytes))?;
    let repo = RepoId::new("retry-original-deadline")?;
    let revision = RevisionId::new("revision")?;
    let generation = ManifestGeneration::new(1);
    let path = store.search_corpus_authority_path(&repo, &revision, generation);
    bytes.bind_candidate_path(path.clone())?;
    let result = store.record_sealed_search_corpus(&repo, &revision, generation, "digest-1");
    assert!(matches!(
        result,
        Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
            ..
        })
    ));
    let original_expired = bytes
        .original_budget
        .lock()
        .map_err(|error| error.to_string())?
        .as_ref()
        .is_some_and(|budget| budget.interruption().is_some());
    assert!(original_expired);
    assert!(bytes.root_probes.load(Ordering::SeqCst) >= 1);
    assert_candidate_absent_nofollow_v1(&path)?;
    Ok(())
}

#[test]
fn busy_retry_releases_root_guard_before_other_pair_admission() -> TestResult {
    let directory = tempdir()?;
    let (trigger_tx, trigger_rx) = mpsc::channel();
    let other_done = Arc::new(AtomicBool::new(false));
    let bytes = Arc::new(AdmissionRetryIndexBytesV1::new(
        AdmissionMeasurementFaultV1::WaitForOtherPair {
            first_thread: thread::current().id(),
            trigger: Mutex::new(Some(trigger_tx)),
            other_done: Arc::clone(&other_done),
        },
        Duration::from_secs(2),
    )?);
    let store = open_retry_authority_v1(directory.path(), Arc::clone(&bytes))?;
    let worker_store = Arc::clone(&store);
    let worker_done = Arc::clone(&other_done);
    let worker = thread::spawn(move || -> Result<(), CoreError> {
        trigger_rx
            .recv_timeout(Duration::from_secs(3))
            .map_err(|error| {
                CoreError::Storage(format!("retry fixture trigger receive: {error}"))
            })?;
        let repo = RepoId::new("retry-other-pair")
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        let revision = RevisionId::new("revision")
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        let _receipt = worker_store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(1),
            "digest-other",
        )?;
        worker_done.store(true, Ordering::SeqCst);
        Ok(())
    });
    let repo = RepoId::new("retry-first-pair")?;
    let revision = RevisionId::new("revision")?;
    let first = store.record_sealed_search_corpus(
        &repo,
        &revision,
        ManifestGeneration::new(1),
        "digest-first",
    );
    worker
        .join()
        .map_err(|_panic| "retry fixture other-pair worker panicked")??;
    let _receipt = first?;
    assert!(other_done.load(Ordering::SeqCst));
    assert!(bytes.root_probes.load(Ordering::SeqCst) >= 3);
    Ok(())
}

#[test]
fn busy_retry_reloads_root_before_projecting_pair_quota() -> TestResult {
    let directory = tempdir()?;
    let (trigger_tx, trigger_rx) = mpsc::channel();
    let other_done = Arc::new(AtomicBool::new(false));
    let bytes = Arc::new(AdmissionRetryIndexBytesV1::new(
        AdmissionMeasurementFaultV1::WaitForOtherPair {
            first_thread: thread::current().id(),
            trigger: Mutex::new(Some(trigger_tx)),
            other_done: Arc::clone(&other_done),
        },
        Duration::from_secs(5),
    )?);
    let policy = SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 1, 64 * 1024 * 1024)?;
    let store = open_retry_authority_with_policy_v1(directory.path(), Arc::clone(&bytes), policy)?;
    let first_repo = RepoId::new("retry-first-pair-root-gold")?;
    let other_repo = RepoId::new("retry-other-pair-root-gold")?;
    let revision = RevisionId::new("revision")?;
    let generation = ManifestGeneration::new(1);
    let first_path = store.search_corpus_authority_path(&first_repo, &revision, generation);
    let other_path = store.search_corpus_authority_path(&other_repo, &revision, generation);
    bytes.bind_candidate_path(first_path.clone())?;
    let worker_store = Arc::clone(&store);
    let worker_done = Arc::clone(&other_done);
    let worker_repo = other_repo;
    let worker_revision = revision.clone();
    let worker = thread::spawn(move || -> Result<(), CoreError> {
        trigger_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|error| {
                CoreError::Storage(format!("retry fixture trigger receive: {error}"))
            })?;
        let _receipt = worker_store.record_sealed_search_corpus(
            &worker_repo,
            &worker_revision,
            generation,
            "digest-other",
        )?;
        worker_done.store(true, Ordering::SeqCst);
        Ok(())
    });
    let first =
        store.record_sealed_search_corpus(&first_repo, &revision, generation, "digest-first");
    let worker_done_before_first_return = other_done.load(Ordering::SeqCst);
    let first_budget_uninterrupted = bytes
        .original_budget
        .lock()
        .map_err(|error| error.to_string())
        .map(|original| {
            original
                .as_ref()
                .is_some_and(|budget| budget.interruption().is_none())
        });
    worker
        .join()
        .map_err(|_panic| "retry fixture root-gold worker panicked")??;
    assert!(worker_done_before_first_return);
    assert!(first_budget_uninterrupted?);
    assert!(matches!(
        first,
        Err(CoreError::Typed {
            code:
                quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
            message,
        }) if message.contains("state-root admission requires revision_pairs=2")
            && message.contains("max_revision_pairs=1")
    ));
    assert!(other_done.load(Ordering::SeqCst));
    assert_candidate_absent_nofollow_v1(&first_path)?;
    let raw: serde_json::Value =
        quanta_index_ipc::decode_cbor_payload(&std::fs::read(&other_path)?)?;
    assert_eq!(
        raw.get("repo_id").and_then(serde_json::Value::as_str),
        Some("retry-other-pair-root-gold")
    );
    assert_eq!(
        raw.get("manifest_digest")
            .and_then(serde_json::Value::as_str),
        Some("digest-other")
    );
    Ok(())
}

#[test]
fn busy_retry_reloads_same_pair_before_digest_conflict() -> TestResult {
    let directory = tempdir()?;
    let (trigger_tx, trigger_rx) = mpsc::channel();
    let other_done = Arc::new(AtomicBool::new(false));
    let bytes = Arc::new(AdmissionRetryIndexBytesV1::new(
        AdmissionMeasurementFaultV1::WaitForOtherPair {
            first_thread: thread::current().id(),
            trigger: Mutex::new(Some(trigger_tx)),
            other_done: Arc::clone(&other_done),
        },
        Duration::from_secs(5),
    )?);
    let store = open_retry_authority_v1(directory.path(), Arc::clone(&bytes))?;
    let repo = RepoId::new("retry-same-pair-gold")?;
    let revision = RevisionId::new("revision")?;
    let generation = ManifestGeneration::new(1);
    let path = store.search_corpus_authority_path(&repo, &revision, generation);
    let worker_store = Arc::clone(&store);
    let worker_done = Arc::clone(&other_done);
    let worker_repo = repo.clone();
    let worker_revision = revision.clone();
    let worker = thread::spawn(move || -> Result<(), CoreError> {
        trigger_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|error| {
                CoreError::Storage(format!("retry fixture trigger receive: {error}"))
            })?;
        let _receipt = worker_store.record_sealed_search_corpus(
            &worker_repo,
            &worker_revision,
            generation,
            "digest-other",
        )?;
        worker_done.store(true, Ordering::SeqCst);
        Ok(())
    });
    let first = store.record_sealed_search_corpus(&repo, &revision, generation, "digest-first");
    worker
        .join()
        .map_err(|_panic| "retry fixture pair-gold worker panicked")??;
    assert!(matches!(
        first,
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusAuthorityConflict,
            ..
        })
    ));
    assert!(other_done.load(Ordering::SeqCst));
    let raw: serde_json::Value = quanta_index_ipc::decode_cbor_payload(&std::fs::read(&path)?)?;
    assert_eq!(
        raw.get("repo_id").and_then(serde_json::Value::as_str),
        Some("retry-same-pair-gold")
    );
    assert_eq!(
        raw.get("manifest_digest")
            .and_then(serde_json::Value::as_str),
        Some("digest-other")
    );
    assert_eq!(
        std::fs::read_dir(store.search_corpus_pair_dir(&repo, &revision))?.count(),
        1
    );
    Ok(())
}

#[test]
fn storage_and_cancel_measurement_faults_are_not_retried() -> TestResult {
    for (fault, expected_code) in [
        (AdmissionMeasurementFaultV1::Storage, None),
        (
            AdmissionMeasurementFaultV1::Cancel,
            Some(quanta_index_contract::SearchPlaneErrorCodeV2::RequestCancelled),
        ),
    ] {
        let directory = tempdir()?;
        let bytes = Arc::new(AdmissionRetryIndexBytesV1::new(
            fault,
            Duration::from_secs(2),
        )?);
        let store = open_retry_authority_v1(directory.path(), Arc::clone(&bytes))?;
        let repo = RepoId::new("retry-no-storage-cancel")?;
        let revision = RevisionId::new("revision")?;
        let generation = ManifestGeneration::new(1);
        let path = store.search_corpus_authority_path(&repo, &revision, generation);
        bytes.bind_candidate_path(path.clone())?;
        let result = store.record_sealed_search_corpus(&repo, &revision, generation, "digest-1");
        match expected_code {
            None => assert!(matches!(result, Err(CoreError::Storage(_)))),
            Some(code) => assert!(
                matches!(result, Err(CoreError::Typed { code: actual, .. }) if actual == code)
            ),
        }
        assert_eq!(bytes.root_probes.load(Ordering::SeqCst), 1);
        assert_candidate_absent_nofollow_v1(&path)?;
    }
    Ok(())
}

#[derive(Debug)]
struct CancelAfterAuthorityRenameV1 {
    bytes: Arc<AdmissionRetryIndexBytesV1>,
    candidate_path: Mutex<Option<std::path::PathBuf>>,
    cancelled_after_rename: AtomicBool,
}

impl crate::readiness::durable_fs::ParentDirectorySyncPort for CancelAfterAuthorityRenameV1 {
    fn sync_parent(&self, parent: &std::path::Path) -> std::io::Result<()> {
        std::fs::File::open(parent)?.sync_all()?;
        let candidate = self
            .candidate_path
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .clone();
        let Some(candidate) = candidate.as_ref() else {
            return Ok(());
        };
        if candidate.parent() == Some(parent)
            && std::fs::symlink_metadata(candidate).is_ok_and(|metadata| metadata.is_file())
        {
            let cancel = self
                .bytes
                .original_budget
                .lock()
                .map_err(|error| std::io::Error::other(error.to_string()))?
                .as_ref()
                .ok_or_else(|| {
                    std::io::Error::other("authority rename happened before measurement budget")
                })?
                .cancel_handle();
            cancel.cancel();
            self.cancelled_after_rename.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[test]
fn late_cancel_after_durable_authority_rename_does_not_clear_receipt() -> TestResult {
    let directory = tempdir()?;
    let bytes = Arc::new(AdmissionRetryIndexBytesV1::new(
        AdmissionMeasurementFaultV1::Success,
        Duration::from_secs(2),
    )?);
    let parent_sync = Arc::new(CancelAfterAuthorityRenameV1 {
        bytes: Arc::clone(&bytes),
        candidate_path: Mutex::new(None),
        cancelled_after_rename: AtomicBool::new(false),
    });
    let store = AuxiliaryAuthorityStore::open_with_parent_sync(
        directory.path(),
        search_corpus_retention(2)?,
        SearchCorpusPairMutationCoordinator::shared(),
        Arc::new(crate::readiness::auxiliary_store::NoActiveSearchCorpusPinsV1),
        bytes,
        parent_sync.clone(),
    )?;
    let repo = RepoId::new("retry-late-cancel")?;
    let revision = RevisionId::new("revision")?;
    let generation = ManifestGeneration::new(1);
    let path = store.search_corpus_authority_path(&repo, &revision, generation);
    *parent_sync
        .candidate_path
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))? = Some(path.clone());
    let receipt = store.record_sealed_search_corpus(&repo, &revision, generation, "digest-1")?;
    assert!(receipt.retains(generation));
    assert!(parent_sync.cancelled_after_rename.load(Ordering::SeqCst));
    assert!(std::fs::symlink_metadata(path)?.is_file());
    Ok(())
}
