//! SEP-21 P03 owner tests — sealed-candidate activation, recovery and
//! quarantine under SQLite-catalog authority (S21-01B + S21-02).
//!
//! Covers the `DoD` matrix: the closed candidate transition table with
//! illegal-transition refusals, same-generation same/different-commitment
//! replay vs conflict, content-bound CAS activation, corruption → durable
//! invalidation → no resurrection, crash at the seal/catalog-commit
//! boundaries (subprocess fault injection), quarantine projection
//! exact-byte replay + journaled tombstone discard, legacy V1 layout
//! refusal, and the ACK binding fields (prior/new commitment, epoch,
//! terminal sequence, replay status).

#![forbid(unsafe_code)]
#![expect(
    clippy::unreachable,
    reason = "test fixtures use invariant literal constructors for repo and revision IDs"
)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "integration tests use Result-returning setup with assertion-style validation"
)]
#![expect(
    clippy::print_stderr,
    reason = "stage traces aid debugging the subprocess crash matrix"
)]

use std::error::Error;
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    CandidateCommitmentV1, FileId, ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV2,
    RepoMapExactnessSummary, RepoMapExpectedActiveV2, RepoMapFileNode, RepoMapGraphCoverage,
    RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapMutationPhaseV2, RepoMapNode,
    RepoMapPublishBundleRequestV2, RepoMapQueryRequest, RepoMapRedactionState, RepoMapSourceBundle,
    RepoRelativePath, RevisionId,
};
use quanta_index_core::{CoreError, RepoMapMutationCommit, RepoMapQuarantinePort};
use quanta_index_repomap::{CandidateProjectionMeta, RepoMapGenerationStore};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn Error>>;

// A distinct valid 64-hex producer digest per fixture marker.

fn read_query_snapshot(
    store: &quanta_index_repomap::RepoMapGenerationStore,
    request: &RepoMapQueryRequest,
) -> Result<quanta_index_contract::RepoMapQueryResponse, quanta_index_core::CoreError> {
    // S21-05: the ambient store read is gone; a test reads through one
    // acquired pinned view, exactly like a production route.
    use quanta_index_core::PinnedRepoMapSnapshot as _;
    store
        .acquire_pinned(&quanta_index_core::RepoMapSnapshotAcquireV1 {
            repo_id: request.repo_id.clone(),
            revision_id: request.revision_id.clone(),
            manifest_generation: request.manifest_generation,
        })?
        .query(request.clone())
}

fn producer_hex(marker: &str) -> String {
    let hash = marker
        .bytes()
        .fold(0_u16, |acc, byte| acc.wrapping_add(u16::from(byte)));
    format!("{}{:04x}", "ab".repeat(30), hash)
}

const CRASH_ROOT_ENV: &str = "QUANTA_INDEX_REPOMAP_P03_CRASH_ROOT";
const CRASH_BOUNDARY_ENV: &str = "QUANTA_INDEX_REPOMAP_CRASH_BOUNDARY";
const REPLAY_SUBSTITUTE_ENV: &str = "QUANTA_INDEX_REPOMAP_V2_REPLAY_SUBSTITUTE";
const CRASH_EXIT_CODE: i32 = 87;
const CATALOG_BUSY_BUDGET: Duration = Duration::from_secs(5);

fn repo() -> RepoId {
    match RepoId::new("repo-p03") {
        Ok(repo) => repo,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    }
}

fn revision() -> RevisionId {
    match RevisionId::new("rev-p03") {
        Ok(revision) => revision,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    }
}

fn bundle(generation: u64, marker: &str) -> RepoMapSourceBundle {
    RepoMapSourceBundle::new(
        repo(),
        revision(),
        ManifestGeneration::new(generation),
        producer_hex(marker),
        format!("snap-{marker}"),
        1,
        "d".repeat(64),
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
    .with_node(RepoMapNode::File(RepoMapFileNode {
        file_id: FileId::new("file://src/lib.rs"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        line_count: 110,
    }))
    .with_node(RepoMapNode::File(RepoMapFileNode {
        file_id: FileId::new("file://src/service/mod.rs"),
        repo_relative_path: RepoRelativePath::new("src/service/mod.rs"),
        line_count: 170,
    }))
}

struct Fixture {
    dir: TempDir,
    catalog: Arc<SqliteCatalog>,
    store: Arc<RepoMapGenerationStore>,
}

fn open_fixture(
    root: &std::path::Path,
) -> Result<(Arc<SqliteCatalog>, Arc<RepoMapGenerationStore>), Box<dyn Error>> {
    let catalog = Arc::new(SqliteCatalog::open(root, CATALOG_BUSY_BUDGET)?);
    let opened = RepoMapGenerationStore::open(root.join("repo-map"), Arc::clone(&catalog))?;
    Ok((catalog, Arc::new(opened.store)))
}

fn fixture() -> Result<Fixture, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (catalog, store) = open_fixture(&root)?;
    Ok(Fixture {
        dir,
        catalog,
        store,
    })
}

fn publish(
    store: &RepoMapGenerationStore,
    generation: u64,
    marker: &str,
) -> Result<RepoMapMutationCommit, CoreError> {
    let request = RepoMapPublishBundleRequestV2::new(bundle(generation, marker))
        .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
    let mutation = store.ingest_bundle_v2(&request)?.mutation;
    Ok(project_mutation(mutation))
}

fn activate(
    store: &RepoMapGenerationStore,
    generation: u64,
) -> Result<RepoMapMutationCommit, CoreError> {
    let mut request = RepoMapActivateGenerationRequestV2::for_bundle(&bundle(
        generation,
        &format!("g{generation}"),
    ))
    .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
    request.expected_active = store.active_head_token(&repo(), &revision())?;
    let mutation = store.activate_generation_v2(&request)?.mutation;
    Ok(project_mutation(mutation))
}

fn project_mutation(mutation: quanta_index_contract::RepoMapMutationAck) -> RepoMapMutationCommit {
    RepoMapMutationCommit {
        prior_candidate_commitment: mutation.prior_candidate_commitment,
        new_candidate_commitment: mutation.new_candidate_commitment,
        activation_epoch: mutation.activation_epoch,
        terminal_sequence: mutation.terminal_sequence,
        replayed: mutation.replayed,
    }
}

fn expected_from_mutation(
    mutation: &RepoMapMutationCommit,
) -> Result<RepoMapExpectedActiveV2, Box<dyn Error>> {
    let epoch = std::num::NonZeroU64::new(mutation.activation_epoch)
        .ok_or_else(|| std::io::Error::other("activation epoch must be positive"))?;
    let commitment = CandidateCommitmentV1::from_wire_str(&mutation.new_candidate_commitment)?;
    Ok(RepoMapExpectedActiveV2::new(epoch, commitment))
}

fn query(store: &RepoMapGenerationStore, generation: u64) -> Result<(), CoreError> {
    let _response = read_query_snapshot(
        store,
        &RepoMapQueryRequest {
            repo_id: repo(),
            revision_id: revision(),
            manifest_generation: ManifestGeneration::new(generation),
            ..query_defaults()
        },
    )?;
    Ok(())
}

fn query_defaults() -> quanta_index_contract::RepoMapQueryRequest {
    // The exact query shape does not matter to these tests; a minimal
    // all-entries page does.
    RepoMapQueryRequest {
        repo_id: repo(),
        revision_id: revision(),
        manifest_generation: ManifestGeneration::new(1),
        query_text: "lib".to_string(),
        top_k: 10,
        token_budget: 1_000,
        focus_subjects: Vec::new(),
    }
}

fn assert_typed(error: &CoreError, code: quanta_index_contract::SearchPlaneErrorCodeV2) {
    match error {
        CoreError::Typed { code: found, .. } if *found == code => {}
        other @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => unreachable!("expected typed {code:?}, got {other:?}"),
    }
}

#[test]
fn closed_transition_table_publish_activate_supersede_and_illegal_refusals() -> TestResult {
    let fixture = fixture()?;
    let store = fixture.store.as_ref();

    // Absent -> Sealed: publish alone never changes active truth.
    let seal = publish(store, 1, "g1")?;
    assert!(!seal.replayed);
    assert_eq!(seal.activation_epoch, 0);
    assert!(seal.prior_candidate_commitment.is_none());
    assert!(seal.new_candidate_commitment.starts_with("sha256:"));
    assert!(query(store, 1).is_err(), "publish alone must not serve");

    // Same logical generation + same commitment: original receipt replay.
    let replay = publish(store, 1, "g1")?;
    assert!(replay.replayed);
    assert_eq!(replay.terminal_sequence, seal.terminal_sequence);

    // Same logical generation + different commitment: typed conflict.
    let conflict = publish(store, 1, "g1-other");
    assert!(conflict.is_err());
    if let Err(error) = conflict {
        assert_typed(
            &error,
            quanta_index_contract::SearchPlaneErrorCodeV2::CandidateCommitmentConflict,
        );
    }

    // Sealed -> Activated.
    let activation = activate(store, 1)?;
    assert!(!activation.replayed);
    assert_eq!(activation.activation_epoch, 1);
    assert!(activation.prior_candidate_commitment.is_none());
    assert!(query(store, 1).is_ok());

    // Activated -> (same) replay of the original activation receipt.
    let replay_activation = store
        .activate_generation_v2(&RepoMapActivateGenerationRequestV2::for_bundle(&bundle(
            1, "g1",
        ))?)?
        .mutation;
    assert!(replay_activation.replayed);
    assert_eq!(
        replay_activation.terminal_sequence,
        activation.terminal_sequence
    );

    // Activate a fresh generation: supersedes, binds prior commitment.
    let seal_two = publish(store, 2, "g2")?;
    let supersede = activate(store, 2)?;
    assert!(!supersede.replayed);
    assert_eq!(supersede.activation_epoch, 2);
    assert_eq!(
        supersede.prior_candidate_commitment.as_deref(),
        Some(seal.new_candidate_commitment.as_str())
    );
    let supersede_replay = store
        .activate_generation_v2(
            &RepoMapActivateGenerationRequestV2::for_bundle(&bundle(2, "g2"))?
                .with_expected_active(expected_from_mutation(&activation)?),
        )?
        .mutation;
    assert!(supersede_replay.replayed);
    assert_eq!(
        supersede_replay.prior_candidate_commitment, supersede.prior_candidate_commitment,
        "activation replay must reproduce the original prior commitment"
    );
    assert_eq!(
        supersede_replay.terminal_sequence,
        supersede.terminal_sequence
    );
    assert!(query(store, 2).is_ok());
    assert!(
        query(store, 1).is_err(),
        "superseded generation must not serve"
    );
    drop(seal_two);

    // Activating an absent generation: typed NotFound.
    let absent = activate(store, 9);
    assert!(absent.is_err());

    // An empty manifest digest: refused before any mutation.
    let mut bad_request = RepoMapActivateGenerationRequestV2::for_bundle(&bundle(2, "g2"))?;
    bad_request.manifest_digest = String::new();
    assert!(store.activate_generation_v2(&bad_request).is_err());
    Ok(())
}

#[test]
fn global_terminal_sequence_is_strictly_monotonic_across_operations() -> TestResult {
    let fixture = fixture()?;
    let store = fixture.store.as_ref();
    let sequences = vec![
        publish(store, 1, "g1")?.terminal_sequence,
        activate(store, 1)?.terminal_sequence,
        publish(store, 2, "g2")?.terminal_sequence,
        activate(store, 2)?.terminal_sequence,
    ];
    for (previous, following) in sequences.iter().zip(sequences.iter().skip(1)) {
        assert!(
            previous < following,
            "terminal sequences must be globally monotonic: {sequences:?}"
        );
    }
    // The allocator stays strictly ahead of every emitted terminal
    // sequence. It is not exactly last+1: supersede invalidations and other
    // lanes' events consume interleaved sequences from the same global
    // allocator by design.
    let (next, exhausted) = fixture.catalog.sequence_allocator()?;
    assert!(!exhausted);
    let next = next.expect("allocator is not exhausted");
    assert!(
        next > sequences.last().copied().unwrap_or_default(),
        "allocator next {next} must exceed every emitted terminal sequence"
    );
    Ok(())
}

#[test]
fn v2_receipts_bind_full_bundle_and_replay_after_restart() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let source = bundle(1, "g1");
    let publish_request = RepoMapPublishBundleRequestV2::new(source.clone())?;
    let activate_request = RepoMapActivateGenerationRequestV2::for_bundle(&source)?;

    let (publish, activate) = {
        let (_catalog, store) = open_fixture(&root)?;
        let publish = store.ingest_bundle_v2(&publish_request)?;
        assert_eq!(publish.phase, RepoMapMutationPhaseV2::Publish);
        assert!(publish.mutation.prior_candidate_commitment.is_none());
        assert_eq!(publish.mutation.activation_epoch, 0);
        assert_eq!(
            publish.source_bundle_digest,
            publish_request.source_bundle_digest
        );
        let activate = store.activate_generation_v2(&activate_request)?;
        assert_eq!(activate.phase, RepoMapMutationPhaseV2::Activate);
        assert_eq!(activate.source_bundle_digest, publish.source_bundle_digest);
        assert_eq!(
            activate.mutation.new_candidate_commitment,
            publish.mutation.new_candidate_commitment
        );
        assert!(activate.mutation.terminal_sequence > publish.mutation.terminal_sequence);
        (publish, activate)
    };

    let (_catalog, reopened) = open_fixture(&root)?;
    let publish_replay = reopened.ingest_bundle_v2(&publish_request)?;
    let activate_replay = reopened.activate_generation_v2(&activate_request)?;
    assert!(publish_replay.mutation.replayed);
    assert!(activate_replay.mutation.replayed);
    let mut expected_publish_replay = publish;
    expected_publish_replay.mutation.replayed = true;
    assert_eq!(publish_replay, expected_publish_replay);
    let mut expected_activate_replay = activate;
    expected_activate_replay.mutation.replayed = true;
    assert_eq!(activate_replay, expected_activate_replay);
    Ok(())
}

#[test]
fn v2_activation_replay_repairs_missing_in_process_projection() -> TestResult {
    let fixture = fixture()?;
    let source = bundle(1, "g1");
    let published = fixture
        .store
        .ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(source.clone())?)?;
    let commitment =
        CandidateCommitmentV1::from_wire_str(&published.mutation.new_candidate_commitment)?;

    // Model the boundary after the catalog commit but before the store's
    // in-memory head projection is updated. A retry must derive the missing
    // projection from the committed catalog outcome, not mint a new event.
    let committed = fixture.catalog.activate_repomap_candidate(
        source.repo_id.as_str(),
        source.revision_id.as_str(),
        source.manifest_generation.get(),
        commitment.as_bytes(),
        None,
    )?;
    assert!(!committed.replayed);
    assert!(query(fixture.store.as_ref(), 1).is_err());

    let replay = fixture
        .store
        .activate_generation_v2(&RepoMapActivateGenerationRequestV2::for_bundle(&source)?)?;
    assert!(replay.mutation.replayed);
    assert_eq!(
        replay.mutation.terminal_sequence,
        u64::try_from(committed.terminal_sequence)?
    );
    assert_eq!(replay.mutation.activation_epoch, committed.epoch);
    assert_eq!(
        fixture
            .store
            .activated_generation_for(&repo(), &revision())?,
        Some(1)
    );
    query(fixture.store.as_ref(), 1)?;
    Ok(())
}

#[test]
fn concurrent_v2_activations_leave_projection_at_catalog_head() -> TestResult {
    let fixture = fixture()?;
    let first_source = bundle(1, "g1");
    let second_source = bundle(2, "g2");
    let _first_publish = fixture
        .store
        .ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(first_source.clone())?)?;
    let _second_publish = fixture
        .store
        .ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(second_source.clone())?)?;
    let first_request = RepoMapActivateGenerationRequestV2::for_bundle(&first_source)?;
    let second_request = RepoMapActivateGenerationRequestV2::for_bundle(&second_source)?;
    let start = Arc::new(Barrier::new(3));

    let first_store = Arc::clone(&fixture.store);
    let first_start = Arc::clone(&start);
    let first = thread::spawn(move || {
        let _started = first_start.wait();
        first_store.activate_generation_v2(&first_request)
    });
    let second_store = Arc::clone(&fixture.store);
    let second_start = Arc::clone(&start);
    let second = thread::spawn(move || {
        let _started = second_start.wait();
        second_store.activate_generation_v2(&second_request)
    });
    let _started = start.wait();
    let first_result = first
        .join()
        .map_err(|_panic| std::io::Error::other("first activation worker panicked"))?;
    let second_result = second
        .join()
        .map_err(|_panic| std::io::Error::other("second activation worker panicked"))?;
    assert!(first_result.is_ok() || second_result.is_ok());

    let active = fixture
        .catalog
        .repomap_activation_row(repo().as_str(), revision().as_str())?
        .ok_or_else(|| std::io::Error::other("catalog has no activated head"))?;
    assert!(active.active);
    assert_eq!(
        fixture
            .store
            .activated_generation_for(&repo(), &revision())?,
        Some(active.manifest_generation)
    );
    query(fixture.store.as_ref(), active.manifest_generation)?;
    Ok(())
}

#[test]
fn stale_prepared_activation_cannot_replace_newer_catalog_head() -> TestResult {
    let fixture = fixture()?;
    let stale_source = bundle(1, "stale");
    let current_source = bundle(2, "current");
    let _stale_publish = fixture
        .store
        .ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(stale_source.clone())?)?;
    let _current_publish = fixture
        .store
        .ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(current_source.clone())?)?;
    let current =
        fixture
            .store
            .activate_generation_v2(&RepoMapActivateGenerationRequestV2::for_bundle(
                &current_source,
            )?)?;
    let before = fixture
        .catalog
        .repomap_activation_row(repo().as_str(), revision().as_str())?
        .ok_or_else(|| std::io::Error::other("active catalog row missing"))?;
    let sequence_before = fixture.catalog.sequence_allocator()?;

    let stale =
        fixture
            .store
            .activate_generation_v2(&RepoMapActivateGenerationRequestV2::for_bundle(
                &stale_source,
            )?);
    let stale = stale.expect_err("stale activation must not supersede catalog head");
    assert_typed(
        &stale,
        quanta_index_contract::SearchPlaneErrorCodeV2::ActivationCasConflict,
    );
    let after = fixture
        .catalog
        .repomap_activation_row(repo().as_str(), revision().as_str())?
        .ok_or_else(|| std::io::Error::other("active catalog row missing"))?;
    assert_eq!(after, before);
    assert_eq!(fixture.catalog.sequence_allocator()?, sequence_before);
    assert_eq!(after.manifest_generation, 2);
    assert_eq!(current.mutation.activation_epoch, after.epoch);
    query(fixture.store.as_ref(), 2)?;
    Ok(())
}

#[test]
fn activation_replay_refuses_a_changed_prior_head_expectation() -> TestResult {
    let fixture = fixture()?;
    let source = bundle(1, "g1");
    let _published = fixture
        .store
        .ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(source.clone())?)?;
    let original = RepoMapActivateGenerationRequestV2::for_bundle(&source)?;
    let committed = fixture.store.activate_generation_v2(&original)?;
    let row_before = fixture
        .catalog
        .repomap_activation_row(repo().as_str(), revision().as_str())?
        .ok_or_else(|| std::io::Error::other("active catalog row missing"))?;
    let sequence_before = fixture.catalog.sequence_allocator()?;
    let changed = original.with_expected_active(
        fixture
            .store
            .active_head_token(&repo(), &revision())?
            .ok_or_else(|| std::io::Error::other("active token missing"))?,
    );
    let error = fixture
        .store
        .activate_generation_v2(&changed)
        .expect_err("a changed prior expectation cannot borrow the original receipt");
    assert_typed(
        &error,
        quanta_index_contract::SearchPlaneErrorCodeV2::ActivationCasConflict,
    );
    assert_eq!(
        fixture
            .catalog
            .repomap_activation_row(repo().as_str(), revision().as_str())?,
        Some(row_before),
    );
    assert_eq!(fixture.catalog.sequence_allocator()?, sequence_before);
    let replay = fixture
        .store
        .activate_generation_v2(&RepoMapActivateGenerationRequestV2::for_bundle(&source)?)?;
    assert!(replay.mutation.replayed);
    assert_eq!(
        replay.mutation.terminal_sequence,
        committed.mutation.terminal_sequence
    );
    Ok(())
}

#[test]
fn v2_ack_loss_replay_skips_object_and_catalog_seal() -> TestResult {
    if let Some(root) = std::env::var_os(CRASH_ROOT_ENV) {
        let (_catalog, store) = open_fixture(&std::path::PathBuf::from(root))?;
        if std::env::var_os(REPLAY_SUBSTITUTE_ENV).is_some() {
            let mut source = bundle(1, "g1");
            source.snapshot_id.push_str("-foreign");
            let request = RepoMapPublishBundleRequestV2::new(source)?;
            let error = store
                .ingest_bundle_v2(&request)
                .expect_err("same logical key with different source must refuse before sealing");
            assert_typed(
                &error,
                quanta_index_contract::SearchPlaneErrorCodeV2::CandidateCommitmentConflict,
            );
            return Ok(());
        }
        let request = RepoMapPublishBundleRequestV2::new(bundle(1, "g1"))?;
        let replay = store.ingest_bundle_v2(&request)?;
        assert!(replay.mutation.replayed);
        return Ok(());
    }

    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let (_catalog, store) = open_fixture(root)?;
    let request = RepoMapPublishBundleRequestV2::new(bundle(1, "g1"))?;
    let first = store.ingest_bundle_v2(&request)?;
    assert!(!first.mutation.replayed);
    drop(store);

    for boundary in ["after-object-sync", "after-catalog-commit"] {
        for substituted in [false, true] {
            let mut child = Command::new(std::env::current_exe()?);
            let _configured = child
                .arg("--exact")
                .arg("v2_ack_loss_replay_skips_object_and_catalog_seal")
                .env(CRASH_ROOT_ENV, root)
                .env(CRASH_BOUNDARY_ENV, boundary);
            if substituted {
                let _configured = child.env(REPLAY_SUBSTITUTE_ENV, "1");
            }
            let status = child.status()?;
            assert!(
                status.success(),
                "V2 ACK-loss replay entered {boundary} (substituted={substituted}): {status}"
            );
        }
    }
    Ok(())
}

#[test]
fn v2_replay_refuses_corrupt_sealed_object() -> TestResult {
    let fixture = fixture()?;
    let request = RepoMapPublishBundleRequestV2::new(bundle(1, "g1"))?;
    let _first = fixture.store.ingest_bundle_v2(&request)?;
    let before = fixture
        .catalog
        .repomap_candidate_row("repo-p03", "rev-p03", 1)?
        .expect("published candidate is durable");
    let object_path = find_single_object(
        &fixture
            .dir
            .path()
            .join("repo-map")
            .join("objects")
            .join("sha256"),
    )?;
    std::fs::write(&object_path, b"corrupt candidate")?;

    let error = fixture
        .store
        .ingest_bundle_v2(&request)
        .expect_err("replay must verify the sealed object");
    assert_typed(
        &error,
        quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
    );
    assert_eq!(
        fixture
            .catalog
            .repomap_candidate_row("repo-p03", "rev-p03", 1)?
            .expect("replay refusal preserves catalog row"),
        before,
    );
    Ok(())
}

#[test]
fn legacy_projection_meta_without_strong_custody_is_refused() {
    let source = bundle(1, "g1");
    let legacy_json = serde_json::json!({
        "snapshot_id": source.snapshot_id,
        "projection_version": source.projection_version,
        "authority_digest": source.authority_digest,
        "item_index_availability": source.graph_coverage.item_index_availability.as_code_str(),
        "graph_coverage_class": source.graph_coverage.graph_coverage_class.as_code_str(),
        "exactness_summary": source.exactness_summary.as_code_str(),
        "redaction_state": source.redaction_state.as_code_str(),
    });
    let upgrade_error = CandidateProjectionMeta::from_json(&legacy_json.to_string())
        .expect_err("legacy metadata without strong custody must be refused");
    assert_typed(
        &upgrade_error,
        quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
    );
}

#[expect(
    clippy::indexing_slicing,
    reason = "each key is written into the parsed object right after parsing it"
)]
#[test]
fn projection_meta_rejects_partial_or_malformed_v2_strong_custody() -> TestResult {
    let source = bundle(1, "g1");
    let current_meta = CandidateProjectionMeta::from_bundle_with_source_digest(
        &source,
        RepoMapPublishBundleRequestV2::new(source.clone())?.source_bundle_digest,
    );
    let current_json = current_meta.to_json()?;
    let mut partial: serde_json::Value = serde_json::from_str(&current_json)?;
    let removed = partial
        .as_object_mut()
        .expect("metadata object")
        .remove("source_bundle_digest");
    assert!(removed.is_some());
    let error = CandidateProjectionMeta::from_json(&serde_json::to_string(&partial)?)
        .expect_err("one strong-custody field without its pair is corrupt");
    assert_typed(
        &error,
        quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
    );

    let mut wrong_types: serde_json::Value = serde_json::from_str(&current_json)?;
    wrong_types["manifest_digest"] = serde_json::json!(7);
    wrong_types["source_bundle_digest"] = serde_json::json!(["sha256:invalid"]);
    let error = CandidateProjectionMeta::from_json(&serde_json::to_string(&wrong_types)?)
        .expect_err("non-string strong-custody fields must refuse");
    assert_typed(
        &error,
        quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
    );

    let mut malformed_digests: serde_json::Value = serde_json::from_str(&current_json)?;
    malformed_digests["manifest_digest"] = serde_json::json!("not-a-manifest-digest");
    malformed_digests["source_bundle_digest"] =
        serde_json::json!(format!("sha256:{}", "A".repeat(64)));
    let error = CandidateProjectionMeta::from_json(&serde_json::to_string(&malformed_digests)?)
        .expect_err("malformed strong-custody digest strings must fail catalog decode");
    assert_typed(
        &error,
        quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
    );
    Ok(())
}

#[test]
fn v2_refuses_digest_and_activation_axis_substitution() -> TestResult {
    let fixture = fixture()?;
    let source = bundle(1, "g1");
    let mut corrupt_publish = RepoMapPublishBundleRequestV2::new(source.clone())?;
    corrupt_publish.source_bundle_digest = format!("sha256:{}", "0".repeat(64));
    assert!(fixture.store.ingest_bundle_v2(&corrupt_publish).is_err());
    assert!(
        fixture
            .catalog
            .repomap_candidate_row("repo-p03", "rev-p03", 1)?
            .is_none(),
        "digest refusal must happen before candidate mutation"
    );

    let publish = RepoMapPublishBundleRequestV2::new(source.clone())?;
    let _receipt = fixture.store.ingest_bundle_v2(&publish)?;
    let durable_before = fixture
        .catalog
        .repomap_candidate_row("repo-p03", "rev-p03", 1)?
        .expect("V2 publish sealed the candidate");
    let mut substituted_source = source.clone();
    substituted_source.snapshot_id.push_str("-foreign");
    let substituted_publish = RepoMapPublishBundleRequestV2::new(substituted_source)?;
    let replay_error = fixture
        .store
        .ingest_bundle_v2(&substituted_publish)
        .expect_err("same commitment with substituted custody axes must fail");
    assert_typed(
        &replay_error,
        quanta_index_contract::SearchPlaneErrorCodeV2::CandidateCommitmentConflict,
    );
    let durable_after = fixture
        .catalog
        .repomap_candidate_row("repo-p03", "rev-p03", 1)?
        .expect("replay refusal preserves the durable candidate");
    assert_eq!(durable_after, durable_before);

    let mut foreign_activate = RepoMapActivateGenerationRequestV2::for_bundle(&source)?;
    foreign_activate.snapshot_id.push_str("-foreign");
    assert!(
        fixture
            .store
            .activate_generation_v2(&foreign_activate)
            .is_err()
    );
    assert!(
        fixture
            .catalog
            .repomap_activation_row("repo-p03", "rev-p03")?
            .is_none(),
        "axis refusal must happen before activation mutation"
    );
    Ok(())
}

#[test]
fn corruption_durable_invalidation_and_no_resurrection() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    {
        let (_catalog, store) = open_fixture(&root)?;
        let _seal = publish(store.as_ref(), 1, "g1")?;
        let _activated = activate(store.as_ref(), 1)?;
        assert!(query(store.as_ref(), 1).is_ok());
    }
    // Corrupt the sealed object bytes in place.
    let objects_dir = root.join("repo-map").join("objects").join("sha256");
    let object_path = find_single_object(&objects_dir)?;
    let original = std::fs::read(&object_path)?;
    let mut corrupted = original.clone();
    if let Some(last) = corrupted.last_mut() {
        *last = last.wrapping_add(1);
    }
    std::fs::write(&object_path, &corrupted)?;

    // Boot reconcile: durable invalidation, unserveable.
    eprintln!("STAGE: reopening after corruption");
    let (catalog, store) = open_fixture(&root)?;
    assert!(
        query(store.as_ref(), 1).is_err(),
        "corrupt object must not serve"
    );
    let activation = catalog.repomap_activation_row("repo-p03", "rev-p03")?;
    let activation = activation.expect("activation row survives invalidation");
    assert!(!activation.active, "corruption must durably invalidate");
    assert!(activation.invalidation_reason.is_some());
    // The corrupt object is gone (quarantined, not left to serve).
    assert!(!object_path.exists());
    // Re-activation of the invalidated candidate: typed refusal.
    let refused = activate(store.as_ref(), 1);
    assert!(refused.is_err());

    // Repair publish of the same original bytes, restart: still no
    // automatic activation.
    std::fs::write(&object_path, &original)?;
    eprintln!("STAGE: reopening after repair");
    let (_catalog, store) = open_fixture(&root)?;
    assert!(
        query(store.as_ref(), 1).is_err(),
        "file reappearance must never resurrect an invalidated activation"
    );
    Ok(())
}

fn find_single_object(objects_dir: &std::path::Path) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let mut found: Option<std::path::PathBuf> = None;
    for entry in walk(objects_dir)? {
        if entry.extension().and_then(|ext| ext.to_str()) == Some("cbor") {
            assert!(
                found.replace(entry.clone()).is_none(),
                "fixture holds exactly one object"
            );
        }
    }
    Ok(found.ok_or("no sealed object found")?)
}

fn walk(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>, Box<dyn Error>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path)?);
        } else {
            out.push(path);
        }
    }
    Ok(out)
}

#[test]
fn quarantine_projection_exact_byte_replay_and_tombstone_discard() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (catalog, store) = open_fixture(&root)?;
    let _sealed = publish(store.as_ref(), 1, "g1")?;
    let _activated = activate(store.as_ref(), 1)?;
    let object_path = find_single_object(&root.join("repo-map").join("objects").join("sha256"))?;
    let original = std::fs::read(&object_path)?;
    std::fs::remove_file(&object_path)?;

    // Missing object at open: durable quarantine incident + invalidation.
    let (_catalog, store) = open_fixture(&root)?;
    assert!(query(store.as_ref(), 1).is_err());
    let incidents = catalog.repomap_quarantine_incidents()?;
    assert_eq!(incidents.len(), 1, "one durable incident");
    let incident = incidents.first().expect("checked length");
    assert!(!incident.envelope_bytes.is_empty());
    // The incident envelope and (absent) payload projections live under
    // the quarantine content addresses.
    let incident_hex = hex(&incident.envelope_digest);
    eprintln!(
        "STAGE: incidents={:?}",
        incidents
            .iter()
            .map(|row| (&row.source_path, row.sequence))
            .collect::<Vec<_>>()
    );
    let incident_path = root
        .join("repo-map")
        .join("quarantine")
        .join("incidents")
        .join("sha256")
        .join(incident_hex.get(..2).unwrap_or_default())
        .join(incident_hex.get(2..4).unwrap_or_default())
        .join(format!(
            "{}.cbor",
            incident_hex.get(4..).unwrap_or_default()
        ));
    eprintln!(
        "STAGE: expect path {} exists={}",
        incident_path.display(),
        incident_path.exists()
    );
    assert_eq!(
        std::fs::read(&incident_path)?,
        incident.envelope_bytes,
        "the durable projection holds the exact envelope bytes"
    );
    // Reopen again: the exact retry reuses the same incident (no new
    // sequence/time); the incident row is the authority.
    let incidents_again = catalog.repomap_quarantine_incidents()?;
    assert_eq!(incidents_again.len(), 1);
    assert_eq!(
        incidents_again.first().map(|row| row.sequence),
        Some(incident.sequence)
    );

    // Listing + journaled tombstone discard: payload-only reclaim.
    let mut listed = store.as_ref().quarantined_files()?;
    assert_eq!(listed.len(), 1);
    let entry = listed.pop().expect("one entry");
    // A stale reason is refused typed.
    let refused =
        store
            .as_ref()
            .discard_quarantined_file(&quanta_index_core::QuarantinedRepoMapFileV1 {
                file_name: entry.file_name.clone(),
                reason: "some-other-reason".to_string(),
            });
    assert!(refused.is_err());
    let outcome = store.as_ref().discard_quarantined_file(&entry)?;
    match outcome {
        quanta_index_core::QuarantineDiscardOutcomeV1::Discarded { .. } => {}
        other @ quanta_index_core::QuarantineDiscardOutcomeV1::Absent => {
            unreachable!("discard must reclaim the payload, got {other:?}")
        }
    }
    // The incident/event row survives; a second discard is Absent.
    let incidents_after = catalog.repomap_quarantine_incidents()?;
    assert_eq!(
        incidents_after.len(),
        1,
        "the incident row is never deleted"
    );
    assert!(incidents_after.first().expect("one").discarded);
    let again = store.as_ref().discard_quarantined_file(&entry)?;
    assert!(matches!(
        again,
        quanta_index_core::QuarantineDiscardOutcomeV1::Absent
    ));
    drop(original);
    Ok(())
}

#[test]
fn tombstone_committed_before_payload_reclaim_is_retried() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (catalog, store) = open_fixture(&root)?;
    let _sealed = publish(store.as_ref(), 1, "g1")?;
    let _activated = activate(store.as_ref(), 1)?;
    let object_path = find_single_object(&root.join("repo-map").join("objects").join("sha256"))?;
    let damaged_bytes = b"damaged candidate payload";
    std::fs::write(&object_path, damaged_bytes)?;
    drop(store);
    drop(catalog);

    let (catalog, store) = open_fixture(&root)?;
    let incidents = catalog.repomap_quarantine_incidents()?;
    let incident = incidents.first().ok_or("missing durable incident")?;
    if incidents.len() != 1 {
        return Err("damaged object must create exactly one incident".into());
    }
    let mut listed = store.as_ref().quarantined_files()?;
    let entry = listed.pop().ok_or("missing listed incident")?;
    if !listed.is_empty() {
        return Err("damaged object must list exactly one incident".into());
    }
    let payload_path = payload_projection_path(&root, &incident.payload_digest)?;
    if std::fs::read(&payload_path)? != damaged_bytes {
        return Err("quarantine projection must contain the damaged bytes".into());
    }

    // Simulate process death after the durable tombstone, before unlinking
    // its payload projection. The old listed target is still retryable.
    let discard_sequence = catalog.discard_repomap_quarantine_payload(&incident.incident_digest)?;
    if discard_sequence <= incident.sequence || !payload_path.is_file() {
        return Err("fault point requires a committed tombstone and live payload".into());
    }
    match store.as_ref().discard_quarantined_file(&entry)? {
        quanta_index_core::QuarantineDiscardOutcomeV1::Discarded { bytes }
            if bytes == u64::try_from(damaged_bytes.len())? => {}
        quanta_index_core::QuarantineDiscardOutcomeV1::Discarded { bytes } => {
            return Err(format!("retry reclaimed the wrong byte count: {bytes}").into());
        }
        quanta_index_core::QuarantineDiscardOutcomeV1::Absent => {
            return Err("retry must reclaim the pending payload, not return Absent".into());
        }
    }
    if payload_path.exists() {
        return Err("retry left the tombstoned payload on disk".into());
    }
    let after = catalog.repomap_quarantine_incidents()?;
    if after.first().and_then(|row| row.discard_sequence) != Some(discard_sequence) {
        return Err("retry changed the durable discard receipt".into());
    }
    if !matches!(
        store.as_ref().discard_quarantined_file(&entry)?,
        quanta_index_core::QuarantineDiscardOutcomeV1::Absent
    ) {
        return Err("fully reclaimed incident must replay as absent".into());
    }
    Ok(())
}

#[test]
fn boot_finishes_a_tombstoned_payload_without_a_client_retry() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (catalog, store) = open_fixture(&root)?;
    let _sealed = publish(store.as_ref(), 1, "g1")?;
    let object_path = find_single_object(&root.join("repo-map").join("objects").join("sha256"))?;
    let damaged_bytes = b"unreclaimed payload";
    std::fs::write(&object_path, damaged_bytes)?;
    drop(store);
    drop(catalog);

    let (catalog, store) = open_fixture(&root)?;
    let incidents = catalog.repomap_quarantine_incidents()?;
    let incident = incidents.first().ok_or("missing durable incident")?;
    if incidents.len() != 1 {
        return Err("damaged object must create one incident".into());
    }
    let entry = store
        .as_ref()
        .quarantined_files()?
        .pop()
        .ok_or("missing listed incident")?;
    let payload_path = payload_projection_path(&root, &incident.payload_digest)?;
    if std::fs::read(&payload_path)? != damaged_bytes {
        return Err("fault point requires a projected payload".into());
    }
    let discard_sequence = catalog.discard_repomap_quarantine_payload(&incident.incident_digest)?;
    drop(store);
    drop(catalog);

    let (catalog, store) = open_fixture(&root)?;
    if payload_path.exists() {
        return Err("boot failed to finish tombstoned payload reclaim".into());
    }
    let after = catalog.repomap_quarantine_incidents()?;
    if after.first().and_then(|row| row.discard_sequence) != Some(discard_sequence) {
        return Err("boot changed the tombstone receipt".into());
    }
    if !matches!(
        store.as_ref().discard_quarantined_file(&entry)?,
        quanta_index_core::QuarantineDiscardOutcomeV1::Absent
    ) {
        return Err("fully recovered tombstone must replay as absent".into());
    }
    Ok(())
}

#[test]
fn discarding_one_incident_preserves_a_shared_live_payload() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (catalog, store) = open_fixture(&root)?;
    let _first = publish(store.as_ref(), 1, "g1")?;
    let _second = publish(store.as_ref(), 2, "g2")?;
    let object_root = root.join("repo-map").join("objects").join("sha256");
    let objects = walk(&object_root)?;
    if objects.len() != 2 {
        return Err("fixture requires two distinct candidate objects".into());
    }
    let damaged_bytes = b"same damaged bytes";
    for path in &objects {
        std::fs::write(path, damaged_bytes)?;
    }
    drop(store);
    drop(catalog);

    let (catalog, store) = open_fixture(&root)?;
    let incidents = catalog.repomap_quarantine_incidents()?;
    let listed = store.as_ref().quarantined_files()?;
    let [first_incident, second_incident] = incidents.as_slice() else {
        return Err("two damaged candidates require two incidents".into());
    };
    let [first_entry, second_entry] = listed.as_slice() else {
        return Err("two damaged candidates require two listed incidents".into());
    };
    if first_incident.payload_digest != second_incident.payload_digest {
        return Err("fixture requires one shared content-addressed payload".into());
    }
    let payload_path = payload_projection_path(&root, &first_incident.payload_digest)?;
    if std::fs::read(&payload_path)
        .map_err(|error| format!("shared projection missing before discard: {error}"))?
        != damaged_bytes
    {
        return Err("both incidents must share the projected bytes".into());
    }
    let first = store.as_ref().discard_quarantined_file(first_entry)?;
    if !matches!(
        first,
        quanta_index_core::QuarantineDiscardOutcomeV1::Discarded { .. }
    ) {
        return Err("first listed incident was not discarded".into());
    }
    if std::fs::read(&payload_path)
        .map_err(|error| format!("shared projection missing after first discard: {error}"))?
        != damaged_bytes
    {
        return Err("discarding one incident removed another live incident's payload".into());
    }
    let remaining = store.as_ref().quarantined_files()?;
    if remaining != [second_entry.clone()] {
        return Err("one live incident must remain listed".into());
    }
    if !matches!(
        store.as_ref().discard_quarantined_file(first_entry)?,
        quanta_index_core::QuarantineDiscardOutcomeV1::Absent
    ) {
        return Err("repeated discard with a shared live payload must be Absent".into());
    }
    let second = store.as_ref().discard_quarantined_file(second_entry)?;
    if !matches!(
        second,
        quanta_index_core::QuarantineDiscardOutcomeV1::Discarded { .. }
    ) || payload_path.exists()
    {
        return Err("last incident must reclaim the unreferenced payload".into());
    }
    Ok(())
}

fn payload_projection_path(
    root: &std::path::Path,
    digest: &[u8; 32],
) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let payload_hex = hex(digest);
    Ok(root
        .join("repo-map")
        .join("quarantine")
        .join("payloads")
        .join("sha256")
        .join(payload_hex.get(..2).ok_or("short payload digest")?)
        .join(payload_hex.get(2..4).ok_or("short payload digest")?)
        .join(format!(
            "{}.bin",
            payload_hex.get(4..).ok_or("short payload digest")?
        )))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

#[test]
fn legacy_v1_root_refuses_mutation_typed_and_untouched() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let repo_map_root = root.join("repo-map");
    let activations = repo_map_root.join("activations");
    std::fs::create_dir_all(&activations)?;
    let legacy_file = activations.join("repo--rev.json");
    let legacy_bytes = br#"{"repo_id":"r","revision_id":"v","manifest_generation":7}"#;
    std::fs::write(&legacy_file, legacy_bytes)?;
    let before = std::fs::symlink_metadata(&legacy_file)?;

    let (_catalog, store) = open_fixture(&root)?;
    let refused_publish = publish(store.as_ref(), 1, "g1");
    assert!(refused_publish.is_err());
    if let Err(error) = refused_publish {
        assert_typed(
            &error,
            quanta_index_contract::SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
        );
    }
    let v2_request = RepoMapPublishBundleRequestV2::new(bundle(1, "g1"))?;
    let refused_v2 = store
        .as_ref()
        .ingest_bundle_v2(&v2_request)
        .expect_err("V2 publish must refuse the legacy root before catalog mutation");
    assert_typed(
        &refused_v2,
        quanta_index_contract::SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
    );
    let refused_activate = activate(store.as_ref(), 1);
    assert!(refused_activate.is_err());
    if let Err(error) = refused_activate {
        assert_typed(
            &error,
            quanta_index_contract::SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
        );
    }
    // Legacy bytes/inode/mtime untouched.
    let after = std::fs::symlink_metadata(&legacy_file)?;
    assert_eq!(std::fs::read(&legacy_file)?, legacy_bytes.to_vec());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before.mtime(), after.mtime());
    }
    Ok(())
}

#[test]
fn ack_binding_fields_carry_prior_new_commitment_epoch_sequence_replay() -> TestResult {
    let fixture = fixture()?;
    let store = fixture.store.as_ref();
    let first = publish(store, 1, "g1")?;
    assert!(first.prior_candidate_commitment.is_none());
    assert_eq!(first.activation_epoch, 0);
    let activate_first = activate(store, 1)?;
    assert_eq!(activate_first.activation_epoch, 1);
    let second = publish(store, 2, "g2")?;
    assert!(second.prior_candidate_commitment.is_none());
    assert_eq!(second.activation_epoch, 0);
    let activate_second = activate(store, 2)?;
    assert_eq!(activate_second.activation_epoch, 2);
    assert_eq!(
        activate_second.prior_candidate_commitment.as_deref(),
        Some(first.new_candidate_commitment.as_str())
    );
    assert_ne!(
        activate_second.new_candidate_commitment,
        activate_second
            .prior_candidate_commitment
            .unwrap_or_default()
    );
    Ok(())
}

#[test]
fn restart_serves_the_activated_generation_with_identical_projection() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    {
        let (_catalog, store) = open_fixture(&root)?;
        let _sealed = publish(store.as_ref(), 1, "g1")?;
        let _activated = activate(store.as_ref(), 1)?;
    }
    let (_catalog, store) = open_fixture(&root)?;
    assert!(query(store.as_ref(), 1).is_ok());
    // The restart-rebuilt registry keeps the bundle-declared snapshot id.
    let response = read_query_snapshot(
        store.as_ref(),
        &RepoMapQueryRequest {
            repo_id: repo(),
            revision_id: revision(),
            manifest_generation: ManifestGeneration::new(1),
            ..query_defaults()
        },
    )?;
    assert_eq!(response.snapshot_meta.snapshot_id, "snap-g1");
    assert!(!response.entries.is_empty());
    Ok(())
}

/// The crash matrix: a child process is killed at each boundary of the
/// seal protocol; the parent reopens and must observe either the old
/// committed authority or the new one — never a mix, never a failure.
#[test]
fn crash_at_seal_boundaries_converges_to_old_or_new_committed_authority() -> TestResult {
    // Child side of the matrix: publish generation 2 under the crash root
    // and die at the configured boundary.
    if let Some(root) = std::env::var_os(CRASH_ROOT_ENV) {
        let root = std::path::PathBuf::from(root);
        let (_catalog, store) = open_fixture(&root)?;
        let _sealed = publish(store.as_ref(), 2, "g2")?;
        return Err("the crash boundary did not fire in the child".into());
    }
    let test_name = "crash_at_seal_boundaries_converges_to_old_or_new_committed_authority";
    for (boundary, catalog_row_expected) in
        [("after-object-sync", false), ("after-catalog-commit", true)]
    {
        let dir = tempfile::tempdir()?;
        let root = dir.path().to_path_buf();
        {
            let (_catalog, store) = open_fixture(&root)?;
            let _sealed = publish(store.as_ref(), 1, "g1")?;
        }
        let status = Command::new(std::env::current_exe()?)
            .arg("--exact")
            .arg(test_name)
            .arg("--nocapture")
            .env(CRASH_ROOT_ENV, &root)
            .env(CRASH_BOUNDARY_ENV, boundary)
            .status()?;
        assert_eq!(
            status.code(),
            Some(CRASH_EXIT_CODE),
            "the child must stop exactly at {boundary}; status={status}"
        );
        // Convergence: reopen succeeds; the candidate row is either
        // absent (crash before the catalog commit) or present and
        // verifiable (crash after it). The activation authority (none
        // yet) is unchanged either way.
        let (catalog, store) = open_fixture(&root)?;
        let row = catalog.repomap_candidate_row("repo-p03", "rev-p03", 2)?;
        assert_eq!(
            row.is_some(),
            catalog_row_expected,
            "at {boundary} the catalog row presence must converge"
        );
        assert!(query(store.as_ref(), 2).is_err(), "nothing is active");
        if catalog_row_expected {
            // The committed seal survives; a retry replays it exactly.
            let replay = publish(store.as_ref(), 2, "g2")?;
            assert!(replay.replayed);
        } else {
            // The object may exist as an orphan; republishing converges to
            // the same sealed bytes and a fresh (or replayed) commit.
            let republish = publish(store.as_ref(), 2, "g2")?;
            assert!(!republish.replayed, "no catalog row existed to replay");
        }
    }
    Ok(())
}

#[test]
fn insecure_object_metadata_refuses_mutation_typed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (catalog, store) = open_fixture(&root)?;
    let _sealed = publish(store.as_ref(), 1, "g1")?;
    // Loosen the object mode: the next verification must refuse typed
    // before any activation mutation.
    let object_path = find_single_object(&root.join("repo-map").join("objects").join("sha256"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&object_path, std::fs::Permissions::from_mode(0o644))?;
    }
    let refused = activate(store.as_ref(), 1);
    assert!(refused.is_err());
    if let Err(error) = refused {
        assert!(
            matches!(error, CoreError::Typed { .. }),
            "activation must refuse typed on insecure metadata, got {error:?}"
        );
    }
    // The catalog row is untouched: no activation was committed.
    let activation = catalog.repomap_activation_row("repo-p03", "rev-p03")?;
    assert!(activation.is_none());
    Ok(())
}
