#![expect(
    clippy::panic_in_result_fn,
    reason = "assertions report failures in Result-returning catalog tests"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{
    ActivationCatalog, PreparedSearchCorpusGenerationV1, SearchCorpusGenerationV1,
    repository_envelope,
};
use crate::readiness::durable_fs::ParentDirectorySyncPort;
use crate::search_corpus_lifecycle::SearchCorpusPairMutationCoordinator;
use quanta_index_contract::{
    BatchPublishReceipt, GenerationSnapshot, IngestOperationKindV1, ManifestGeneration, RepoId,
    RepoMapTerminalReceiptV2, RevisionId, SearchPlaneTrackKind, SemanticContentRootsV1,
    SourcePublicationEvent,
};
use quanta_index_core::{
    ClaimOutcomeV1, CoreError, IdempotencyCatalogPort, IdempotencyKeyV1, OperationInspectV1,
    PreparedMutationV1, SourceEventBindingV1, SourceEventPhaseV1, SourceEventReservationV1,
    SourcePublicationCatalogPort,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn binding(
    revision: &str,
    stream: &str,
    event: &str,
    base: Option<&str>,
    generation: u64,
) -> SourceEventBindingV1 {
    let target = GenerationSnapshot {
        repo_id: RepoId::new("repository").expect("fixture repo"),
        revision_id: RevisionId::new(revision).expect("fixture revision"),
        manifest_generation: ManifestGeneration::new(generation),
        manifest_digest: format!("manifest-{generation}"),
        track: SearchPlaneTrackKind::Lexical,
    };
    SourceEventBindingV1 {
        event: SourcePublicationEvent {
            stream_id: stream.into(),
            event_id: event.into(),
            expected_base_event_id: base.map(str::to_string),
            payload_sha256: [3; 32],
        },
        journal_key: IdempotencyKeyV1 {
            kind: IngestOperationKindV1::SearchCorpus,
            repo_id: target.repo_id.clone(),
            revision_id: target.revision_id.clone(),
            generation: target.manifest_generation,
            batch_digest: "a".repeat(64),
        },
        target,
    }
}
fn corpus(binding: &SourceEventBindingV1) -> Result<SearchCorpusGenerationV1, CoreError> {
    let mut semantic = binding.target.clone();
    semantic.track = SearchPlaneTrackKind::Semantic;
    SearchCorpusGenerationV1::new(
        binding.target.clone(),
        semantic,
        SemanticContentRootsV1 {
            row_root_digest: format!("sha256:{}", "a".repeat(64)),
            membership_root_digest: format!("sha256:{}", "b".repeat(64)),
        },
    )
}
fn journal(binding: &SourceEventBindingV1) -> InspectJournal {
    InspectJournal {
        key: binding.journal_key.clone(),
        inspected: OperationInspectV1::Committed {
            durable_sequence: 41,
            receipt: BatchPublishReceipt {
                generation: binding.target.manifest_generation,
                manifest_digest: Some(binding.target.manifest_digest.clone()),
                batch_digest: binding.journal_key.batch_digest.clone(),
                accepted_replace_scopes: 3,
                accepted_tombstone_scopes: 0,
                accepted_semantic_replace_scopes: 2,
                accepted_semantic_tombstone_scopes: 0,
                accepted_clear_surfaces: 0,
                sealed: true,
                applied: true,
                durable_sequence: 0,
                semantic_content: Some(SemanticContentRootsV1 {
                    row_root_digest: format!("sha256:{}", "a".repeat(64)),
                    membership_root_digest: format!("sha256:{}", "b".repeat(64)),
                }),
            },
        },
    }
}
fn activate(catalog: &ActivationCatalog, binding: &SourceEventBindingV1) -> Result<(), CoreError> {
    let guard = catalog
        .lifecycle_coordinator
        .lock_pair(&binding.target.repo_id, &binding.target.revision_id)?;
    let candidate = corpus(binding)?;
    let prepared = PreparedSearchCorpusGenerationV1::new(candidate, None)?;
    let _activation =
        catalog.activate_prepared_under_guard_v1(&guard, &prepared, Some(&binding.event))?;
    Ok(())
}

#[test]
fn source_event_cross_revision_replay_preserves_original_binding_and_cas() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let first = binding("r1", "stream", "event-1", None, 1);
    assert!(matches!(
        catalog.reserve_source_event(&first)?,
        SourceEventReservationV1::Reserved(_)
    ));
    let replay = binding("r2", "stream", "event-1", None, 99);
    let SourceEventReservationV1::Existing(existing) = catalog.reserve_source_event(&replay)?
    else {
        panic!("must return original identity");
    };
    assert_eq!(existing.binding, first);
    assert_eq!(existing.phase, SourceEventPhaseV1::Pending);
    let mut conflict = replay.clone();
    conflict.event.payload_sha256[0] ^= 1;
    assert!(catalog.reserve_source_event(&conflict).is_err());
    assert!(
        catalog
            .reserve_source_event(&binding("r2", "stream", "event-2", None, 2))
            .is_err()
    );
    assert!(
        activate(&catalog, &first).is_err(),
        "pending is not a staged proof"
    );
    let _staged =
        catalog.reconcile_source_event(&first.target.repo_id, &first.event, &journal(&first))?;
    assert!(
        catalog
            .validate_proved_source_history(&first.target, Some(&first.event))
            .is_err(),
        "staging alone does not authorize rollback or restart"
    );
    activate(&catalog, &first)?;
    catalog.validate_proved_source_history(&first.target, Some(&first.event))?;
    assert!(
        catalog
            .reserve_source_event(&binding("r2", "stream", "event-2", None, 2))
            .is_err(),
        "same payload with new event still needs active base CAS"
    );
    let second = binding("r2", "stream", "event-2", Some("event-1"), 2);
    let _reserved = catalog.reserve_source_event(&second)?;
    drop(catalog);
    let reopened = ActivationCatalog::open(dir.path())?;
    assert_eq!(
        reopened
            .inspect_source_event(&first.target.repo_id, &first.event)?
            .expect("retained event")
            .phase,
        SourceEventPhaseV1::Active
    );
    assert_eq!(
        reopened
            .inspect_source_event(&second.target.repo_id, &second.event)?
            .expect("pending event")
            .phase,
        SourceEventPhaseV1::Pending
    );
    assert!(
        reopened
            .active_search_corpus_v1(&first.target.repo_id, &first.target.revision_id)?
            .is_some()
    );
    Ok(())
}

#[test]
fn source_event_reconciliation_rejects_missing_partial_wrong_receipt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let event = binding("r1", "stream", "event", None, 1);
    let _reserved = catalog.reserve_source_event(&event)?;
    let mut probe = journal(&event);
    for inspected in [
        OperationInspectV1::Absent,
        OperationInspectV1::Uncertain {
            owner: "crashed".into(),
        },
        OperationInspectV1::Refused {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest,
            message: "not a nonpublication proof".into(),
            durable_sequence: 7,
        },
    ] {
        probe.inspected = inspected;
        assert!(
            catalog
                .reconcile_source_event(&event.target.repo_id, &event.event, &probe)
                .is_err()
        );
    }
    let mutations: [fn(&mut BatchPublishReceipt, &mut u64); 7] = [
        |receipt, _| receipt.generation = ManifestGeneration::new(9),
        |receipt, _| receipt.manifest_digest = Some("different-manifest".into()),
        |receipt, _| receipt.batch_digest = "b".repeat(64),
        |receipt, _| receipt.sealed = false,
        |receipt, _| receipt.semantic_content = None,
        |receipt, _| {
            receipt
                .semantic_content
                .as_mut()
                .expect("fixture semantic roots")
                .row_root_digest = "malformed-root".into();
        },
        |_, sequence| *sequence = 0,
    ];
    for mutate in mutations {
        probe = journal(&event);
        let OperationInspectV1::Committed {
            receipt,
            durable_sequence,
        } = &mut probe.inspected
        else {
            panic!("committed fixture");
        };
        mutate(receipt, durable_sequence);
        assert!(
            catalog
                .reconcile_source_event(&event.target.repo_id, &event.event, &probe)
                .is_err()
        );
        assert_eq!(
            catalog
                .inspect_source_event(&event.target.repo_id, &event.event)?
                .expect("reservation retained")
                .phase,
            SourceEventPhaseV1::Pending
        );
        assert!(activate(&catalog, &event).is_err());
    }
    let staged =
        catalog.reconcile_source_event(&event.target.repo_id, &event.event, &journal(&event))?;
    assert_eq!(staged.phase, SourceEventPhaseV1::Staged);
    Ok(())
}

#[test]
fn rollback_and_reopen_preserve_source_high_water_above_the_visible_generation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let first = binding("r1", "stream", "event-1", None, 1);
    let second = binding("r1", "stream", "event-2", Some("event-1"), 2);
    let mut expected = None;
    for event in [&first, &second] {
        let _reserved = catalog.reserve_source_event(event)?;
        let _staged =
            catalog.reconcile_source_event(&event.target.repo_id, &event.event, &journal(event))?;
        let guard = catalog
            .lifecycle_coordinator
            .lock_pair(&event.target.repo_id, &event.target.revision_id)?;
        let prepared = PreparedSearchCorpusGenerationV1::new(corpus(event)?, expected.take())?;
        expected = Some(
            catalog
                .activate_prepared_under_guard_v1(&guard, &prepared, Some(&event.event))?
                .active,
        );
    }
    let rolled_back = catalog.rollback(
        &quanta_index_contract::SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: expected.expect("second active head"),
            target: corpus(&first)?.to_contract_v1(),
        },
    )?;
    assert_eq!(
        rolled_back.active.generation,
        corpus(&first)?.to_contract_v1()
    );
    drop(catalog);
    let reopened = ActivationCatalog::open(dir.path())?;
    assert_eq!(
        reopened.active_search_corpus_v1(&first.target.repo_id, &first.target.revision_id)?,
        Some(corpus(&first)?)
    );
    for event in [&first, &second] {
        reopened.validate_proved_source_history(&event.target, Some(&event.event))?;
    }
    let stale = binding("r1", "stream", "event-3", Some("event-1"), 3);
    let path = dir
        .path()
        .join(repository_envelope::file_name(&first.target.repo_id));
    let before = std::fs::read(&path)?;
    assert!(matches!(
        reopened.reserve_source_event(&stale),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::DeltaBaseConflict,
            ..
        })
    ));
    assert_eq!(std::fs::read(&path)?, before);
    let next = binding("r1", "stream", "event-3", Some("event-2"), 3);
    assert!(matches!(
        reopened.reserve_source_event(&next)?,
        SourceEventReservationV1::Reserved(_)
    ));
    Ok(())
}

#[test]
fn concurrent_revision_activations_preserve_both_roots_and_stream_heads() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = Arc::new(ActivationCatalog::open(dir.path())?);
    let events = [
        binding("r1", "s1", "event", None, 1),
        binding("r2", "s2", "event", None, 2),
    ];
    for event in &events {
        let _reserved = catalog.reserve_source_event(event)?;
        let _staged =
            catalog.reconcile_source_event(&event.target.repo_id, &event.event, &journal(event))?;
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = events
            .iter()
            .map(|event| {
                let catalog = Arc::clone(&catalog);
                scope.spawn(move || activate(&catalog, event))
            })
            .collect();
        for handle in handles {
            handle.join().expect("activation worker")?;
        }
        Ok::<_, CoreError>(())
    })?;
    let reopened = ActivationCatalog::open(dir.path())?;
    for event in &events {
        assert_eq!(
            reopened.active_search_corpus_v1(&event.target.repo_id, &event.target.revision_id)?,
            Some(corpus(event)?)
        );
        assert_eq!(
            reopened
                .inspect_source_event(&event.target.repo_id, &event.event)?
                .expect("retained event")
                .phase,
            SourceEventPhaseV1::Active
        );
    }
    assert_eq!(
        std::fs::read_dir(dir.path())?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .count(),
        1
    );
    Ok(())
}

#[derive(Debug)]
struct ToggleSync(AtomicBool);
impl ParentDirectorySyncPort for ToggleSync {
    fn sync_parent(&self, parent: &std::path::Path) -> std::io::Result<()> {
        if self.0.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("injected sync failure"));
        }
        std::fs::File::open(parent)?.sync_all()
    }
}
#[test]
fn activation_after_rename_failure_reopens_matching_root_and_event() -> TestResult {
    let dir = tempfile::tempdir()?;
    let sync = Arc::new(ToggleSync(AtomicBool::new(false)));
    let catalog = ActivationCatalog::open_with_parent_sync(
        dir.path(),
        SearchCorpusPairMutationCoordinator::shared(),
        sync.clone(),
    )?;
    let event = binding("r1", "stream", "event", None, 1);
    let _reserved = catalog.reserve_source_event(&event)?;
    let _staged =
        catalog.reconcile_source_event(&event.target.repo_id, &event.event, &journal(&event))?;
    sync.0.store(true, Ordering::SeqCst);
    assert!(activate(&catalog, &event).is_err());
    assert!(
        catalog
            .inspect_source_event(&event.target.repo_id, &event.event)
            .is_err(),
        "uncertain process must freeze"
    );
    let reopened = ActivationCatalog::open(dir.path())?;
    assert_eq!(
        reopened.active_search_corpus_v1(&event.target.repo_id, &event.target.revision_id)?,
        Some(corpus(&event)?)
    );
    assert_eq!(
        reopened
            .inspect_source_event(&event.target.repo_id, &event.event)?
            .expect("retained event")
            .phase,
        SourceEventPhaseV1::Active
    );
    Ok(())
}

#[test]
fn envelope_rejects_legacy_duplicate_reordered_and_oversized_inputs() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let event = binding("r1", "stream", "event", None, 1);
    let _reserved = catalog.reserve_source_event(&event)?;
    let path = dir
        .path()
        .join(repository_envelope::file_name(&event.target.repo_id));
    let bytes = std::fs::read(&path)?;
    let row: serde_json::Value = serde_json::from_slice(&bytes)?;
    let mut duplicate = row.clone();
    duplicate[4]
        .as_array_mut()
        .expect("events array")
        .push(row[4][0].clone());
    assert!(repository_envelope::decode(&path, &serde_json::to_vec(&duplicate)?).is_err());
    let mut wrong_format = row.clone();
    wrong_format[0] = serde_json::json!(1);
    assert!(repository_envelope::decode(&path, &serde_json::to_vec(&wrong_format)?).is_err());
    assert!(repository_envelope::decode(&path, br#"{"lexical":{},"semantic":{}}"#).is_err());
    let mut overlimit = row;
    overlimit[4] = serde_json::Value::Array(vec![
        duplicate[4][0].clone();
        repository_envelope::MAX_EVENTS + 1
    ]);
    assert!(repository_envelope::decode(&path, &serde_json::to_vec(&overlimit)?).is_err());
    let file = std::fs::OpenOptions::new().write(true).open(&path)?;
    file.set_len(repository_envelope::MAX_ENVELOPE_BYTES as u64 + 1)?;
    assert!(repository_envelope::read_bounded(&path).is_err());
    Ok(())
}

#[test]
fn envelope_rejects_multiple_events_for_one_target_generation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let first = binding("r1", "stream-a", "event", None, 1);
    let second = binding("r1", "stream-b", "event", None, 2);
    let _first = catalog.reserve_source_event(&first)?;
    let path = dir
        .path()
        .join(repository_envelope::file_name(&first.target.repo_id));
    let before = std::fs::read(&path)?;
    let collision = binding("r1", "stream-b", "event", None, 1);
    assert!(catalog.reserve_source_event(&collision).is_err());
    assert_eq!(std::fs::read(&path)?, before);
    let _second = catalog.reserve_source_event(&second)?;
    // Equal generation numbers in distinct containing revisions are valid.
    let other_revision = binding("r2", "stream-c", "event", None, 1);
    let _other_revision = catalog.reserve_source_event(&other_revision)?;
    let bytes = std::fs::read(&path)?;
    let (_, mut state) = repository_envelope::decode(&path, &bytes)?;
    let row: serde_json::Value = serde_json::from_slice(&bytes)?;

    for manifest_digest in [
        &first.target.manifest_digest,
        &second.target.manifest_digest,
    ] {
        let mut target = first.target.clone();
        target.manifest_digest.clone_from(manifest_digest);
        let mut forged = row.clone();
        forged[4][1][1] = serde_json::to_value(target)?;
        let forged_bytes = serde_json::to_vec(&forged)?;
        assert!(repository_envelope::decode(&path, &forged_bytes).is_err());
        // Exercise the production restart path as well as the decoder.
        std::fs::write(&path, &forged_bytes)?;
        assert!(ActivationCatalog::open(dir.path()).is_err());
        assert_eq!(std::fs::read(&path)?, forged_bytes);
    }

    let history = state
        .histories
        .get_mut(&first.target.repo_id)
        .expect("decoded repository history");
    let duplicate = history
        .records
        .get_mut(&(
            second.event.stream_id.clone(),
            second.event.event_id.clone(),
        ))
        .expect("second event");
    duplicate.binding.target = first.target.clone();
    duplicate.binding.journal_key.generation = first.target.manifest_generation;
    assert!(repository_envelope::encode(&first.target.repo_id, &state).is_err());
    Ok(())
}

struct InspectJournal {
    key: IdempotencyKeyV1,
    inspected: OperationInspectV1,
}
fn unexpected<T>() -> Result<T, CoreError> {
    Err(CoreError::Storage(
        "test journal permits only inspection".into(),
    ))
}
impl IdempotencyCatalogPort for InspectJournal {
    fn inspect(&self, key: &IdempotencyKeyV1) -> Result<OperationInspectV1, CoreError> {
        if *key != self.key {
            return unexpected();
        }
        Ok(self.inspected.clone())
    }
    fn prepare(
        &self,
        _: &IdempotencyKeyV1,
        _: &[u8; 32],
        _: &str,
        _: u64,
        _: &[u8; 32],
    ) -> Result<PreparedMutationV1, CoreError> {
        unexpected()
    }
    fn claim_prepared(
        &self,
        _: &IdempotencyKeyV1,
        _: &[u8; 32],
        _: &str,
        _: u64,
        _: &[u8; 32],
    ) -> Result<ClaimOutcomeV1, CoreError> {
        unexpected()
    }
    fn mark_applying(&self, _: &PreparedMutationV1) -> Result<(), CoreError> {
        unexpected()
    }
    fn record_refused(&self, _: &PreparedMutationV1, _: &CoreError) -> Result<u64, CoreError> {
        unexpected()
    }
    fn commit(&self, _: &PreparedMutationV1, _: &BatchPublishReceipt) -> Result<u64, CoreError> {
        unexpected()
    }
    fn commit_repomap(
        &self,
        _: &PreparedMutationV1,
        _: &RepoMapTerminalReceiptV2,
    ) -> Result<u64, CoreError> {
        unexpected()
    }
    fn mark_uncertain(&self, _: &PreparedMutationV1) -> Result<(), CoreError> {
        unexpected()
    }
    fn recover(&self, _: &IdempotencyKeyV1) -> Result<OperationInspectV1, CoreError> {
        unexpected()
    }
    fn generations_for_pair(
        &self,
        _: &RepoId,
        _: &RevisionId,
    ) -> Result<Vec<ManifestGeneration>, CoreError> {
        unexpected()
    }
    fn forget_generation(
        &self,
        _: &RepoId,
        _: &RevisionId,
        _: ManifestGeneration,
    ) -> Result<u64, CoreError> {
        unexpected()
    }
}

#[test]
fn failure_before_replace_preserves_staged_event_and_old_root() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let event = binding("r1", "stream", "event", None, 1);
    let _reserved = catalog.reserve_source_event(&event)?;
    let _staged =
        catalog.reconcile_source_event(&event.target.repo_id, &event.event, &journal(&event))?;
    let path = dir
        .path()
        .join(repository_envelope::file_name(&event.target.repo_id));
    let before = std::fs::read(&path)?;
    let staging = dir.path().join(".staging");
    std::fs::remove_dir(&staging)?;
    std::fs::write(&staging, b"block temporary creation")?;
    assert!(activate(&catalog, &event).is_err());
    assert_eq!(std::fs::read(&path)?, before);
    assert!(
        catalog
            .active_search_corpus_v1(&event.target.repo_id, &event.target.revision_id)?
            .is_none()
    );
    assert_eq!(
        catalog
            .inspect_source_event(&event.target.repo_id, &event.event)?
            .expect("staged event")
            .phase,
        SourceEventPhaseV1::Staged
    );
    std::fs::remove_file(&staging)?;
    std::fs::create_dir(&staging)?;
    let reopened = ActivationCatalog::open(dir.path())?;
    assert_eq!(
        reopened
            .inspect_source_event(&event.target.repo_id, &event.event)?
            .expect("staged event")
            .phase,
        SourceEventPhaseV1::Staged
    );
    Ok(())
}

#[test]
fn stream_capacity_refusal_does_not_evict_identity_or_modify_root() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    for index in 0..repository_envelope::MAX_STREAMS {
        let event = binding(
            "r1",
            &format!("stream-{index:04}"),
            "event",
            None,
            index as u64 + 1,
        );
        let _reserved = catalog.reserve_source_event(&event)?;
    }
    let event = binding("r1", "stream-over-limit", "event", None, 9999);
    let path = dir
        .path()
        .join(repository_envelope::file_name(&event.target.repo_id));
    let before = std::fs::read(&path)?;
    assert!(matches!(
        catalog.reserve_source_event(&event),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        })
    ));
    assert_eq!(std::fs::read(&path)?, before);
    let original = binding("r1", "stream-0000", "event", None, 1);
    assert_eq!(
        catalog
            .inspect_source_event(&original.target.repo_id, &original.event)?
            .expect("identity retained")
            .binding,
        original
    );
    Ok(())
}

#[test]
fn poisoned_publication_writer_fences_serving_and_subsequent_mutations() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = ActivationCatalog::open(dir.path())?;
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _mutation = catalog.mutation.lock().expect("fixture writer lock");
        panic!("simulated publisher unwind while durable outcome is unknown");
    }));
    assert!(caught.is_err());
    assert!(matches!(
        catalog.active_inventory_v1(),
        Err(CoreError::NotReady(_))
    ));
    let pending = binding("rev", "stream", "event", None, 1);
    assert!(matches!(
        catalog.inspect_source_event(&pending.target.repo_id, &pending.event),
        Err(CoreError::NotReady(_))
    ));
    assert!(catalog.reserve_source_event(&pending).is_err());
    Ok(())
}
