//! S21-02/P03 — the `RepoMap` bootstrap flow under SQLite-catalog
//! authority: opening a fresh root bootstraps empty, publishing alone
//! never activates, the open report reflects the catalog reconcile, and
//! independent repo/revision pairs stay isolated.

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
use quanta_index_repomap::{OpenedRepoMapStore, RepoMapGenerationStore};

type TestResult = Result<(), Box<dyn Error>>;

/// A distinct valid 64-hex producer digest per fixture marker.
fn producer_hex(marker: &str) -> String {
    let hash = marker
        .bytes()
        .fold(0_u16, |acc, byte| acc.wrapping_add(u16::from(byte)));
    format!("{}{:04x}", "ab".repeat(30), hash)
}

const CATALOG_BUSY_BUDGET: Duration = Duration::from_secs(5);

fn repo() -> RepoId {
    match RepoId::new("repo-bootstrap") {
        Ok(repo) => repo,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    }
}

fn other_repo() -> RepoId {
    match RepoId::new("repo-other") {
        Ok(repo) => repo,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    }
}

fn revision() -> RevisionId {
    match RevisionId::new("rev-bootstrap") {
        Ok(revision) => revision,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    }
}

fn bundle(repo_id: &RepoId, generation: u64, marker: &str) -> RepoMapSourceBundle {
    RepoMapSourceBundle::new(
        repo_id.clone(),
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
        line_count: 90,
    }))
}

fn activate_request(repo_id: &RepoId, generation: u64) -> RepoMapActivateGenerationRequest {
    RepoMapActivateGenerationRequest {
        repo_id: repo_id.clone(),
        revision_id: revision(),
        manifest_generation: ManifestGeneration::new(generation),
        manifest_digest: producer_hex(&format!("g{generation}")),
    }
}

fn query_request(repo_id: &RepoId, generation: u64) -> RepoMapQueryRequest {
    RepoMapQueryRequest {
        repo_id: repo_id.clone(),
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
) -> Result<(Arc<SqliteCatalog>, OpenedRepoMapStore), Box<dyn Error>> {
    let catalog = Arc::new(SqliteCatalog::open(root, CATALOG_BUSY_BUDGET)?);
    let opened = RepoMapGenerationStore::open(root.join("repo-map"), Arc::clone(&catalog))?;
    Ok((catalog, opened))
}

#[test]
fn a_fresh_root_bootstraps_empty_and_serves_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (_catalog, opened) = open(dir.path())?;
    assert_eq!(opened.report.snapshots_loaded, 0);
    assert_eq!(opened.report.activations_loaded, 0);
    assert!(opened.report.quarantined.is_empty());
    assert!(opened.report.activations_without_snapshot.is_empty());
    match opened.store.read_query_snapshot(&query_request(&repo(), 1)) {
        Err(CoreError::NotFound(_)) => {}
        other => unreachable!("expected typed NOT_FOUND, got {other:?}"),
    }
    Ok(())
}

#[test]
fn a_published_generation_is_visible_only_after_activation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (_catalog, opened) = open(&root)?;
    let store = &opened.store;
    let _receipt = store.ingest_bundle(&bundle(&repo(), 1, "g1"))?;
    assert!(
        store
            .read_query_snapshot(&query_request(&repo(), 1))
            .is_err()
    );
    let _activation = store.activate_generation(&activate_request(&repo(), 1))?;
    assert!(
        store
            .read_query_snapshot(&query_request(&repo(), 1))
            .is_ok()
    );

    // After a restart, the same rules hold from the catalog alone.
    let (_catalog, opened) = open(&root)?;
    let store = &opened.store;
    assert_eq!(opened.report.snapshots_loaded, 1);
    assert_eq!(opened.report.activations_loaded, 1);
    assert!(
        store
            .read_query_snapshot(&query_request(&repo(), 1))
            .is_ok()
    );

    // A sealed-but-not-activated second generation stays invisible.
    let _receipt = store.ingest_bundle(&bundle(&repo(), 2, "g2"))?;
    let (_catalog, opened) = open(&root)?;
    assert_eq!(opened.report.snapshots_loaded, 2);
    assert_eq!(opened.report.activations_loaded, 1);
    assert!(
        opened
            .store
            .read_query_snapshot(&query_request(&repo(), 2))
            .is_err()
    );
    Ok(())
}

#[test]
fn repo_revisions_are_isolated_activation_keys() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (_catalog, opened) = open(&root)?;
    let store = &opened.store;
    let _first = store.ingest_bundle(&bundle(&repo(), 1, "a1"))?;
    let _second = store.ingest_bundle(&bundle(&other_repo(), 5, "b5"))?;
    let _activated = store.activate_generation(&activate_request(&other_repo(), 5))?;
    // Only the other repo's generation is active.
    assert!(
        store
            .read_query_snapshot(&query_request(&repo(), 1))
            .is_err()
    );
    assert!(
        store
            .read_query_snapshot(&query_request(&other_repo(), 5))
            .is_ok()
    );
    assert_eq!(
        store.activated_generation_for(&repo(), &revision())?,
        None,
        "the first repo has no activation of its own"
    );
    Ok(())
}

#[test]
fn an_activation_for_a_missing_candidate_object_reports_and_fail_closes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    {
        let (_catalog, opened) = open(&root)?;
        let store = &opened.store;
        let _sealed = store.ingest_bundle(&bundle(&repo(), 1, "g1"))?;
        let _activated = store.activate_generation(&activate_request(&repo(), 1))?;
    }
    // The object disappears without the catalog knowing.
    let object_root = root.join("repo-map").join("objects");
    let mut removed = 0_usize;
    remove_files(&object_root, &mut removed)?;
    assert_eq!(removed, 1);

    let (_catalog, opened) = open(&root)?;
    assert!(
        !opened.report.activations_without_snapshot.is_empty(),
        "the open report names the activation whose object is gone"
    );
    assert!(
        !opened.report.quarantined.is_empty(),
        "the loss is a durable quarantine incident"
    );
    match opened.store.read_query_snapshot(&query_request(&repo(), 1)) {
        Err(CoreError::NotFound(_)) => {}
        other => unreachable!("expected typed NOT_FOUND, got {other:?}"),
    }
    Ok(())
}

fn remove_files(dir: &std::path::Path, removed: &mut usize) -> Result<(), Box<dyn Error>> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            remove_files(&path, removed)?;
        } else {
            std::fs::remove_file(&path)?;
            *removed = removed.saturating_add(1);
        }
    }
    Ok(())
}
