#![forbid(unsafe_code)]

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    FileId, ManifestGeneration, RepoId, RepoMapActivateGenerationRequest, RepoMapChunkExactness,
    RepoMapDocType, RepoMapExactnessSummary, RepoMapFocusSubjectDto, RepoMapGraphCoverage,
    RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapNode, RepoMapNodeRef,
    RepoMapOwnsChunkEdge, RepoMapQueryRequest, RepoMapRedactionState, RepoMapSourceBundle,
    RepoRelativePath, RevisionId,
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
    RepoMapSourceBundle::new(
        repo_id(),
        revision_id(),
        manifest_generation(),
        "manifest-digest-17",
        "snapshot-17",
        3,
        "auth-digest-17",
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Full,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
    .with_node(RepoMapNode::File(quanta_index_contract::RepoMapFileNode {
        file_id: FileId::new("src/lib.rs"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        line_count: 200,
    }))
    .with_node(RepoMapNode::File(quanta_index_contract::RepoMapFileNode {
        file_id: FileId::new("src/main.rs"),
        repo_relative_path: RepoRelativePath::new("src/main.rs"),
        line_count: 120,
    }))
    .with_node(RepoMapNode::File(quanta_index_contract::RepoMapFileNode {
        file_id: FileId::new("src/http.rs"),
        repo_relative_path: RepoRelativePath::new("src/http.rs"),
        line_count: 90,
    }))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://lib"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            language: LanguageCode::new("rust").expect("valid language"),
            start_byte: 0,
            end_byte: 100,
            start_line: 1,
            end_line: 10,
            token_count: 120,
            preview_text: "owner path library orchestrates query ranking".to_string(),
            exactness: RepoMapChunkExactness::Exact,
        },
    ))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://main"),
            owner_path: RepoRelativePath::new("src/main.rs"),
            language: LanguageCode::new("rust").expect("valid language"),
            start_byte: 101,
            end_byte: 180,
            start_line: 11,
            end_line: 18,
            token_count: 84,
            preview_text: "main entrypoint owner path".to_string(),
            exactness: RepoMapChunkExactness::Exact,
        },
    ))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://http"),
            owner_path: RepoRelativePath::new("src/http.rs"),
            language: LanguageCode::new("rust").expect("valid language"),
            start_byte: 181,
            end_byte: 240,
            start_line: 19,
            end_line: 24,
            token_count: 56,
            preview_text: "http owner surface fallback".to_string(),
            exactness: RepoMapChunkExactness::Approximate,
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Call(
        quanta_index_contract::RepoMapCallEdge {
            caller: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
            callee: RepoMapNodeRef::File(FileId::new("src/main.rs")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Call(
        quanta_index_contract::RepoMapCallEdge {
            caller: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
            callee: RepoMapNodeRef::File(FileId::new("src/http.rs")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Import(
        quanta_index_contract::RepoMapImportEdge {
            importer: RepoMapNodeRef::File(FileId::new("src/main.rs")),
            imported: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
            chunk: RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new("chunk://lib")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::File(FileId::new("src/main.rs")),
            chunk: RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new("chunk://main")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::File(FileId::new("src/http.rs")),
            chunk: RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new("chunk://http")),
        },
    ))
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
        subject_doc_type: RepoMapDocType::File,
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
