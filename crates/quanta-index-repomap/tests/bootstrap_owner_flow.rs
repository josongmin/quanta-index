use std::fs;

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV1, RepoMapChunkRecordDtoV1,
    RepoMapFileIndexRecordV1, RepoMapFocusSubjectDtoV1, RepoMapGraphEdgeDtoV1,
    RepoMapQueryRequestV1, RepoMapSourceBundleV1, RepoMapSymbolRecordDtoV1, RevisionId,
};
use quanta_index_core::CoreError;
use quanta_index_repomap::RepoMapGenerationStore;

fn sample_bundle() -> RepoMapSourceBundleV1 {
    RepoMapSourceBundleV1 {
        repo_id: RepoId::new("repo-a"),
        revision_id: RevisionId::new("rev-a"),
        manifest_generation: ManifestGeneration::new(7),
        snapshot_id: "snap-7".to_string(),
        projection_version: 1,
        authority_digest: "digest-7".to_string(),
        item_index_availability: "available".to_string(),
        graph_coverage_class: "complete".to_string(),
        exactness_summary: "exact-owner-bundle".to_string(),
        redaction_state: "Unredacted".to_string(),
        file_indices: vec![
            RepoMapFileIndexRecordV1 {
                file_identity: "src/lib.rs".to_string(),
                file_path: "src/lib.rs".to_string(),
                file_kind: "library".to_string(),
                line_count: 140,
                symbol_records: vec![
                    RepoMapSymbolRecordDtoV1 {
                        subject_identity: "src/lib.rs::OwnerAlpha".to_string(),
                        subject_doc_type: "Symbol".to_string(),
                        subject_kind: "service".to_string(),
                        symbol_name: "OwnerAlpha".to_string(),
                        owner_path: "src/lib.rs".to_string(),
                    },
                    RepoMapSymbolRecordDtoV1 {
                        subject_identity: "src/lib.rs::OwnerBeta".to_string(),
                        subject_doc_type: "Symbol".to_string(),
                        subject_kind: "struct".to_string(),
                        symbol_name: "OwnerBeta".to_string(),
                        owner_path: "src/lib.rs".to_string(),
                    },
                ],
            },
            RepoMapFileIndexRecordV1 {
                file_identity: "src/runtime/mod.rs".to_string(),
                file_path: "src/runtime/mod.rs".to_string(),
                file_kind: "runtime".to_string(),
                line_count: 96,
                symbol_records: Vec::new(),
            },
            RepoMapFileIndexRecordV1 {
                file_identity: "tests/repomap.rs".to_string(),
                file_path: "tests/repomap.rs".to_string(),
                file_kind: "test".to_string(),
                line_count: 64,
                symbol_records: Vec::new(),
            },
        ],
        call_edges: vec![
            RepoMapGraphEdgeDtoV1 {
                from_identity: "src/lib.rs::OwnerAlpha".to_string(),
                to_identity: "src/lib.rs::OwnerBeta".to_string(),
                edge_kind: "call".to_string(),
            },
            RepoMapGraphEdgeDtoV1 {
                from_identity: "src/lib.rs::OwnerAlpha".to_string(),
                to_identity: "src/runtime/mod.rs".to_string(),
                edge_kind: "call".to_string(),
            },
        ],
        import_edges: vec![
            RepoMapGraphEdgeDtoV1 {
                from_identity: "src/runtime/mod.rs".to_string(),
                to_identity: "src/lib.rs".to_string(),
                edge_kind: "import".to_string(),
            },
            RepoMapGraphEdgeDtoV1 {
                from_identity: "tests/repomap.rs".to_string(),
                to_identity: "src/runtime/mod.rs".to_string(),
                edge_kind: "import".to_string(),
            },
        ],
        chunk_records: vec![
            RepoMapChunkRecordDtoV1 {
                subject_identity: "src/lib.rs::OwnerAlpha".to_string(),
                owner_path: "src/lib.rs".to_string(),
                token_count: 80,
                preview_text: "OwnerAlpha coordinates repo map ownership".to_string(),
                exactness: "Exact".to_string(),
            },
            RepoMapChunkRecordDtoV1 {
                subject_identity: "src/lib.rs::OwnerBeta".to_string(),
                owner_path: "src/lib.rs".to_string(),
                token_count: 56,
                preview_text: "OwnerBeta carries owner surface metadata".to_string(),
                exactness: "Exact".to_string(),
            },
            RepoMapChunkRecordDtoV1 {
                subject_identity: "src/runtime/mod.rs".to_string(),
                owner_path: "src/runtime/mod.rs".to_string(),
                token_count: 40,
                preview_text: "runtime module activation path".to_string(),
                exactness: "Approximate".to_string(),
            },
        ],
    }
}

fn activate(
    store: &RepoMapGenerationStore,
    bundle: &RepoMapSourceBundleV1,
) -> Result<(), CoreError> {
    store.activate_generation(&RepoMapActivateGenerationRequestV1 {
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

    let response_result = store.read_query_snapshot(&RepoMapQueryRequestV1 {
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

    let response_result = store.read_query_snapshot(&RepoMapQueryRequestV1 {
        repo_id: bundle.repo_id.clone(),
        revision_id: bundle.revision_id.clone(),
        manifest_generation: bundle.manifest_generation,
        query_text: "runtime activation".to_string(),
        top_k: 10,
        token_budget: 512,
        focus_subjects: vec![RepoMapFocusSubjectDtoV1 {
            subject_identity: "src/runtime/mod.rs".to_string(),
            subject_doc_type: "File".to_string(),
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
            .map(|entry| entry.subject_doc_type.as_str()),
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

    let query_result = store.read_query_snapshot(&RepoMapQueryRequestV1 {
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
    let query_result = store.read_query_snapshot(&RepoMapQueryRequestV1 {
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

    let activation_result = store.activate_generation(&RepoMapActivateGenerationRequestV1 {
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
    let activation_result = store.activate_generation(&RepoMapActivateGenerationRequestV1 {
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

    let response_result = reloaded.read_query_snapshot(&RepoMapQueryRequestV1 {
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
