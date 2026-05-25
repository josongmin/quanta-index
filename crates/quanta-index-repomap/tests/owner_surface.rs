#![forbid(unsafe_code)]

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapActivateGenerationRequest, RepoMapChunkRecordDto,
    RepoMapFileIndexRecord, RepoMapFocusSubjectDto, RepoMapGraphEdgeDto, RepoMapQueryRequest,
    RepoMapSourceBundle, RevisionId,
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

fn source_bundle() -> RepoMapSourceBundle {
    RepoMapSourceBundle {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: manifest_generation(),
        snapshot_id: "snapshot-17".to_string(),
        projection_version: 3,
        authority_digest: "auth-digest-17".to_string(),
        item_index_availability: "full".to_string(),
        graph_coverage_class: "complete".to_string(),
        exactness_summary: "owner-surface-exact".to_string(),
        redaction_state: "Unredacted".to_string(),
        file_indices: vec![
            RepoMapFileIndexRecord {
                file_identity: "src/lib.rs".to_string(),
                file_path: "src/lib.rs".to_string(),
                file_kind: "library".to_string(),
                line_count: 200,
                symbol_records: Vec::new(),
            },
            RepoMapFileIndexRecord {
                file_identity: "src/main.rs".to_string(),
                file_path: "src/main.rs".to_string(),
                file_kind: "binary".to_string(),
                line_count: 120,
                symbol_records: Vec::new(),
            },
            RepoMapFileIndexRecord {
                file_identity: "src/http.rs".to_string(),
                file_path: "src/http.rs".to_string(),
                file_kind: "http".to_string(),
                line_count: 90,
                symbol_records: Vec::new(),
            },
        ],
        call_edges: vec![
            RepoMapGraphEdgeDto {
                from_identity: "src/lib.rs".to_string(),
                to_identity: "src/main.rs".to_string(),
                edge_kind: "call".to_string(),
            },
            RepoMapGraphEdgeDto {
                from_identity: "src/lib.rs".to_string(),
                to_identity: "src/http.rs".to_string(),
                edge_kind: "call".to_string(),
            },
        ],
        import_edges: vec![RepoMapGraphEdgeDto {
            from_identity: "src/main.rs".to_string(),
            to_identity: "src/lib.rs".to_string(),
            edge_kind: "import".to_string(),
        }],
        chunk_records: vec![
            RepoMapChunkRecordDto {
                subject_identity: "src/lib.rs".to_string(),
                owner_path: "src/lib.rs".to_string(),
                token_count: 120,
                preview_text: "owner path library orchestrates query ranking".to_string(),
                exactness: "Exact".to_string(),
            },
            RepoMapChunkRecordDto {
                subject_identity: "src/main.rs".to_string(),
                owner_path: "src/main.rs".to_string(),
                token_count: 84,
                preview_text: "main entrypoint owner path".to_string(),
                exactness: "Exact".to_string(),
            },
            RepoMapChunkRecordDto {
                subject_identity: "src/http.rs".to_string(),
                owner_path: "src/http.rs".to_string(),
                token_count: 56,
                preview_text: "http owner surface fallback".to_string(),
                exactness: "Approximate".to_string(),
            },
        ],
    }
}

fn activate(store: &RepoMapGenerationStore) -> Result<(), CoreError> {
    let bundle = source_bundle();
    store.activate_generation(&RepoMapActivateGenerationRequest {
        repo_id: bundle.repo_id,
        revision_id: bundle.revision_id,
        manifest_generation: bundle.manifest_generation,
        manifest_digest: "manifest-digest-17".to_string(),
    })
}

fn query_request(focus_subjects: Vec<RepoMapFocusSubjectDto>) -> RepoMapQueryRequest {
    RepoMapQueryRequest {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: manifest_generation(),
        query_text: "owner path".to_string(),
        top_k: 2,
        token_budget: 256,
        focus_subjects,
    }
}

fn assert_not_found_contains(error: CoreError, needle: &str) {
    match error {
        CoreError::NotFound(message) => assert!(message.contains(needle)),
        CoreError::InvalidContract(message) => {
            assert!(false, "expected NotFound, got InvalidContract({message})");
        }
        CoreError::Typed { code, message } => {
            assert!(false, "expected NotFound, got Typed({code}, {message})");
        }
        CoreError::NotReady(message) => {
            assert!(false, "expected NotFound, got NotReady({message})");
        }
        CoreError::NotImplemented(message) => {
            assert!(false, "expected NotFound, got NotImplemented({message})");
        }
        CoreError::Storage(message) => {
            assert!(false, "expected NotFound, got Storage({message})");
        }
    }
}

#[test]
fn ingest_bundle_materializes_snapshot_and_serves_query() {
    let store = RepoMapGenerationStore::default();
    let bundle = source_bundle();

    let ingest_result = store.ingest_bundle(&bundle);
    assert!(
        ingest_result.is_ok(),
        "bundle ingest should succeed: {ingest_result:?}"
    );
    let activation_result = activate(&store);
    assert!(
        activation_result.is_ok(),
        "activate should succeed: {activation_result:?}"
    );

    let response_result = store.read_query_snapshot(&query_request(vec![RepoMapFocusSubjectDto {
        subject_identity: "src/lib.rs".to_string(),
        subject_doc_type: "File".to_string(),
    }]));
    assert!(
        response_result.is_ok(),
        "query should succeed against materialized snapshot: {response_result:?}"
    );
    let Ok(response) = response_result else {
        return;
    };

    assert_eq!(response.repo_id, bundle.repo_id);
    assert_eq!(response.revision_id, bundle.revision_id);
    assert_eq!(response.manifest_generation, bundle.manifest_generation);
    assert_eq!(response.snapshot_meta.snapshot_id, "snapshot-17");
    assert_eq!(response.snapshot_meta.projection_version, 3);
    assert_eq!(response.snapshot_meta.authority_digest, "auth-digest-17");
    assert_eq!(response.entries.len(), 1);
    assert_eq!(
        response
            .entries
            .first()
            .map(|entry| entry.subject_identity.as_str()),
        Some("src/lib.rs")
    );
    assert_eq!(
        response
            .entries
            .first()
            .map(|entry| entry.owner_path.as_str()),
        Some("src/lib.rs")
    );
    assert_eq!(response.entries.first().map(|entry| entry.rank), Some(1));
}

#[test]
fn query_without_focus_returns_rank_sorted_entries_with_budget_cap() {
    let store = RepoMapGenerationStore::default();
    let bundle = source_bundle();

    let ingest_result = store.ingest_bundle(&bundle);
    assert!(
        ingest_result.is_ok(),
        "bundle ingest should succeed: {ingest_result:?}"
    );
    let activation_result = activate(&store);
    assert!(
        activation_result.is_ok(),
        "activate should succeed: {activation_result:?}"
    );

    let response_result = store.read_query_snapshot(&query_request(Vec::new()));
    assert!(
        response_result.is_ok(),
        "query should succeed without focus filter: {response_result:?}"
    );
    let Ok(response) = response_result else {
        return;
    };

    assert_eq!(response.entries.len(), 3);
    assert_eq!(
        response
            .entries
            .first()
            .map(|entry| entry.subject_identity.as_str()),
        Some("src/lib.rs")
    );
    assert_eq!(response.entries.first().map(|entry| entry.rank), Some(1));
    assert_eq!(
        response
            .entries
            .get(1)
            .map(|entry| entry.subject_identity.as_str()),
        Some("src/main.rs")
    );
    assert_eq!(response.entries.get(1).map(|entry| entry.rank), Some(2));
    assert_eq!(
        response
            .entries
            .get(2)
            .map(|entry| entry.subject_identity.as_str()),
        Some("src/http.rs")
    );
    assert_eq!(
        response.entries.get(2).map(|entry| entry.included),
        Some(false)
    );
    assert_eq!(response.entries.get(2).map(|entry| entry.rank), Some(0));
    assert_eq!(response.dropped_entries_count, 1);
}

#[test]
fn missing_snapshot_fails_closed_with_not_found() {
    let store = RepoMapGenerationStore::default();

    let query_result = store.read_query_snapshot(&query_request(Vec::new()));
    assert!(
        query_result.is_err(),
        "query should fail when snapshot was never materialized: {query_result:?}"
    );
    let Err(error) = query_result else {
        return;
    };
    assert_not_found_contains(error, "no activated generation");
}
