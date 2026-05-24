#![forbid(unsafe_code)]

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapFocusSubjectDtoV1, RepoMapQueryRequestV1,
    RepoMapSourceBundleV1, RevisionId,
};
use quanta_index_core::CoreError;
use quanta_index_repomap::RepoMapGenerationStore;

fn repo_id() -> RepoId {
    RepoId::new("repo-map-owner-test")
}

fn revision_id() -> RevisionId {
    RevisionId::new("rev-owner-test")
}

fn manifest_generation() -> ManifestGeneration {
    ManifestGeneration::new(17)
}

fn source_bundle() -> RepoMapSourceBundleV1 {
    RepoMapSourceBundleV1 {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: manifest_generation(),
        snapshot_id: "snapshot-17".to_string(),
        projection_version: 3,
        authority_digest: "auth-digest-17".to_string(),
        item_index_availability: "full".to_string(),
        graph_coverage_class: "complete".to_string(),
        exactness_summary: "owner-surface".to_string(),
        entry_identities: vec![
            "src/lib.rs::AlphaNode".to_string(),
            "src/lib.rs::BetaNode".to_string(),
            "src/main.rs::GammaNode".to_string(),
        ],
    }
}

fn query_request(focus_subjects: Vec<RepoMapFocusSubjectDtoV1>) -> RepoMapQueryRequestV1 {
    RepoMapQueryRequestV1 {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: manifest_generation(),
        query_text: "owner path".to_string(),
        top_k: 2,
        token_budget: 128,
        focus_subjects,
    }
}

#[test]
fn ingest_bundle_materializes_snapshot_and_serves_query() {
    let store = RepoMapGenerationStore::default();
    let bundle = source_bundle();

    store
        .ingest_bundle(&bundle)
        .expect("bundle ingest should succeed");

    let response = store
        .read_query_snapshot(&query_request(vec![RepoMapFocusSubjectDtoV1 {
            subject_identity: "src/lib.rs::BetaNode".to_string(),
            subject_doc_type: "Symbol".to_string(),
        }]))
        .expect("query should succeed against materialized snapshot");

    assert_eq!(response.repo_id, bundle.repo_id);
    assert_eq!(response.revision_id, bundle.revision_id);
    assert_eq!(response.manifest_generation, bundle.manifest_generation);
    assert_eq!(response.snapshot_meta.snapshot_id, "snapshot-17");
    assert_eq!(response.snapshot_meta.projection_version, 3);
    assert_eq!(response.snapshot_meta.authority_digest, "auth-digest-17");
    assert_eq!(response.entries.len(), 1);
    assert_eq!(response.entries[0].subject_identity, "src/lib.rs::BetaNode");
    assert_eq!(response.entries[0].owner_path, "src/lib.rs");
    assert_eq!(response.entries[0].rank, 1);
}

#[test]
fn query_without_focus_returns_rank_sorted_entries_with_budget_cap() {
    let store = RepoMapGenerationStore::default();

    store
        .ingest_bundle(&source_bundle())
        .expect("bundle ingest should succeed");

    let response = store
        .read_query_snapshot(&query_request(Vec::new()))
        .expect("query should succeed without focus filter");

    assert_eq!(response.entries.len(), 3);
    assert_eq!(
        response.entries[0].subject_identity,
        "src/lib.rs::AlphaNode"
    );
    assert_eq!(response.entries[0].rank, 1);
    assert_eq!(response.entries[1].subject_identity, "src/lib.rs::BetaNode");
    assert_eq!(response.entries[1].rank, 2);
    assert_eq!(response.entries[2].subject_identity, "src/main.rs::GammaNode");
    assert!(!response.entries[2].included);
    assert_eq!(response.entries[2].rank, 0);
    assert_eq!(response.dropped_entries_count, 1);
}

#[test]
fn missing_snapshot_fails_closed_with_not_found() {
    let store = RepoMapGenerationStore::default();

    let err = store
        .read_query_snapshot(&query_request(Vec::new()))
        .expect_err("query should fail when snapshot was never materialized");

    match err {
        CoreError::NotFound(message) => {
            assert!(message.contains("repomap snapshot missing"));
            assert!(message.contains("repo=repo-map-owner-test"));
            assert!(message.contains("revision=rev-owner-test"));
            assert!(message.contains("generation=17"));
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}
