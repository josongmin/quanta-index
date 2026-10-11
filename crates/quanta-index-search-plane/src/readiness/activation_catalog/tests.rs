#![expect(
    clippy::panic_in_result_fn,
    reason = "assertions report failures in Result-returning catalog tests"
)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{
    ActivationCatalog, PreparedSearchCorpusGenerationV1, SearchCorpusGenerationV1,
    repository_envelope,
};
use crate::readiness::durable_fs::ParentDirectorySyncPort;
use crate::search_corpus_lifecycle::SearchCorpusPairMutationCoordinator;
use quanta_index_contract::{
    BatchPublishReceipt, GenerationPin, GenerationSnapshot, IngestOperationKindV1,
    ManifestGeneration, RepoId, RepoMapTerminalReceiptV2, RevisionId, SearchPlaneTrackKind,
    SemanticContentRootsV1, SourcePublicationEvent,
};
use quanta_index_core::{
    ClaimOutcomeV1, CoreError, GenerationNamespaceReadPort, IdempotencyCatalogPort,
    IdempotencyKeyV1, OperationInspectV1, PreparedMutationV1, SourceEventAllocationOutcomeV1,
    SourceEventAllocationRequestV1, SourceEventBindingV1, SourceEventPhaseV1,
    SourceEventReservationV1, SourceEventStateV1, SourcePublicationCatalogPort,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Debug)]
struct EmptyFixtureNamespace;

impl GenerationNamespaceReadPort for EmptyFixtureNamespace {
    fn generation_namespace_is_empty(
        &self,
        _repo: &RepoId,
        _revision: &RevisionId,
    ) -> Result<bool, CoreError> {
        Ok(true)
    }
}

#[derive(Debug)]
struct OccupiedFixtureNamespace;

impl GenerationNamespaceReadPort for OccupiedFixtureNamespace {
    fn generation_namespace_is_empty(
        &self,
        _repo: &RepoId,
        _revision: &RevisionId,
    ) -> Result<bool, CoreError> {
        Ok(false)
    }
}

fn configure_catalog(catalog: &ActivationCatalog) -> Result<(), CoreError> {
    catalog.configure_generation_namespace_reads_v1(
        Arc::new(EmptyFixtureNamespace),
        Arc::new(EmptyFixtureNamespace),
    )
}

#[test]
fn lexical_id_reservations_are_original_and_shared_across_sibling_revisions() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
    let first = binding("r1", "stream-a", "event-a", None, 1);
    let sibling = binding("r2", "stream-b", "event-b", None, 1);
    assert!(
        catalog
            .reserve_lexical_document_versions(&first, 3, 7)
            .is_err()
    );
    let _reserved = catalog.reserve_source_event(&first)?;
    let a = catalog.reserve_lexical_document_versions(&first, 3, 7)?;
    assert_eq!((a.file.first, a.file.count), (1, 3));
    assert_eq!((a.native_text.first, a.native_text.count), (1, 7));
    let _reserved = catalog.reserve_source_event(&sibling)?;
    let b = catalog.reserve_lexical_document_versions(&sibling, 2, 5)?;
    assert_eq!((b.file.first, b.native_text.first), (4, 8));
    assert_eq!(a.file.scope_sha256, b.file.scope_sha256);
    assert_ne!(a.file.scope_sha256, a.native_text.scope_sha256);
    assert_eq!(catalog.reserve_lexical_document_versions(&first, 3, 7)?, a);
    assert!(
        catalog
            .reserve_lexical_document_versions(&first, 4, 7)
            .is_err()
    );
    drop(catalog);
    let reopened = open_catalog(dir.path())?;
    assert_eq!(reopened.reserve_lexical_document_versions(&first, 3, 7)?, a);
    assert_eq!(
        reopened.reserve_lexical_document_versions(&sibling, 2, 5)?,
        b
    );
    Ok(())
}

#[test]
fn lexical_id_counter_exhaustion_retains_original_range_and_zero_count_is_valid() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
    let first = binding("r1", "stream-a", "event-a", None, 1);
    let _reserved = catalog.reserve_source_event(&first)?;
    let full = catalog.reserve_lexical_document_versions(&first, u64::MAX, 0)?;
    assert_eq!((full.file.first, full.file.count), (1, u64::MAX));
    assert_eq!((full.native_text.first, full.native_text.count), (0, 0));
    let second = binding("r2", "stream-b", "event-b", None, 1);
    let _reserved = catalog.reserve_source_event(&second)?;
    assert!(
        catalog
            .reserve_lexical_document_versions(&second, 1, 1)
            .is_err()
    );
    assert_eq!(
        catalog.reserve_lexical_document_versions(&first, u64::MAX, 0)?,
        full
    );
    let zero = catalog.reserve_lexical_document_versions(&second, 0, 1)?;
    assert_eq!((zero.file.first, zero.native_text.first), (0, 1));
    Ok(())
}

#[test]
fn lexical_id_envelope_rejects_missing_counter_and_overlapping_original_ranges() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
    let first = binding("r1", "stream-a", "event-a", None, 1);
    let second = binding("r2", "stream-b", "event-b", None, 1);
    for event in [&first, &second] {
        let _reserved = catalog.reserve_source_event(event)?;
        let _ids = catalog.reserve_lexical_document_versions(event, 3, 4)?;
    }
    let path = dir
        .path()
        .join(repository_envelope::file_name(&first.target.repo_id));
    let original = std::fs::read(&path)?;
    let row: serde_json::Value = serde_json::from_slice(&original)?;
    let mut missing = row.clone();
    *missing
        .get_mut(6)
        .ok_or("fixed envelope counter field missing")? = serde_json::json!([]);
    assert!(repository_envelope::decode(&path, &serde_json::to_vec(&missing)?).is_err());
    let mut overlap = row;
    let mut cursor = &mut overlap;
    for index in [4, 1, 5, 0, 0] {
        cursor = cursor
            .get_mut(index)
            .ok_or("fixed envelope range field missing")?;
    }
    *cursor = serde_json::json!(1);
    assert!(repository_envelope::decode(&path, &serde_json::to_vec(&overlap)?).is_err());
    Ok(())
}

fn open_catalog(path: &Path) -> Result<ActivationCatalog, CoreError> {
    let catalog = ActivationCatalog::open(path)?;
    configure_catalog(&catalog)?;
    Ok(catalog)
}

#[test]
fn allocation_requires_configured_empty_pair_and_retains_number_on_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let unconfigured = ActivationCatalog::open(dir.path())?;
    let first = binding("r1", "stream", "event", None, 1);
    let request = SourceEventAllocationRequestV1 {
        repo_id: first.target.repo_id.clone(),
        revision_id: first.target.revision_id.clone(),
        event: first.event.clone(),
    };
    let pin = GenerationPin::new(
        request.repo_id.clone(),
        request.revision_id.clone(),
        ManifestGeneration::new(1),
    );
    assert!(matches!(
        unconfigured.require_reserved_generation(&pin),
        Err(CoreError::NotReady(_))
    ));
    assert!(matches!(
        unconfigured.allocate_source_event(&request),
        Err(CoreError::NotReady(_))
    ));
    unconfigured.configure_generation_namespace_reads_v1(
        Arc::new(EmptyFixtureNamespace),
        Arc::new(OccupiedFixtureNamespace),
    )?;
    assert!(matches!(
        unconfigured.allocate_source_event(&request),
        Err(CoreError::NotReady(_))
    ));
    drop(unconfigured);
    let catalog = open_catalog(dir.path())?;
    let SourceEventAllocationOutcomeV1::Allocated(allocated) =
        catalog.allocate_source_event(&request)?
    else {
        panic!("first allocation");
    };
    assert_eq!(allocated.allocation.generation, ManifestGeneration::new(1));
    assert_eq!(allocated.state, SourceEventStateV1::Allocated);
    catalog.require_reserved_generation(&pin)?;
    assert!(matches!(
        catalog.require_reserved_generation(&GenerationPin::new(
            request.repo_id.clone(),
            request.revision_id.clone(),
            ManifestGeneration::new(2),
        )),
        Err(CoreError::NotReady(_))
    ));
    assert!(matches!(
        catalog.require_reserved_generation(&GenerationPin::new(
            request.repo_id.clone(),
            RevisionId::new("other-revision")?,
            ManifestGeneration::new(1),
        )),
        Err(CoreError::NotReady(_))
    ));
    assert!(matches!(
        catalog.allocate_source_event(&request)?,
        SourceEventAllocationOutcomeV1::Existing(_)
    ));
    let mut changed_revision = request.clone();
    changed_revision.revision_id = RevisionId::new("r2")?;
    assert!(catalog.allocate_source_event(&changed_revision).is_err());
    drop(catalog);
    let reopened = open_catalog(dir.path())?;
    assert_eq!(
        reopened.inspect_source_event(&request.repo_id, &request.event)?,
        Some(allocated)
    );
    reopened.require_reserved_generation(&pin)?;
    assert!(matches!(
        reopened.reserve_source_event(&first)?,
        SourceEventReservationV1::Reserved(_)
    ));
    reopened.require_reserved_generation(&pin)?;
    let mut changed_body = first;
    changed_body.target.manifest_digest.push_str("-different");
    assert!(reopened.reserve_source_event(&changed_body).is_err());
    assert!(matches!(
        reopened.allocate_source_event(&request)?,
        SourceEventAllocationOutcomeV1::Existing(_)
    ));
    Ok(())
}

#[test]
fn explicit_writer_advances_counter_and_rejects_reuse() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
    let first = binding("r1", "stream-a", "event", None, 7);
    assert!(matches!(
        catalog.reserve_source_event(&first)?,
        SourceEventReservationV1::Reserved(_)
    ));
    let collision = binding("r1", "stream-b", "event", None, 7);
    assert!(catalog.reserve_source_event(&collision).is_err());
    let request = SourceEventAllocationRequestV1 {
        repo_id: first.target.repo_id,
        revision_id: first.target.revision_id,
        event: collision.event,
    };
    let SourceEventAllocationOutcomeV1::Allocated(allocated) =
        catalog.allocate_source_event(&request)?
    else {
        panic!("new allocation");
    };
    assert_eq!(allocated.allocation.generation, ManifestGeneration::new(8));
    let path = dir
        .path()
        .join(repository_envelope::file_name(&request.repo_id));
    let before = std::fs::read(&path)?;
    let stale = binding("r1", "stream-b", "event", None, 9);
    assert!(catalog.reserve_source_event(&stale).is_err());
    assert_eq!(std::fs::read(&path)?, before);
    drop(catalog);
    let reopened = open_catalog(dir.path())?;
    let next = binding("r1", "stream-c", "event", None, 9);
    let next_request = SourceEventAllocationRequestV1 {
        repo_id: next.target.repo_id,
        revision_id: next.target.revision_id,
        event: next.event,
    };
    let SourceEventAllocationOutcomeV1::Allocated(ninth) =
        reopened.allocate_source_event(&next_request)?
    else {
        panic!("next allocation after reopen");
    };
    assert_eq!(ninth.allocation.generation, ManifestGeneration::new(9));
    Ok(())
}

#[test]
fn restored_incarnation_cannot_bind_old_unfinished_allocation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
    let first = binding("r1", "stream", "event", None, 1);
    let request = SourceEventAllocationRequestV1 {
        repo_id: first.target.repo_id.clone(),
        revision_id: first.target.revision_id.clone(),
        event: first.event.clone(),
    };
    let _allocated = catalog.allocate_source_event(&request)?;
    let pin = GenerationPin::new(
        request.repo_id.clone(),
        request.revision_id.clone(),
        ManifestGeneration::new(1),
    );
    catalog.require_reserved_generation(&pin)?;
    drop(catalog);
    ActivationCatalog::rotate_root_incarnation_for_restore_v1(dir.path())?;
    let reopened = open_catalog(dir.path())?;
    assert!(matches!(
        reopened.require_reserved_generation(&pin),
        Err(CoreError::NotReady(_))
    ));
    assert!(reopened.reserve_source_event(&first).is_err());
    assert!(matches!(
        reopened.allocate_source_event(&request)?,
        SourceEventAllocationOutcomeV1::Existing(_)
    ));
    Ok(())
}

#[test]
fn allocation_parent_sync_failure_fences_process_and_reopens_number() -> TestResult {
    let dir = tempfile::tempdir()?;
    let sync = Arc::new(ToggleSync(AtomicBool::new(false)));
    let catalog = ActivationCatalog::open_with_parent_sync(
        dir.path(),
        SearchCorpusPairMutationCoordinator::shared(),
        sync.clone(),
    )?;
    configure_catalog(&catalog)?;
    let first = binding("r1", "stream", "event", None, 1);
    let request = SourceEventAllocationRequestV1 {
        repo_id: first.target.repo_id,
        revision_id: first.target.revision_id,
        event: first.event,
    };
    let pin = GenerationPin::new(
        request.repo_id.clone(),
        request.revision_id.clone(),
        ManifestGeneration::new(1),
    );
    sync.0.store(true, Ordering::SeqCst);
    assert!(catalog.allocate_source_event(&request).is_err());
    assert!(
        catalog
            .inspect_source_event(&request.repo_id, &request.event)
            .is_err()
    );
    assert!(catalog.require_reserved_generation(&pin).is_err());
    drop(catalog);
    let reopened = open_catalog(dir.path())?;
    let retained = reopened.inspect_source_event(&request.repo_id, &request.event)?;
    assert_eq!(
        retained
            .expect("persisted allocation")
            .allocation
            .generation,
        ManifestGeneration::new(1)
    );
    reopened.require_reserved_generation(&pin)?;
    Ok(())
}

#[test]
fn concurrent_stream_allocations_have_unique_durable_pair_numbers() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = Arc::new(open_catalog(dir.path())?);
    let requests: Vec<_> = (0..8)
        .map(|index| {
            let fixture = binding("r1", &format!("stream-{index}"), "event", None, 1);
            SourceEventAllocationRequestV1 {
                repo_id: fixture.target.repo_id,
                revision_id: fixture.target.revision_id,
                event: fixture.event,
            }
        })
        .collect();
    let outcomes = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(requests.len());
        for request in &requests {
            let catalog = Arc::clone(&catalog);
            handles.push(scope.spawn(move || catalog.allocate_source_event(request)));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("allocation worker"))
            .collect::<Result<Vec<_>, CoreError>>()
    })?;
    let mut numbers = BTreeSet::new();
    for outcome in &outcomes {
        let SourceEventAllocationOutcomeV1::Allocated(record) = outcome else {
            panic!("distinct event must receive a new allocation");
        };
        assert_eq!(record.state, SourceEventStateV1::Allocated);
        assert!(numbers.insert(record.allocation.generation.get()));
    }
    assert_eq!(numbers, BTreeSet::from([1, 2, 3, 4, 5, 6, 7, 8]));
    drop(catalog);
    let reopened = open_catalog(dir.path())?;
    for (request, outcome) in requests.iter().zip(&outcomes) {
        let SourceEventAllocationOutcomeV1::Allocated(record) = outcome else {
            return Err("checked allocation outcome changed".into());
        };
        assert_eq!(
            reopened.inspect_source_event(&request.repo_id, &request.event)?,
            Some(record.clone())
        );
    }
    Ok(())
}

#[test]
fn allocation_identity_rejects_revision_payload_and_expected_base_changes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
    let fixture = binding("r1", "stream", "event", None, 1);
    let request = SourceEventAllocationRequestV1 {
        repo_id: fixture.target.repo_id,
        revision_id: fixture.target.revision_id,
        event: fixture.event,
    };
    let SourceEventAllocationOutcomeV1::Allocated(original) =
        catalog.allocate_source_event(&request)?
    else {
        panic!("first allocation");
    };
    let path = dir
        .path()
        .join(repository_envelope::file_name(&request.repo_id));
    let before = std::fs::read(&path)?;
    let mut changed_revision = request.clone();
    changed_revision.revision_id = RevisionId::new("r2")?;
    let mut changed_payload = request.clone();
    changed_payload.event.payload_sha256 = [9; 32];
    let mut changed_base = request.clone();
    changed_base.event.expected_base_event_id = Some("prior-event".into());
    for changed in [&changed_revision, &changed_payload, &changed_base] {
        assert!(matches!(
            catalog.allocate_source_event(changed),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::BatchDigestConflict,
                ..
            })
        ));
        assert_eq!(std::fs::read(&path)?, before);
    }
    assert_eq!(
        catalog.inspect_source_event(&request.repo_id, &request.event)?,
        Some(original)
    );
    Ok(())
}

#[test]
fn maximum_counter_refuses_new_number_without_losing_existing_allocation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
    let fixture = binding("r1", "stream-allocated", "event", None, 1);
    let request = SourceEventAllocationRequestV1 {
        repo_id: fixture.target.repo_id,
        revision_id: fixture.target.revision_id,
        event: fixture.event,
    };
    let SourceEventAllocationOutcomeV1::Allocated(original) =
        catalog.allocate_source_event(&request)?
    else {
        panic!("first allocation");
    };
    let terminal = binding("r1", "stream-maximum", "event", None, u64::MAX);
    assert!(matches!(
        catalog.reserve_source_event(&terminal)?,
        SourceEventReservationV1::Reserved(_)
    ));
    let next = binding("r1", "stream-next", "event", None, 2);
    let next_request = SourceEventAllocationRequestV1 {
        repo_id: next.target.repo_id,
        revision_id: next.target.revision_id,
        event: next.event,
    };
    let path = dir
        .path()
        .join(repository_envelope::file_name(&request.repo_id));
    let before = std::fs::read(&path)?;
    assert!(matches!(
        catalog.allocate_source_event(&next_request),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        })
    ));
    assert_eq!(std::fs::read(&path)?, before);
    assert_eq!(
        catalog.allocate_source_event(&request)?,
        SourceEventAllocationOutcomeV1::Existing(original)
    );
    Ok(())
}

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
    drop(guard);
    Ok(())
}

#[test]
fn source_event_cross_revision_replay_preserves_original_binding_and_cas() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
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
    assert_eq!(existing.binding_v1().expect("bound event"), &first);
    assert_eq!(
        existing.phase_v1().expect("bound event"),
        SourceEventPhaseV1::Pending
    );
    let mut conflict = replay;
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
    let reopened = open_catalog(dir.path())?;
    assert_eq!(
        reopened
            .inspect_source_event(&first.target.repo_id, &first.event)?
            .expect("retained event")
            .phase_v1()
            .expect("bound event"),
        SourceEventPhaseV1::Active
    );
    assert_eq!(
        reopened
            .inspect_source_event(&second.target.repo_id, &second.event)?
            .expect("pending event")
            .phase_v1()
            .expect("bound event"),
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
    let catalog = open_catalog(dir.path())?;
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
                .phase_v1()
                .expect("bound event"),
            SourceEventPhaseV1::Pending
        );
        assert!(activate(&catalog, &event).is_err());
    }
    let staged =
        catalog.reconcile_source_event(&event.target.repo_id, &event.event, &journal(&event))?;
    assert_eq!(
        staged.phase_v1().expect("bound event"),
        SourceEventPhaseV1::Staged
    );
    Ok(())
}

#[test]
fn rollback_and_reopen_preserve_source_high_water_above_the_visible_generation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
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
    let reopened = open_catalog(dir.path())?;
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
    let catalog = Arc::new(open_catalog(dir.path())?);
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
    let reopened = open_catalog(dir.path())?;
    for event in &events {
        assert_eq!(
            reopened.active_search_corpus_v1(&event.target.repo_id, &event.target.revision_id)?,
            Some(corpus(event)?)
        );
        assert_eq!(
            reopened
                .inspect_source_event(&event.target.repo_id, &event.event)?
                .expect("retained event")
                .phase_v1()
                .expect("bound event"),
            SourceEventPhaseV1::Active
        );
    }
    assert_eq!(
        std::fs::read_dir(dir.path())?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .count(),
        1
    );
    Ok(())
}

#[test]
fn newer_stream_cannot_overtake_unresolved_source_for_same_pair() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
    let first = binding("r1", "stream-a", "event-a", None, 1);
    let second = binding("r1", "stream-b", "event-b", None, 2);
    let _first = catalog.reserve_source_event(&first)?;
    let _second = catalog.reserve_source_event(&second)?;
    let _second_stage =
        catalog.reconcile_source_event(&second.target.repo_id, &second.event, &journal(&second))?;
    assert!(matches!(
        activate(&catalog, &second),
        Err(CoreError::NotReady(_))
    ));
    assert!(
        catalog
            .active_search_corpus_v1(&first.target.repo_id, &first.target.revision_id)?
            .is_none()
    );

    let _first_stage =
        catalog.reconcile_source_event(&first.target.repo_id, &first.event, &journal(&first))?;
    let coordinator = catalog.lifecycle_coordinator();
    let first_head = {
        let guard = coordinator.lock_pair(&first.target.repo_id, &first.target.revision_id)?;
        catalog
            .activate_prepared_under_guard_v1(
                &guard,
                &PreparedSearchCorpusGenerationV1::new(corpus(&first)?, None)?,
                Some(&first.event),
            )?
            .active
    };
    {
        let guard = coordinator.lock_pair(&second.target.repo_id, &second.target.revision_id)?;
        let _activated = catalog.activate_prepared_under_guard_v1(
            &guard,
            &PreparedSearchCorpusGenerationV1::new(corpus(&second)?, Some(first_head))?,
            Some(&second.event),
        )?;
    }
    assert_eq!(
        catalog.active_search_corpus_v1(&second.target.repo_id, &second.target.revision_id)?,
        Some(corpus(&second)?)
    );
    let stale = binding("r1", "stream-c", "event-c", None, 1);
    assert!(matches!(
        catalog.reserve_source_event(&stale),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusGenerationConflict,
            ..
        })
    ));
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
    configure_catalog(&catalog)?;
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
    let reopened = open_catalog(dir.path())?;
    assert_eq!(
        reopened.active_search_corpus_v1(&event.target.repo_id, &event.target.revision_id)?,
        Some(corpus(&event)?)
    );
    assert_eq!(
        reopened
            .inspect_source_event(&event.target.repo_id, &event.event)?
            .expect("retained event")
            .phase_v1()
            .expect("bound event"),
        SourceEventPhaseV1::Active
    );
    Ok(())
}

#[test]
fn envelope_rejects_legacy_duplicate_reordered_and_oversized_inputs() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
    let event = binding("r1", "stream", "event", None, 1);
    let _reserved = catalog.reserve_source_event(&event)?;
    let path = dir
        .path()
        .join(repository_envelope::file_name(&event.target.repo_id));
    let bytes = std::fs::read(&path)?;
    let row: serde_json::Value = serde_json::from_slice(&bytes)?;
    let first_event = row
        .get(4)
        .and_then(serde_json::Value::as_array)
        .and_then(|events| events.first())
        .ok_or("missing first event")?
        .clone();
    let mut duplicate = row.clone();
    duplicate
        .get_mut(4)
        .ok_or("missing event slot")?
        .as_array_mut()
        .expect("events array")
        .push(first_event.clone());
    assert!(repository_envelope::decode(&path, &serde_json::to_vec(&duplicate)?).is_err());
    let mut wrong_format = row.clone();
    *wrong_format.get_mut(0).ok_or("missing format slot")? = serde_json::json!(1);
    assert!(matches!(
        repository_envelope::decode(&path, &serde_json::to_vec(&wrong_format)?),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            ..
        })
    ));
    assert!(repository_envelope::decode(&path, br#"{"lexical":{},"semantic":{}}"#).is_err());
    let mut overlimit = row;
    *overlimit.get_mut(4).ok_or("missing event slot")? =
        serde_json::Value::Array(vec![first_event; repository_envelope::MAX_EVENTS + 1]);
    assert!(repository_envelope::decode(&path, &serde_json::to_vec(&overlimit)?).is_err());
    let file = std::fs::OpenOptions::new().write(true).open(&path)?;
    file.set_len(
        u64::try_from(repository_envelope::MAX_ENVELOPE_BYTES)?
            .checked_add(1)
            .ok_or("envelope limit overflow")?,
    )?;
    assert!(repository_envelope::read_bounded(&path).is_err());
    Ok(())
}

#[test]
fn current_envelope_rejects_missing_zero_or_duplicate_generation_counter() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
    let fixture = binding("r1", "stream", "event", None, 1);
    let request = SourceEventAllocationRequestV1 {
        repo_id: fixture.target.repo_id,
        revision_id: fixture.target.revision_id,
        event: fixture.event,
    };
    let _allocated = catalog.allocate_source_event(&request)?;
    drop(catalog);
    let path = dir
        .path()
        .join(repository_envelope::file_name(&request.repo_id));
    let original = std::fs::read(&path)?;
    let row: serde_json::Value = serde_json::from_slice(&original)?;
    let counter = row
        .get(5)
        .and_then(serde_json::Value::as_array)
        .and_then(|rows| rows.first())
        .ok_or("missing canonical counter")?
        .clone();

    let mut missing_field = row.clone();
    let _removed = missing_field
        .as_array_mut()
        .ok_or("missing envelope row")?
        .pop();
    let mut missing_pair = row.clone();
    *missing_pair.get_mut(5).ok_or("missing counter slot")? = serde_json::json!([]);
    let mut zero = row.clone();
    *zero
        .get_mut(5)
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|rows| rows.first_mut())
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|row| row.get_mut(1))
        .ok_or("missing counter value")? = serde_json::json!(0);
    let mut duplicate = row;
    duplicate
        .get_mut(5)
        .and_then(serde_json::Value::as_array_mut)
        .ok_or("missing counter rows")?
        .push(counter);

    for malformed in [missing_field, missing_pair, zero, duplicate] {
        let bytes = serde_json::to_vec(&malformed)?;
        assert!(matches!(
            repository_envelope::decode(&path, &bytes),
            Err(CoreError::Storage(_))
        ));
        std::fs::write(&path, &bytes)?;
        assert!(ActivationCatalog::open(dir.path()).is_err());
        assert_eq!(std::fs::read(&path)?, bytes);
    }
    std::fs::write(&path, &original)?;
    assert!(
        open_catalog(dir.path())?
            .inspect_source_event(&request.repo_id, &request.event)?
            .is_some()
    );
    Ok(())
}

#[test]
fn envelope_rejects_multiple_events_for_one_target_generation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
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
        *forged
            .get_mut(4)
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|events| events.get_mut(1))
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|event| event.get_mut(4))
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|bound| bound.get_mut(0))
            .ok_or("missing target slot")? = serde_json::to_value(target)?;
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
        .get_mut(&(second.event.stream_id.clone(), second.event.event_id))
        .expect("second event");
    if let quanta_index_core::SourceEventStateV1::Bound { binding, .. } = &mut duplicate.state {
        binding.target = first.target.clone();
        binding.journal_key.generation = first.target.manifest_generation;
    } else {
        panic!("bound event fixture");
    }
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
    let catalog = open_catalog(dir.path())?;
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
            .phase_v1()
            .expect("bound event"),
        SourceEventPhaseV1::Staged
    );
    std::fs::remove_file(&staging)?;
    std::fs::create_dir(&staging)?;
    let reopened = open_catalog(dir.path())?;
    assert_eq!(
        reopened
            .inspect_source_event(&event.target.repo_id, &event.event)?
            .expect("staged event")
            .phase_v1()
            .expect("bound event"),
        SourceEventPhaseV1::Staged
    );
    Ok(())
}

#[test]
fn stream_capacity_refusal_does_not_evict_identity_or_modify_root() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
    for index in 0..repository_envelope::MAX_STREAMS {
        let event = binding(
            "r1",
            &format!("stream-{index:04}"),
            "event",
            None,
            u64::try_from(index)?
                .checked_add(1)
                .ok_or("stream index overflow")?,
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
            .binding_v1()
            .expect("bound event"),
        &original
    );
    Ok(())
}

#[test]
fn poisoned_publication_writer_fences_serving_and_subsequent_mutations() -> TestResult {
    let dir = tempfile::tempdir()?;
    let catalog = open_catalog(dir.path())?;
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
