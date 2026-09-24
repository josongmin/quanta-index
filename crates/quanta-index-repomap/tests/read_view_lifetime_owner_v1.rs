//! SEP-21 P04 owner tests — read-view lifetime, pins and the GC gate
//! (S21-05).
//!
//! Covers the `DoD` matrix at the authority the view acquires from:
//! - the activation/retire/query barrier: a view acquired before an
//!   activation/retire/GC completes on the same commitment it pinned;
//! - retire-wins: a new acquire of a superseded generation is a typed
//!   refusal;
//! - cancel/panic: RAII returns the pin and GC proceeds afterwards;
//! - the pin-authority GC gate: physical reclamation happens for
//!   unpinned retired objects only, and never for the active head or a
//!   sealed future generation;
//! - the attach fence: a pinned logical generation cannot have its
//!   physical artifact swapped by a republish;
//! - concurrent acquire-vs-activate never serves a mixed identity.

#![forbid(unsafe_code)]
#![expect(
    clippy::unreachable,
    reason = "test fixtures use invariant literal constructors for repo and revision IDs"
)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "integration tests use Result-returning setup with assertion-style validation"
)]

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    FileId, ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV2,
    RepoMapExactnessSummary, RepoMapFileNode, RepoMapGraphCoverage, RepoMapGraphCoverageClass,
    RepoMapItemIndexAvailability, RepoMapMutationAck, RepoMapNode, RepoMapPublishBundleRequestV2,
    RepoMapQueryRequest, RepoMapRedactionState, RepoMapSourceBundle, RepoRelativePath, RevisionId,
};
use quanta_index_core::{CoreError, PinnedRepoMapSnapshot, RepoMapSnapshotAcquireV1};
use quanta_index_repomap::{RepoMapGcOutcomeV1, RepoMapGenerationStore};

const CATALOG_BUSY_BUDGET: Duration = Duration::from_secs(5);

fn repo() -> RepoId {
    match RepoId::new("repo-p04") {
        Ok(repo) => repo,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    }
}

fn other_repo() -> RepoId {
    match RepoId::new("repo-p04-b") {
        Ok(repo) => repo,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    }
}

fn revision() -> RevisionId {
    match RevisionId::new("rev-p04") {
        Ok(revision) => revision,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    }
}

fn producer_hex(marker: &str) -> String {
    let hash = marker
        .bytes()
        .fold(0_u16, |acc, byte| acc.wrapping_add(u16::from(byte)));
    format!("{}{:04x}", "ab".repeat(30), hash)
}

fn bundle(repo_id: RepoId, generation: u64, marker: &str) -> RepoMapSourceBundle {
    RepoMapSourceBundle::new(
        repo_id,
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
}

fn acquire_request(repo_id: &RepoId, generation: u64) -> RepoMapSnapshotAcquireV1 {
    RepoMapSnapshotAcquireV1 {
        repo_id: repo_id.clone(),
        revision_id: revision(),
        manifest_generation: ManifestGeneration::new(generation),
    }
}

fn query_request(generation: u64) -> RepoMapQueryRequest {
    RepoMapQueryRequest {
        repo_id: repo(),
        revision_id: revision(),
        manifest_generation: ManifestGeneration::new(generation),
        query_text: "lib".to_string(),
        top_k: 10,
        token_budget: 1_000,
        focus_subjects: Vec::new(),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<RepoMapGenerationStore>,
    root: std::path::PathBuf,
}

fn open_fixture(
    root: &Path,
) -> Result<(Arc<SqliteCatalog>, Arc<RepoMapGenerationStore>), Box<dyn Error>> {
    let catalog = Arc::new(SqliteCatalog::open(root, CATALOG_BUSY_BUDGET)?);
    let opened = RepoMapGenerationStore::open(root.join("repo-map"), Arc::clone(&catalog))?;
    Ok((catalog, Arc::new(opened.store)))
}

fn fixture() -> Result<Fixture, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (_catalog, store) = open_fixture(&root)?;
    Ok(Fixture {
        _dir: dir,
        store,
        root,
    })
}

/// Publish and activate `generation` in one step.
fn activate(
    store: &RepoMapGenerationStore,
    generation: u64,
) -> Result<RepoMapMutationAck, CoreError> {
    let source = bundle(repo(), generation, &format!("g{generation}"));
    let publish = RepoMapPublishBundleRequestV2::new(source.clone())
        .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
    let _receipt = store.ingest_bundle_v2(&publish)?;
    let request = RepoMapActivateGenerationRequestV2::for_bundle(&source)
        .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
    Ok(store.activate_generation_v2(&request)?.mutation)
}

fn object_files(root: &Path) -> Vec<std::path::PathBuf> {
    let objects = root.join("repo-map").join("objects");
    let mut out = Vec::new();
    let mut stack = vec![objects];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

#[test]
fn an_old_view_completes_on_the_same_commitment_across_activate_and_gc() -> TestResult {
    let fixture = fixture()?;
    let store = fixture.store.as_ref();
    let first = activate(store, 1)?;
    // The view is acquired before anything else happens to the store.
    let view = store.acquire_pinned(&acquire_request(&repo(), 1))?;
    assert_eq!(
        view.evidence().candidate_commitment,
        first.new_candidate_commitment
    );
    assert_eq!(view.evidence().activation_epoch, 1);
    // Activation of generation 2 retires generation 1 in the catalog;
    // the pin the view holds must defer physical reclamation.
    let second = activate(store, 2)?;
    assert_eq!(second.activation_epoch, 2);
    let deferred = store.gc_retired_objects()?;
    assert_eq!(
        deferred,
        RepoMapGcOutcomeV1 {
            considered: 1,
            reclaimed: 0,
            deferred_pinned: 1,
        }
    );
    assert_eq!(
        store.pinned_view_count(&repo(), &revision(), ManifestGeneration::new(1))?,
        1
    );
    // The barrier: the old view still answers, on the commitment it
    // pinned, although the store moved on.
    let response = view.query(query_request(1))?;
    assert_eq!(response.manifest_generation, ManifestGeneration::new(1));
    assert!(!response.entries.is_empty());
    assert_eq!(
        view.evidence().candidate_commitment,
        first.new_candidate_commitment
    );
    Ok(())
}

#[test]
fn after_the_view_releases_gc_reclaims_the_retired_object() -> TestResult {
    let fixture = fixture()?;
    let store = fixture.store.as_ref();
    let _first = activate(store, 1)?;
    let second = activate(store, 2)?;
    // No view holds generation 1: the gate opens immediately.
    let outcome = store.gc_retired_objects()?;
    assert_eq!(outcome.reclaimed, 1);
    assert_eq!(outcome.deferred_pinned, 0);
    // One object remains: the active generation 2.
    assert_eq!(object_files(&fixture.root).len(), 1);
    // The retired generation no longer serves, the active one still does.
    let err = store
        .acquire_pinned(&acquire_request(&repo(), 1))
        .unwrap_err();
    assert!(matches!(err, CoreError::NotFound(_)), "{err:?}");
    let view = store.acquire_pinned(&acquire_request(&repo(), 2))?;
    assert_eq!(
        view.evidence().candidate_commitment,
        second.new_candidate_commitment
    );
    assert!(!view.query(query_request(2))?.entries.is_empty());
    // Reopen: reconcile treats the reclaimed invalidated row as the
    // expected terminal condition, quarantines nothing, and the active
    // head still serves.
    let (_catalog, reopened) = open_fixture(&fixture.root)?;
    let reopened = reopened.as_ref();
    assert!(reopened.gc_retired_objects()?.considered >= 1);
    let view = reopened.acquire_pinned(&acquire_request(&repo(), 2))?;
    assert!(!view.query(query_request(2))?.entries.is_empty());
    Ok(())
}

#[test]
fn retire_wins_refuses_a_new_old_generation_acquire_typed() -> TestResult {
    let fixture = fixture()?;
    let store = fixture.store.as_ref();
    let _first = activate(store, 1)?;
    let _second = activate(store, 2)?;
    let err = store
        .acquire_pinned(&acquire_request(&repo(), 1))
        .unwrap_err();
    match err {
        CoreError::NotFound(message) => {
            assert!(
                message.contains("is not the activated generation 2"),
                "{message}"
            );
        }
        CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::Storage(_) => {
            unreachable!("a superseded generation is a typed refusal")
        }
    }
    // A repo with no activation at all refuses typed as well.
    let err = store
        .acquire_pinned(&acquire_request(&other_repo(), 1))
        .unwrap_err();
    assert!(matches!(err, CoreError::NotFound(_)), "{err:?}");
    Ok(())
}

#[test]
fn panic_and_early_release_return_the_pin_and_gc_proceeds() -> TestResult {
    let fixture = fixture()?;
    let store = Arc::clone(&fixture.store);
    let _first = activate(store.as_ref(), 1)?;
    let _second = activate(store.as_ref(), 2)?;
    let generation = ManifestGeneration::new(1);
    // A panic between acquisition and completion must unwind through the
    // view's drop, returning the pin.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let view = store
            .acquire_pinned(&acquire_request(&repo(), 1))
            .expect("acquire");
        let _evidence = view.evidence().clone();
        panic!("query lane failed mid-flight");
    }));
    assert!(result.is_err());
    assert_eq!(
        store.pinned_view_count(&repo(), &revision(), generation)?,
        0
    );
    // Cancellation is the same RAII path: an early drop returns the pin.
    {
        let _view = store.acquire_pinned(&acquire_request(&repo(), 2))?;
        assert_eq!(
            store.pinned_view_count(&repo(), &revision(), ManifestGeneration::new(2))?,
            1
        );
    }
    assert_eq!(
        store.pinned_view_count(&repo(), &revision(), ManifestGeneration::new(2))?,
        0
    );
    // With the pin back at baseline, GC reclaims the retired object.
    let outcome = store.gc_retired_objects()?;
    assert_eq!(outcome.reclaimed, 1);
    assert_eq!(outcome.deferred_pinned, 0);
    Ok(())
}

#[test]
fn a_pinned_generation_cannot_be_republished_underneath_a_view() -> TestResult {
    let fixture = fixture()?;
    let store = fixture.store.as_ref();
    let _first = activate(store, 1)?;
    let view = store.acquire_pinned(&acquire_request(&repo(), 1))?;
    // Same logical identity, different bytes: immutable candidate custody
    // refuses before the attach fence; the pinned view stays unchanged.
    let err = store
        .ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(bundle(
            repo(),
            1,
            "g1-replacement",
        ))?)
        .unwrap_err();
    match err {
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::CandidateCommitmentConflict,
            ..
        } => {}
        CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_) => {
            unreachable!("a different request for one immutable candidate must conflict")
        }
    }
    // The view still serves the artifact it pinned.
    assert!(!view.query(query_request(1))?.entries.is_empty());
    drop(view);
    // After release the pin gate no longer refuses; the catalog's own
    // content-bound CAS is then the authority over the republish.
    let post_release = store.ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(bundle(
        repo(),
        1,
        "g1-replacement",
    ))?);
    assert!(
        post_release.is_err(),
        "the catalog CAS still owns the conflict"
    );
    Ok(())
}

#[test]
fn gc_leaves_the_active_head_and_sealed_future_generations_alone() -> TestResult {
    let fixture = fixture()?;
    let store = fixture.store.as_ref();
    let _first = activate(store, 1)?;
    let _second = activate(store, 2)?;
    // A sealed-but-not-activated future generation must survive GC.
    let _g3 = store.ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(bundle(
        repo(),
        3,
        "g3",
    ))?)?;
    let before = object_files(&fixture.root).len();
    assert_eq!(before, 3);
    let outcome = store.gc_retired_objects()?;
    assert_eq!(outcome.considered, 1);
    assert_eq!(outcome.reclaimed, 1);
    assert_eq!(object_files(&fixture.root).len(), 2);
    // Churn in repo A never touches repo B's objects.
    let _b7 = store.ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(bundle(
        other_repo(),
        7,
        "b7",
    ))?)?;
    let _b8 = store.ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(bundle(
        other_repo(),
        8,
        "b8",
    ))?)?;
    let _view = store.acquire_pinned(&acquire_request(&repo(), 2))?;
    let outcome = store.gc_retired_objects()?;
    assert_eq!(outcome.reclaimed, 0);
    Ok(())
}

#[test]
fn concurrent_acquire_during_activation_never_serves_a_mixed_identity() -> TestResult {
    let fixture = fixture()?;
    let store = Arc::clone(&fixture.store);
    let _first = activate(store.as_ref(), 1)?;
    let first = store
        .acquire_pinned(&acquire_request(&repo(), 1))?
        .evidence()
        .clone();
    drop(first);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let acquire_barrier = Arc::clone(&barrier);
    let acquirer_store = Arc::clone(&store);
    let acquirer = std::thread::spawn(move || {
        let _synced = acquire_barrier.wait();
        let mut seen = Vec::new();
        let store = acquirer_store.as_ref();
        for _ in 0..64 {
            match store.acquire_pinned(&acquire_request(&repo(), 1)) {
                Ok(view) => seen.push(Ok(view.evidence().clone())),
                Err(err) => seen.push(Err(err.to_string())),
            }
        }
        seen
    });
    let _synced = barrier.wait();
    let _second = activate(store.as_ref(), 2)?;
    let seen = acquirer.join().expect("acquirer must not panic");
    assert!(!seen.is_empty());
    for outcome in seen {
        match outcome {
            // A view that won the race carries exactly the generation-1
            // identity and its epoch; the commitment must be a real
            // sealed commitment, never a mix.
            Ok(evidence) => {
                assert_eq!(evidence.manifest_generation, 1);
                assert_eq!(evidence.activation_epoch, 1);
                assert!(evidence.candidate_commitment.starts_with("sha256:"));
            }
            // A view that lost the race is refused typed.
            Err(message) => {
                assert!(
                    message.contains("is not the activated generation")
                        || message.contains("no activated generation"),
                    "{message}"
                );
            }
        }
    }
    Ok(())
}

type TestResult = Result<(), Box<dyn Error>>;
