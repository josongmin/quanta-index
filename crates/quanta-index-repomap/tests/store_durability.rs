//! S21-02/P03 — the `RepoMap` store's durability under SQLite-catalog
//! authority: sealed candidates and activations survive restart, a
//! damaged candidate object is durably quarantined and answered
//! fail-closed instead of failing the open, and supersede keeps old
//! objects (physical GC is tombstone-only/disabled before P04).

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
use std::sync::Arc;
use std::time::Duration;

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    FileId, ManifestGeneration, RepoId, RepoMapActivateGenerationRequest, RepoMapExactnessSummary,
    RepoMapFileNode, RepoMapGraphCoverage, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability,
    RepoMapNode, RepoMapQueryRequest, RepoMapRedactionState, RepoMapSourceBundle, RepoRelativePath,
    RevisionId,
};
use quanta_index_core::CoreError;
use quanta_index_repomap::RepoMapGenerationStore;

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

const CATALOG_BUSY_BUDGET: Duration = Duration::from_secs(5);

fn repo() -> RepoId {
    match RepoId::new("repo-durable") {
        Ok(repo) => repo,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    }
}

fn revision() -> RevisionId {
    match RevisionId::new("rev-durable") {
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
        file_id: FileId::new("file://src/main.rs"),
        repo_relative_path: RepoRelativePath::new("src/main.rs"),
        line_count: 120,
    }))
}

fn activate_request(generation: u64) -> RepoMapActivateGenerationRequest {
    RepoMapActivateGenerationRequest {
        repo_id: repo(),
        revision_id: revision(),
        manifest_generation: ManifestGeneration::new(generation),
        manifest_digest: producer_hex(&format!("g{generation}")),
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

fn open(
    root: &std::path::Path,
) -> Result<(Arc<SqliteCatalog>, Arc<RepoMapGenerationStore>), Box<dyn Error>> {
    let catalog = Arc::new(SqliteCatalog::open(root, CATALOG_BUSY_BUDGET)?);
    let opened = RepoMapGenerationStore::open(root.join("repo-map"), Arc::clone(&catalog))?;
    Ok((catalog, Arc::new(opened.store)))
}

fn publish_activate(
    store: &RepoMapGenerationStore,
    generation: u64,
    marker: &str,
) -> Result<(), CoreError> {
    let _sealed = store.ingest_bundle(&bundle(generation, marker))?;
    let _activated = store.activate_generation(&activate_request(generation))?;
    Ok(())
}

fn find_objects(root: &std::path::Path) -> Result<Vec<std::path::PathBuf>, Box<dyn Error>> {
    let mut out = Vec::new();
    let objects = root.join("repo-map").join("objects");
    if !objects.exists() {
        return Ok(out);
    }
    collect_files(&objects, &mut out)?;
    Ok(out)
}

fn collect_files(
    dir: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) -> Result<(), Box<dyn Error>> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

#[test]
fn activation_is_durable_across_restart() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    {
        let (_catalog, store) = open(&root)?;
        publish_activate(store.as_ref(), 1, "g1")?;
        let answer = read_query_snapshot(store.as_ref(), &query_request(1))?;
        assert!(!answer.entries.is_empty());
    }
    let (_catalog, store) = open(&root)?;
    assert_eq!(
        store
            .as_ref()
            .activated_generation_for(&repo(), &revision())?,
        Some(1),
        "the activation survives the restart from the catalog"
    );
    let answer = read_query_snapshot(store.as_ref(), &query_request(1))?;
    assert_eq!(answer.snapshot_meta.snapshot_id, "snap-g1");
    assert!(!answer.entries.is_empty());
    Ok(())
}

#[test]
fn a_damaged_candidate_is_quarantined_and_the_rest_answers_fail_closed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    {
        let (_catalog, store) = open(&root)?;
        publish_activate(store.as_ref(), 1, "g1")?;
    }
    // Bit-rot the sealed object: decode or digest verification fails.
    let objects = find_objects(&root)?;
    assert_eq!(objects.len(), 1);
    let object = objects.first().expect("one object").clone();
    let bytes = std::fs::read(&object)?;
    let mut damaged = bytes;
    if let Some(last) = damaged.last_mut() {
        *last = last.wrapping_add(1);
    }
    std::fs::write(&object, &damaged)?;

    let (catalog, store) = open(&root)?;
    // The open succeeded; the repo answers fail-closed until a fresh
    // activation, which is the answer a damaged object earns.
    match read_query_snapshot(store.as_ref(), &query_request(1)) {
        Err(CoreError::NotFound(_)) => {}
        other => unreachable!("expected typed NOT_FOUND, got {other:?}"),
    }
    let activation = catalog
        .repomap_activation_row(repo().as_str(), revision().as_str())?
        .expect("the activation row survives, invalidated");
    assert!(!activation.active);
    // The next open is clean: the damaged object is quarantined, not left
    // to serve.
    let (_catalog, store) = open(&root)?;
    assert!(read_query_snapshot(store.as_ref(), &query_request(1)).is_err());
    assert!(!object.exists(), "the damaged source object is gone");
    Ok(())
}

#[test]
fn supersede_keeps_the_prior_object_gc_is_tombstone_only_pre_p04() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    {
        let (_catalog, store) = open(&root)?;
        publish_activate(store.as_ref(), 1, "g1")?;
        publish_activate(store.as_ref(), 2, "g2")?;
    }
    // Physical GC refuses deletion without pinned-handle/rollback-proof
    // (P04): both sealed objects stay on disk after the supersede.
    let objects = find_objects(&root)?;
    assert_eq!(
        objects.len(),
        2,
        "both generations' objects survive; GC is tombstone-only pre-P04"
    );
    let (_catalog, store) = open(&root)?;
    assert!(read_query_snapshot(store.as_ref(), &query_request(2)).is_ok());
    assert!(read_query_snapshot(store.as_ref(), &query_request(1)).is_err());
    Ok(())
}

#[test]
fn publish_refuses_malformed_bundles_with_zero_mutation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (catalog, store) = open(&root)?;
    let mut empty_digest = bundle(1, "g1");
    empty_digest.manifest_digest = String::new();
    assert!(store.as_ref().ingest_bundle(&empty_digest).is_err());
    let mut no_nodes = bundle(1, "g1");
    no_nodes.nodes.clear();
    assert!(store.as_ref().ingest_bundle(&no_nodes).is_err());
    assert!(
        find_objects(&root)?.is_empty(),
        "a refused publish mutates nothing on disk"
    );
    assert!(
        catalog
            .repomap_candidate_row(repo().as_str(), revision().as_str(), 1)?
            .is_none(),
        "a refused publish mutates nothing in the catalog"
    );
    Ok(())
}
