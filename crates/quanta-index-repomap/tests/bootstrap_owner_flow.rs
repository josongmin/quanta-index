use std::fs;

use quanta_index_contract::lex::{LanguageCode, SymbolKindCode};
use quanta_index_contract::{
    FileId, ManifestGeneration, RepoId, RepoMapActivateGenerationRequest, RepoMapChunkExactness,
    RepoMapDocType, RepoMapExactnessSummary, RepoMapFocusSubjectDto, RepoMapGraphCoverage,
    RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapNode, RepoMapNodeRef,
    RepoMapQueryRequest, RepoMapRedactionState, RepoMapSourceBundle, RepoRelativePath, RevisionId,
    SymbolId,
};
use quanta_index_core::CoreError;
use quanta_index_repomap::RepoMapGenerationStore;

fn sample_bundle() -> RepoMapSourceBundle {
    RepoMapSourceBundle::new(
        RepoId::new("repo-a"),
        RevisionId::new("rev-a"),
        ManifestGeneration::new(7),
        "manifest-digest-7",
        "snap-7",
        1,
        "digest-7",
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
    .with_node(RepoMapNode::File(quanta_index_contract::RepoMapFileNode {
        file_id: FileId::new("src/lib.rs"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        line_count: 140,
    }))
    .with_node(RepoMapNode::File(quanta_index_contract::RepoMapFileNode {
        file_id: FileId::new("src/runtime/mod.rs"),
        repo_relative_path: RepoRelativePath::new("src/runtime/mod.rs"),
        line_count: 96,
    }))
    .with_node(RepoMapNode::File(quanta_index_contract::RepoMapFileNode {
        file_id: FileId::new("tests/repomap.rs"),
        repo_relative_path: RepoRelativePath::new("tests/repomap.rs"),
        line_count: 64,
    }))
    .with_node(RepoMapNode::Symbol(
        quanta_index_contract::RepoMapSymbolNode {
            symbol_id: SymbolId::new("src/lib.rs::OwnerAlpha"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            local_name: "OwnerAlpha".to_string(),
            qualified_name: "src::lib::OwnerAlpha".to_string(),
            symbol_kind: SymbolKindCode::new("service").expect("valid symbol kind"),
        },
    ))
    .with_node(RepoMapNode::Symbol(
        quanta_index_contract::RepoMapSymbolNode {
            symbol_id: SymbolId::new("src/lib.rs::OwnerBeta"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            local_name: "OwnerBeta".to_string(),
            qualified_name: "src::lib::OwnerBeta".to_string(),
            symbol_kind: SymbolKindCode::new("struct").expect("valid symbol kind"),
        },
    ))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://alpha"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            language: LanguageCode::new("rust").expect("valid language"),
            start_byte: 0,
            end_byte: 90,
            start_line: 1,
            end_line: 9,
            token_count: 80,
            preview_text: "OwnerAlpha coordinates repo map ownership".to_string(),
            exactness: RepoMapChunkExactness::Exact,
        },
    ))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://beta"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            language: LanguageCode::new("rust").expect("valid language"),
            start_byte: 91,
            end_byte: 150,
            start_line: 10,
            end_line: 16,
            token_count: 56,
            preview_text: "OwnerBeta carries owner surface metadata".to_string(),
            exactness: RepoMapChunkExactness::Exact,
        },
    ))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://runtime"),
            owner_path: RepoRelativePath::new("src/runtime/mod.rs"),
            language: LanguageCode::new("rust").expect("valid language"),
            start_byte: 151,
            end_byte: 200,
            start_line: 17,
            end_line: 22,
            token_count: 40,
            preview_text: "runtime module activation path".to_string(),
            exactness: RepoMapChunkExactness::Approximate,
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Contains(
        quanta_index_contract::RepoMapContainsEdge {
            container: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
            contained: RepoMapNodeRef::Symbol(SymbolId::new("src/lib.rs::OwnerAlpha")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Contains(
        quanta_index_contract::RepoMapContainsEdge {
            container: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
            contained: RepoMapNodeRef::Symbol(SymbolId::new("src/lib.rs::OwnerBeta")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Call(
        quanta_index_contract::RepoMapCallEdge {
            caller: RepoMapNodeRef::Symbol(SymbolId::new("src/lib.rs::OwnerAlpha")),
            callee: RepoMapNodeRef::Symbol(SymbolId::new("src/lib.rs::OwnerBeta")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Call(
        quanta_index_contract::RepoMapCallEdge {
            caller: RepoMapNodeRef::Symbol(SymbolId::new("src/lib.rs::OwnerAlpha")),
            callee: RepoMapNodeRef::File(FileId::new("src/runtime/mod.rs")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Import(
        quanta_index_contract::RepoMapImportEdge {
            importer: RepoMapNodeRef::File(FileId::new("src/runtime/mod.rs")),
            imported: RepoMapNodeRef::File(FileId::new("src/lib.rs")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Import(
        quanta_index_contract::RepoMapImportEdge {
            importer: RepoMapNodeRef::File(FileId::new("tests/repomap.rs")),
            imported: RepoMapNodeRef::File(FileId::new("src/runtime/mod.rs")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        quanta_index_contract::RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::Symbol(SymbolId::new("src/lib.rs::OwnerAlpha")),
            chunk: RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new("chunk://alpha")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        quanta_index_contract::RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::Symbol(SymbolId::new("src/lib.rs::OwnerBeta")),
            chunk: RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new("chunk://beta")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        quanta_index_contract::RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::File(FileId::new("src/runtime/mod.rs")),
            chunk: RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new("chunk://runtime")),
        },
    ))
}

fn activate(store: &RepoMapGenerationStore, bundle: &RepoMapSourceBundle) -> Result<(), CoreError> {
    store.activate_generation(&RepoMapActivateGenerationRequest {
        repo_id: bundle.repo_id.clone(),
        revision_id: bundle.revision_id.clone(),
        manifest_generation: bundle.manifest_generation,
        manifest_digest: "manifest-digest-7".to_string(),
    })
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

fn assert_invalid_contract_contains(error: CoreError, needle: &str) {
    match error {
        CoreError::InvalidContract(message) => assert!(message.contains(needle)),
        CoreError::Typed { code, message } => {
            assert!(
                false,
                "expected InvalidContract, got Typed({code}, {message})"
            );
        }
        CoreError::NotReady(message) => {
            assert!(false, "expected InvalidContract, got NotReady({message})");
        }
        CoreError::NotImplemented(message) => {
            assert!(
                false,
                "expected InvalidContract, got NotImplemented({message})"
            );
        }
        CoreError::NotFound(message) => {
            assert!(false, "expected InvalidContract, got NotFound({message})");
        }
        CoreError::Storage(message) => {
            assert!(false, "expected InvalidContract, got Storage({message})");
        }
    }
}

fn assert_storage_contains(error: CoreError, needles: &[&str]) {
    match error {
        CoreError::Storage(message) => {
            for needle in needles {
                assert!(message.contains(needle));
            }
        }
        CoreError::InvalidContract(message) => {
            assert!(false, "expected Storage, got InvalidContract({message})");
        }
        CoreError::Typed { code, message } => {
            assert!(false, "expected Storage, got Typed({code}, {message})");
        }
        CoreError::NotReady(message) => {
            assert!(false, "expected Storage, got NotReady({message})");
        }
        CoreError::NotImplemented(message) => {
            assert!(false, "expected Storage, got NotImplemented({message})");
        }
        CoreError::NotFound(message) => {
            assert!(false, "expected Storage, got NotFound({message})");
        }
    }
}

#[test]
fn ingest_and_query_returns_ranked_entries() {
    let store = RepoMapGenerationStore::default();
    let bundle = sample_bundle();

    let ingest_result = store.ingest_bundle(&bundle);
    assert!(
        ingest_result.is_ok(),
        "bundle ingest should succeed: {ingest_result:?}"
    );

    let activation_result = activate(&store, &bundle);
    assert!(
        activation_result.is_ok(),
        "activate should succeed: {activation_result:?}"
    );

    let response_result = store.read_query_snapshot(&RepoMapQueryRequest {
        repo_id: bundle.repo_id.clone(),
        revision_id: bundle.revision_id.clone(),
        manifest_generation: bundle.manifest_generation,
        query_text: "owner runtime".to_string(),
        top_k: 2,
        token_budget: 256,
        focus_subjects: Vec::new(),
    });
    assert!(
        response_result.is_ok(),
        "query should succeed: {response_result:?}"
    );
    let Ok(response) = response_result else {
        return;
    };

    assert_eq!(response.repo_id, bundle.repo_id);
    assert_eq!(response.revision_id, bundle.revision_id);
    assert_eq!(response.manifest_generation, bundle.manifest_generation);
    assert_eq!(response.snapshot_meta.snapshot_id, "snap-7");
    assert_eq!(response.entries.len(), 5);
    assert_eq!(
        response
            .entries
            .first()
            .map(|entry| entry.subject_identity.as_str()),
        Some("src/lib.rs::OwnerAlpha")
    );
    assert_eq!(response.entries.first().map(|entry| entry.rank), Some(1));
    assert!(response.entries.get(1).is_some_and(|entry| entry.included));
    assert_eq!(
        response
            .entries
            .iter()
            .filter(|entry| entry.included)
            .count(),
        2
    );
    assert_eq!(response.dropped_entries_count, 3);
    assert!(
        response
            .drop_reason_codes
            .iter()
            .any(|code| code == "top_k_exhausted")
    );
}

#[test]
fn query_honors_focus_subjects() {
    let store = RepoMapGenerationStore::default();
    let bundle = sample_bundle();

    let ingest_result = store.ingest_bundle(&bundle);
    assert!(
        ingest_result.is_ok(),
        "bundle ingest should succeed: {ingest_result:?}"
    );

    let activation_result = activate(&store, &bundle);
    assert!(
        activation_result.is_ok(),
        "activate should succeed: {activation_result:?}"
    );

    let response_result = store.read_query_snapshot(&RepoMapQueryRequest {
        repo_id: bundle.repo_id.clone(),
        revision_id: bundle.revision_id.clone(),
        manifest_generation: bundle.manifest_generation,
        query_text: "runtime activation".to_string(),
        top_k: 10,
        token_budget: 512,
        focus_subjects: vec![RepoMapFocusSubjectDto {
            subject_identity: "src/runtime/mod.rs".to_string(),
            subject_doc_type: RepoMapDocType::File,
        }],
    });
    assert!(
        response_result.is_ok(),
        "focused query should succeed: {response_result:?}"
    );
    let Ok(response) = response_result else {
        return;
    };

    assert_eq!(response.entries.len(), 1);
    assert_eq!(
        response
            .entries
            .first()
            .map(|entry| entry.subject_identity.as_str()),
        Some("src/runtime/mod.rs")
    );
    assert_eq!(
        response
            .entries
            .first()
            .map(|entry| entry.subject_doc_type.as_code_str()),
        Some("File")
    );
    assert_eq!(response.entries.first().map(|entry| entry.rank), Some(1));
}

#[test]
fn query_before_activate_fails_closed() {
    let store = RepoMapGenerationStore::default();
    let bundle = sample_bundle();
    let ingest_result = store.ingest_bundle(&bundle);
    assert!(
        ingest_result.is_ok(),
        "bundle ingest should succeed: {ingest_result:?}"
    );

    let query_result = store.read_query_snapshot(&RepoMapQueryRequest {
        repo_id: bundle.repo_id.clone(),
        revision_id: bundle.revision_id.clone(),
        manifest_generation: bundle.manifest_generation,
        query_text: "repo map".to_string(),
        top_k: 1,
        token_budget: 64,
        focus_subjects: Vec::new(),
    });
    assert!(
        query_result.is_err(),
        "query before activate should fail: {query_result:?}"
    );
    let Err(error) = query_result else {
        return;
    };
    assert_not_found_contains(error, "no activated generation");
}

#[test]
fn missing_snapshot_fails_closed() {
    let store = RepoMapGenerationStore::default();
    let query_result = store.read_query_snapshot(&RepoMapQueryRequest {
        repo_id: RepoId::new("repo-missing"),
        revision_id: RevisionId::new("rev-missing"),
        manifest_generation: ManifestGeneration::new(42),
        query_text: "repo map".to_string(),
        top_k: 1,
        token_budget: 64,
        focus_subjects: Vec::new(),
    });
    assert!(
        query_result.is_err(),
        "missing snapshot should fail: {query_result:?}"
    );
    let Err(error) = query_result else {
        return;
    };
    assert_not_found_contains(error, "no activated generation");
}

#[test]
fn activate_generation_tracks_latest_owner_state() {
    let store = RepoMapGenerationStore::default();
    let bundle = sample_bundle();
    let ingest_result = store.ingest_bundle(&bundle);
    assert!(
        ingest_result.is_ok(),
        "bundle ingest should succeed: {ingest_result:?}"
    );

    let activation_result = activate(&store, &bundle);
    assert!(
        activation_result.is_ok(),
        "activate should succeed: {activation_result:?}"
    );

    assert_eq!(
        store.activated_generation_for(&bundle.repo_id, &bundle.revision_id),
        Some(7)
    );
}

#[test]
fn activate_generation_rejects_empty_manifest_digest() {
    let store = RepoMapGenerationStore::default();
    let bundle = sample_bundle();
    let ingest_result = store.ingest_bundle(&bundle);
    assert!(
        ingest_result.is_ok(),
        "bundle ingest should succeed: {ingest_result:?}"
    );

    let activation_result = store.activate_generation(&RepoMapActivateGenerationRequest {
        repo_id: bundle.repo_id.clone(),
        revision_id: bundle.revision_id.clone(),
        manifest_generation: bundle.manifest_generation,
        manifest_digest: String::new(),
    });
    assert!(
        activation_result.is_err(),
        "empty manifest digest should fail: {activation_result:?}"
    );
    let Err(error) = activation_result else {
        return;
    };
    assert_invalid_contract_contains(error, "manifest_digest must not be empty");
}

#[test]
fn activate_generation_requires_materialized_snapshot() {
    let store = RepoMapGenerationStore::default();
    let activation_result = store.activate_generation(&RepoMapActivateGenerationRequest {
        repo_id: RepoId::new("repo-missing"),
        revision_id: RevisionId::new("rev-missing"),
        manifest_generation: ManifestGeneration::new(99),
        manifest_digest: "manifest-digest-99".to_string(),
    });
    assert!(
        activation_result.is_err(),
        "activation without snapshot should fail: {activation_result:?}"
    );
    let Err(error) = activation_result else {
        return;
    };
    assert_not_found_contains(error, "repomap activate: no snapshot");
}

#[test]
fn persistent_store_reloads_snapshot_and_activation() {
    let dir_result = tempfile::tempdir();
    assert!(dir_result.is_ok(), "tempdir should succeed: {dir_result:?}");
    let Ok(dir) = dir_result else {
        return;
    };

    let bundle = sample_bundle();
    let store_result = RepoMapGenerationStore::with_persistence_root(dir.path());
    assert!(
        store_result.is_ok(),
        "persistent store open should succeed: {store_result:?}"
    );
    let Ok(store) = store_result else {
        return;
    };

    let ingest_result = store.ingest_bundle(&bundle);
    assert!(
        ingest_result.is_ok(),
        "bundle ingest should succeed: {ingest_result:?}"
    );
    let activation_result = activate(&store, &bundle);
    assert!(
        activation_result.is_ok(),
        "activate should succeed: {activation_result:?}"
    );
    drop(store);

    let reload_result = RepoMapGenerationStore::with_persistence_root(dir.path());
    assert!(
        reload_result.is_ok(),
        "reloaded store open should succeed: {reload_result:?}"
    );
    let Ok(reloaded) = reload_result else {
        return;
    };

    let response_result = reloaded.read_query_snapshot(&RepoMapQueryRequest {
        repo_id: bundle.repo_id.clone(),
        revision_id: bundle.revision_id.clone(),
        manifest_generation: bundle.manifest_generation,
        query_text: "owner runtime".to_string(),
        top_k: 2,
        token_budget: 256,
        focus_subjects: Vec::new(),
    });
    assert!(
        response_result.is_ok(),
        "reloaded query should succeed: {response_result:?}"
    );
    let Ok(response) = response_result else {
        return;
    };

    assert_eq!(response.snapshot_meta.snapshot_id, "snap-7");
    assert_eq!(response.entries.len(), 5);
    assert_eq!(
        reloaded.activated_generation_for(&bundle.repo_id, &bundle.revision_id),
        Some(7)
    );
}

#[test]
fn persistent_store_rejects_orphaned_activation_file() {
    let dir_result = tempfile::tempdir();
    assert!(dir_result.is_ok(), "tempdir should succeed: {dir_result:?}");
    let Ok(dir) = dir_result else {
        return;
    };

    let activations_dir = dir.path().join("activations");
    let mkdir_result = fs::create_dir_all(&activations_dir);
    assert!(
        mkdir_result.is_ok(),
        "activations dir create should succeed: {mkdir_result:?}"
    );

    let write_result = fs::write(
        activations_dir.join("repo-missing--rev-missing.json"),
        r#"{
  "repo_id": "repo-missing",
  "revision_id": "rev-missing",
  "manifest_generation": 99
}"#,
    );
    assert!(
        write_result.is_ok(),
        "orphan activation write should succeed: {write_result:?}"
    );

    let store_result = RepoMapGenerationStore::with_persistence_root(dir.path());
    assert!(
        store_result.is_err(),
        "orphaned activation should fail closed: {store_result:?}"
    );
    let Err(error) = store_result else {
        return;
    };

    assert_storage_contains(
        error,
        &[
            "activation persisted without snapshot",
            "repo=repo-missing",
            "revision=rev-missing",
            "generation=99",
        ],
    );
}
