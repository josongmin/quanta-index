//! S21-02/P03 — the `RepoMap` owner's port surface under SQLite-catalog
//! authority: the bundle ingest / activation / query / quarantine ports
//! hang off one store, the quarantine is listed from the durable incident
//! rows and discarded only as listed (journaled tombstone, payload-only
//! reclaim), and a publish that names no nodes is refused before any
//! mutation.

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
use quanta_index_core::{
    CoreError, QuarantineDiscardOutcomeV1, QuarantinedRepoMapFileV1, RepoMapBundleIngestPort,
    RepoMapGenerationActivatePort, RepoMapQuarantinePort, RepoMapQueryPort,
};
use quanta_index_repomap::RepoMapGenerationStore;

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
    match RepoId::new("repo-surface") {
        Ok(repo) => repo,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    }
}

fn revision() -> RevisionId {
    match RevisionId::new("rev-surface") {
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
        line_count: 64,
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

#[expect(
    clippy::type_complexity,
    reason = "the five-port fixture tuple is the test's subject; factoring it would hide the surface under test"
)]
fn ports(
    root: &std::path::Path,
) -> Result<
    (
        Arc<SqliteCatalog>,
        Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
        Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
        Arc<dyn RepoMapQueryPort + Send + Sync>,
        Arc<dyn RepoMapQuarantinePort + Send + Sync>,
    ),
    Box<dyn Error>,
> {
    let catalog = Arc::new(SqliteCatalog::open(root, CATALOG_BUSY_BUDGET)?);
    let opened = RepoMapGenerationStore::open(root.join("repo-map"), Arc::clone(&catalog))?;
    let store = Arc::new(opened.store);
    Ok((catalog, store.clone(), store.clone(), store.clone(), store))
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

#[test]
fn the_four_ports_drive_one_store_end_to_end() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (_catalog, ingest, activate, query, quarantine) = ports(&root)?;
    let receipt = ingest.ingest_bundle(&bundle(1, "g1"))?;
    assert!(receipt.new_candidate_commitment.starts_with("sha256:"));
    assert!(query.query(query_request(1)).is_err());
    let activation = activate.activate_generation(&activate_request(1))?;
    assert_eq!(activation.activation_epoch, 1);
    let response = query.query(query_request(1))?;
    assert!(!response.entries.is_empty());
    assert!(quarantine.quarantined_files()?.is_empty());
    Ok(())
}

#[test]
fn quarantine_is_listed_from_durable_incidents_and_discarded_only_as_listed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (catalog, ingest, activate, _query, quarantine) = ports(&root)?;
    let _sealed = ingest.ingest_bundle(&bundle(1, "g1"))?;
    let _activated = activate.activate_generation(&activate_request(1))?;
    // Lose the object: the next open records a durable incident.
    let mut removed = 0_usize;
    remove_files(&root.join("repo-map").join("objects"), &mut removed)?;
    assert_eq!(removed, 1);
    drop(quarantine);
    drop(activate);
    drop(ingest);
    drop(catalog);
    let (_catalog, _ingest, _activate, _query, quarantine) = ports(&root)?;

    let mut listed = quarantine.quarantined_files()?;
    assert_eq!(listed.len(), 1, "exactly one durable incident is listed");
    let entry = listed.pop().expect("one incident");

    // A discard naming a reason the record does not carry removes
    // nothing and is refused typed.
    let stale = QuarantinedRepoMapFileV1 {
        file_name: entry.file_name.clone(),
        reason: "not-the-recorded-reason".to_string(),
    };
    match quarantine.discard_quarantined_file(&stale) {
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
            ..
        }) => {}
        other => unreachable!("a stale discard must be refused typed, got {other:?}"),
    }
    // A name that is not a recorded incident is refused typed too.
    let unknown = QuarantinedRepoMapFileV1 {
        file_name: "../escape.bin".to_string(),
        reason: entry.reason.clone(),
    };
    assert!(quarantine.discard_quarantined_file(&unknown).is_err());

    // The listed discard reclaims the payload projection only; the
    // incident/event row stays durable, and a repeat is Absent.
    match quarantine.discard_quarantined_file(&entry)? {
        QuarantineDiscardOutcomeV1::Discarded { .. } => {}
        other @ QuarantineDiscardOutcomeV1::Absent => {
            unreachable!("the listed discard reclaims, got {other:?}")
        }
    }
    match quarantine.discard_quarantined_file(&entry)? {
        QuarantineDiscardOutcomeV1::Absent => {}
        other @ QuarantineDiscardOutcomeV1::Discarded { .. } => {
            unreachable!("the repeat discard is absent, got {other:?}")
        }
    }
    Ok(())
}

#[test]
fn a_nodeless_bundle_is_refused_before_any_mutation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let (_catalog, ingest, _activate, _query, _quarantine) = ports(&root)?;
    let mut nodeless = bundle(1, "g1");
    nodeless.nodes.clear();
    match ingest.ingest_bundle(&nodeless) {
        Err(CoreError::InvalidContract(_)) => {}
        other => unreachable!("a nodeless bundle is refused, got {other:?}"),
    }
    let mut removed = 0_usize;
    remove_files(&root.join("repo-map").join("objects"), &mut removed)?;
    assert_eq!(removed, 0, "nothing was written");
    Ok(())
}
