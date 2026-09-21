#![expect(
    clippy::unreachable,
    reason = "test fixtures use invariant literal constructors for language and symbol kinds"
)]

use std::fs;

use quanta_index_contract::lex::{LanguageCode, SymbolKindCode};
use quanta_index_contract::{
    FileId, ManifestGeneration, RepoId, RepoMapActivateGenerationRequest, RepoMapChunkExactness,
    RepoMapDocType, RepoMapExactnessSummary, RepoMapFocusSubjectDto, RepoMapGraphCoverage,
    RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapMutationOperationV2,
    RepoMapNode, RepoMapNodeRef, RepoMapQueryRequest, RepoMapRedactionState, RepoMapSourceBundle,
    RepoRelativePath, RevisionId, SymbolId, canonical_repo_map_source_bundle_digest_v1,
};
use quanta_index_core::CoreError;
use quanta_index_repomap::RepoMapGenerationStore;

fn rust_language() -> LanguageCode {
    match LanguageCode::new("rust") {
        Ok(language) => language,
        Err(err) => unreachable!("valid language: {err}"),
    }
}

fn symbol_kind(name: &str) -> SymbolKindCode {
    match SymbolKindCode::new(name) {
        Ok(symbol_kind) => symbol_kind,
        Err(err) => unreachable!("valid symbol kind `{name}`: {err}"),
    }
}

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
            symbol_kind: symbol_kind("struct"),
        },
    ))
    .with_node(RepoMapNode::Symbol(
        quanta_index_contract::RepoMapSymbolNode {
            symbol_id: SymbolId::new("src/lib.rs::OwnerBeta"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            local_name: "OwnerBeta".to_string(),
            qualified_name: "src::lib::OwnerBeta".to_string(),
            symbol_kind: symbol_kind("struct"),
        },
    ))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://alpha"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            language: rust_language(),
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
            language: rust_language(),
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
            language: rust_language(),
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
    let _receipt = store.activate_generation(&RepoMapActivateGenerationRequest {
        repo_id: bundle.repo_id.clone(),
        revision_id: bundle.revision_id.clone(),
        manifest_generation: bundle.manifest_generation,
        manifest_digest: "manifest-digest-7".to_string(),
    })?;
    Ok(())
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
    // `top_k=2` returns two rows; the other three are a count, not rows
    // (QI-BB-008).
    assert_eq!(response.entries.len(), 2);
    assert_eq!(
        response
            .entries
            .first()
            .map(|entry| entry.subject_identity.as_str()),
        Some("src/lib.rs::OwnerAlpha")
    );
    assert_eq!(
        response
            .entries
            .iter()
            .map(|entry| entry.rank)
            .collect::<Vec<_>>(),
        vec![1, 2]
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
fn mutation_receipts_bind_exact_persisted_bundle_and_activation() {
    let store = RepoMapGenerationStore::default();
    let bundle = sample_bundle();
    let expected_bundle_digest = canonical_repo_map_source_bundle_digest_v1(&bundle);
    assert!(
        expected_bundle_digest.is_ok(),
        "bundle digest should encode: {expected_bundle_digest:?}"
    );
    let Ok(expected_bundle_digest) = expected_bundle_digest else {
        return;
    };

    let publish = store.ingest_bundle(&bundle);
    assert!(publish.is_ok(), "publish should succeed: {publish:?}");
    let Ok(publish) = publish else {
        return;
    };
    assert_eq!(publish.operation, RepoMapMutationOperationV2::Publish);
    assert_eq!(publish.manifest_digest, bundle.manifest_digest);
    assert_eq!(publish.snapshot_id, bundle.snapshot_id);
    assert_eq!(publish.projection_version, bundle.projection_version);
    assert_eq!(publish.authority_digest, bundle.authority_digest);
    assert_eq!(publish.source_bundle_digest, expected_bundle_digest);

    let activate = store.activate_generation(&RepoMapActivateGenerationRequest {
        repo_id: bundle.repo_id.clone(),
        revision_id: bundle.revision_id.clone(),
        manifest_generation: bundle.manifest_generation,
        manifest_digest: bundle.manifest_digest.clone(),
    });
    assert!(activate.is_ok(), "activation should succeed: {activate:?}");
    let Ok(activate) = activate else {
        return;
    };
    assert_eq!(activate.operation, RepoMapMutationOperationV2::Activate);
    assert_eq!(activate.source_bundle_digest, expected_bundle_digest);
    assert_eq!(activate.manifest_digest, bundle.manifest_digest);
}

#[test]
fn activate_generation_rejects_manifest_digest_mismatch() {
    let store = RepoMapGenerationStore::default();
    let bundle = sample_bundle();
    let publish = store.ingest_bundle(&bundle);
    assert!(publish.is_ok(), "publish should succeed: {publish:?}");

    let activation = store.activate_generation(&RepoMapActivateGenerationRequest {
        repo_id: bundle.repo_id,
        revision_id: bundle.revision_id,
        manifest_generation: bundle.manifest_generation,
        manifest_digest: "manifest:wrong".to_string(),
    });
    assert!(activation.is_err(), "mismatched manifest must fail");
    let Err(error) = activation else {
        return;
    };
    assert_invalid_contract_contains(error, "manifest_digest mismatch");
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

    let activated = store.activated_generation_for(&bundle.repo_id, &bundle.revision_id);
    assert!(
        activated.is_ok(),
        "activated_generation_for should succeed: {activated:?}"
    );
    let Ok(activated) = activated else {
        return;
    };
    assert_eq!(activated, Some(7));
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
    let store_result = RepoMapGenerationStore::open(dir.path());
    assert!(
        store_result.is_ok(),
        "persistent store open should succeed: {store_result:?}"
    );
    let Ok(opened) = store_result else {
        return;
    };
    let store = opened.store;

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

    let reload_result = RepoMapGenerationStore::open(dir.path());
    assert!(
        reload_result.is_ok(),
        "reloaded store open should succeed: {reload_result:?}"
    );
    let Ok(reopened) = reload_result else {
        return;
    };
    assert_eq!(reopened.report.snapshots_loaded, 1);
    assert_eq!(reopened.report.activations_loaded, 1);
    assert!(reopened.report.quarantined.is_empty());
    let reloaded = reopened.store;

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
    assert_eq!(response.entries.len(), 2);
    let activated = reloaded.activated_generation_for(&bundle.repo_id, &bundle.revision_id);
    assert!(
        activated.is_ok(),
        "activated_generation_for should succeed: {activated:?}"
    );
    let Ok(activated) = activated else {
        return;
    };
    assert_eq!(activated, Some(7));
}

/// An activation whose snapshot is gone is reported and answers `NOT_FOUND`
/// for its repo; it no longer fails the whole store open (QI-BB-008).
#[test]
fn persistent_store_reports_an_orphaned_activation_and_fails_closed_for_its_repo() {
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

    let store_result = RepoMapGenerationStore::open(dir.path());
    assert!(
        store_result.is_ok(),
        "an orphaned activation must not fail the open: {store_result:?}"
    );
    let Ok(opened) = store_result else {
        return;
    };
    assert_eq!(
        opened.report.activations_without_snapshot,
        vec!["repo=repo-missing revision=rev-missing generation=99".to_string()]
    );
    let query_result = opened.store.read_query_snapshot(&RepoMapQueryRequest {
        repo_id: RepoId::new("repo-missing"),
        revision_id: RevisionId::new("rev-missing"),
        manifest_generation: ManifestGeneration::new(99),
        query_text: "anything".to_string(),
        top_k: 1,
        token_budget: 16,
        focus_subjects: Vec::new(),
    });
    assert!(
        query_result.is_err(),
        "the orphaned repo answers fail-closed: {query_result:?}"
    );
    let Err(error) = query_result else {
        return;
    };
    assert_not_found_contains(error, "no activated generation");
}
