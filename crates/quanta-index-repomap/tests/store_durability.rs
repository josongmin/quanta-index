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
use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;
use std::time::Duration;

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    CandidateObjectDigestV1, FileId, LogicalGenerationIdentityV1, ManifestGeneration,
    QuarantinePayloadDigestV1, RepoId, RepoMapActivateGenerationRequestV2, RepoMapExactnessSummary,
    RepoMapFileNode, RepoMapGraphCoverage, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability,
    RepoMapNode, RepoMapPublishBundleRequestV2, RepoMapQueryRequest, RepoMapRedactionState,
    RepoMapSourceBundle, RepoRelativePath, RepositoryRevisionIdentityV1, RevisionId,
};
use quanta_index_core::CoreError;
use quanta_index_repomap::{
    CandidateObjectAddressV1, RepoMapGenerationStore, RepoMapGraphCompiler,
};

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

fn publish_activate(store: &RepoMapGenerationStore, generation: u64, marker: &str) -> TestResult {
    let source = bundle(generation, marker);
    let _sealed = store.ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(source.clone())?)?;
    let mut request = RepoMapActivateGenerationRequestV2::for_bundle(&source)?;
    request.expected_active = store.active_head_token(&repo(), &revision())?;
    let _activated = store.activate_generation_v2(&request)?;
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

/// One canonical candidate envelope for `generation`, built from the
/// compiler the store itself uses, without any store or catalog.
fn canonical_envelope(
    generation: u64,
    marker: &str,
) -> Result<quanta_index_contract::RepoMapCandidateEnvelopeV1, Box<dyn Error>> {
    let source = bundle(generation, marker);
    let candidate = RepoMapGraphCompiler::with_default_budget()
        .compile(&source)
        .map_err(|refusal| format!("the fixture bundle compiles: {refusal}"))?;
    let identity = LogicalGenerationIdentityV1::new(
        RepositoryRevisionIdentityV1::new(source.repo_id.clone(), source.revision_id.clone()),
        source.manifest_generation.get(),
    );
    Ok(candidate.envelope(identity)?)
}

fn object_relative_path(address: CandidateObjectDigestV1) -> String {
    CandidateObjectAddressV1::new(address)
        .relative_path()
        .to_string_lossy()
        .into_owned()
}

/// Write `bytes` at their own content address and seal a catalog row that
/// claims them.
///
/// This is the one shape the publish path cannot produce: it is how a
/// hand-written (or importer-written) object and a disagreeing row reach
/// the owner, so the owner's reconcile can be asked what it does with
/// each disagreement.
fn seal_object_bytes(
    root: &std::path::Path,
    generation: u64,
    bytes: &[u8],
    commitment: &[u8; 32],
) -> Result<CandidateObjectDigestV1, Box<dyn Error>> {
    let catalog = SqliteCatalog::open(root, CATALOG_BUSY_BUDGET)?;
    let address = CandidateObjectDigestV1::for_canonical_envelope(bytes);
    let relative = object_relative_path(address);
    let path = root.join("repo-map").join(&relative);
    // The layout's own modes: the root and every fanout directory 0700, the
    // object 0600 — what the store's security context verifies on each read.
    let layout_root = root.join("repo-map");
    std::fs::create_dir_all(&layout_root)?;
    std::fs::set_permissions(&layout_root, std::fs::Permissions::from_mode(0o700))?;
    let mut current = layout_root;
    let components = std::path::Path::new(&relative)
        .components()
        .collect::<Vec<_>>();
    for component in components
        .split_last()
        .map(|(_leaf, directories)| directories)
        .unwrap_or_default()
    {
        current.push(component);
        if !current.exists() {
            std::fs::create_dir(&current)?;
            std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    std::fs::write(&path, bytes)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let _sealed = catalog.seal_repomap_candidate(
        repo().as_str(),
        revision().as_str(),
        generation,
        commitment,
        address.as_bytes(),
        &[3_u8; 32],
        u64::try_from(bytes.len())?,
        "{}",
    )?;
    Ok(address)
}

#[test]
fn a_quarantine_incident_names_the_defect_not_only_that_the_object_failed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let canonical = canonical_envelope(1, "g1")?.encode_canonical()?;
    // Generation 1: the canonical bytes plus one trailing byte. The payload
    // is exactly what its own address claims, so the defect is the
    // encoding (contract reason 2), never a swap.
    let mut non_canonical = canonical.clone();
    non_canonical.push(0);
    let non_canonical_address = seal_object_bytes(&root, 1, &non_canonical, &[7_u8; 32])?;
    // Generation 2: canonical bytes at their own address, with only the
    // catalog row's commitment disagreeing (contract reason 4).
    let canonical_address = seal_object_bytes(&root, 2, &canonical, &[8_u8; 32])?;
    assert_ne!(non_canonical_address, canonical_address);

    let (catalog, _store) = open(&root)?;
    let mut listed: Vec<(String, String)> = catalog
        .repomap_quarantine_incidents()?
        .into_iter()
        .map(|incident| (incident.source_path, incident.reason_code))
        .collect();
    listed.sort();
    let mut expected = vec![
        (
            object_relative_path(non_canonical_address),
            "quarantine-reason-2".to_string(),
        ),
        (
            object_relative_path(canonical_address),
            "quarantine-reason-4".to_string(),
        ),
    ];
    expected.sort();
    assert_eq!(listed, expected);
    // Each incident kept the payload it read — the plain digest of the
    // bytes on disk — so an operator can tell a swapped object from a
    // non-canonical one without the bytes themselves.
    let incidents = catalog.repomap_quarantine_incidents()?;
    assert_eq!(incidents.len(), 2);
    for incident in &incidents {
        let payload = if incident.source_path == object_relative_path(non_canonical_address) {
            &non_canonical
        } else {
            &canonical
        };
        assert_eq!(
            incident.payload_digest,
            *QuarantinePayloadDigestV1::for_payload(payload).as_bytes()
        );
    }
    Ok(())
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
    assert!(
        store
            .as_ref()
            .ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(empty_digest)?)
            .is_err()
    );
    let mut no_nodes = bundle(1, "g1");
    no_nodes.nodes.clear();
    assert!(
        store
            .as_ref()
            .ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(no_nodes)?)
            .is_err()
    );
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
