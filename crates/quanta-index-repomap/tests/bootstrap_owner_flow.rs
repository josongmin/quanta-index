use std::{error::Error, fs};

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV1, RepoMapFocusSubjectDtoV1,
    RepoMapQueryRequestV1, RepoMapSourceBundleV1, RevisionId,
};
use quanta_index_core::CoreError;
use quanta_index_repomap::RepoMapGenerationStore;

type TestResult = Result<(), Box<dyn Error>>;

fn sample_bundle() -> RepoMapSourceBundleV1 {
    RepoMapSourceBundleV1 {
        repo_id: RepoId::new("repo-a"),
        revision_id: RevisionId::new("rev-a"),
        manifest_generation: ManifestGeneration::new(7),
        snapshot_id: "snap-7".to_string(),
        projection_version: 1,
        authority_digest: "digest-7".to_string(),
        item_index_availability: "available".to_string(),
        graph_coverage_class: "full".to_string(),
        exactness_summary: "bootstrap".to_string(),
        entry_identities: vec![
            "src/lib.rs".to_string(),
            "src/runtime/mod.rs".to_string(),
            "tests/repomap.rs".to_string(),
        ],
    }
}

#[test]
fn ingest_and_query_returns_ranked_entries() {
    let store = RepoMapGenerationStore::default();
    let bundle = sample_bundle();
    store
        .ingest_bundle(&bundle)
        .expect("bundle ingest should succeed");

    let response = store
        .read_query_snapshot(&RepoMapQueryRequestV1 {
            repo_id: bundle.repo_id.clone(),
            revision_id: bundle.revision_id.clone(),
            manifest_generation: bundle.manifest_generation,
            query_text: "repo map".to_string(),
            top_k: 2,
            token_budget: 256,
            focus_subjects: Vec::new(),
        })
        .expect("query should succeed");

    assert_eq!(response.repo_id, bundle.repo_id);
    assert_eq!(response.revision_id, bundle.revision_id);
    assert_eq!(response.manifest_generation, bundle.manifest_generation);
    assert_eq!(response.snapshot_meta.snapshot_id, "snap-7");
    assert_eq!(response.entries.len(), 3);
    assert_eq!(response.entries[0].rank, 1);
    assert_eq!(response.entries[0].owner_path, "src/lib.rs");
    assert_eq!(response.entries[2].rank, 0);
    assert_eq!(response.dropped_entries_count, 1);
}

#[test]
fn query_honors_focus_subjects() {
    let store = RepoMapGenerationStore::default();
    let bundle = sample_bundle();
    store
        .ingest_bundle(&bundle)
        .expect("bundle ingest should succeed");

    let response = store
        .read_query_snapshot(&RepoMapQueryRequestV1 {
            repo_id: bundle.repo_id.clone(),
            revision_id: bundle.revision_id.clone(),
            manifest_generation: bundle.manifest_generation,
            query_text: "repo map".to_string(),
            top_k: 10,
            token_budget: 640,
            focus_subjects: vec![RepoMapFocusSubjectDtoV1 {
                subject_identity: "src/runtime/mod.rs".to_string(),
                subject_doc_type: "File".to_string(),
            }],
        })
        .expect("focused query should succeed");

    assert_eq!(response.entries.len(), 1);
    assert_eq!(response.entries[0].subject_identity, "src/runtime/mod.rs");
    assert_eq!(response.entries[0].subject_doc_type, "File");
}

#[test]
fn missing_snapshot_fails_closed() {
    let store = RepoMapGenerationStore::default();
    let error = store
        .read_query_snapshot(&RepoMapQueryRequestV1 {
            repo_id: RepoId::new("repo-missing"),
            revision_id: RevisionId::new("rev-missing"),
            manifest_generation: ManifestGeneration::new(42),
            query_text: "repo map".to_string(),
            top_k: 1,
            token_budget: 64,
            focus_subjects: Vec::new(),
        })
        .expect_err("missing snapshot should fail");

    match error {
        CoreError::NotFound(message) => {
            assert!(message.contains("repomap snapshot missing"));
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn activate_generation_tracks_latest_owner_state() {
    let store = RepoMapGenerationStore::default();
    let bundle = sample_bundle();
    store
        .ingest_bundle(&bundle)
        .expect("bundle ingest should succeed");
    store
        .activate_generation(&RepoMapActivateGenerationRequestV1 {
            repo_id: bundle.repo_id.clone(),
            revision_id: bundle.revision_id.clone(),
            manifest_generation: bundle.manifest_generation,
            manifest_digest: "manifest-digest-7".to_string(),
        })
        .expect("activate should succeed");

    let activated = store
        .activated_generation_for(&bundle.repo_id, &bundle.revision_id)
        .expect("activated generation should be recorded");
    assert_eq!(activated, 7);
}

#[test]
fn activate_generation_rejects_empty_manifest_digest() {
    let store = RepoMapGenerationStore::default();
    let bundle = sample_bundle();
    store
        .ingest_bundle(&bundle)
        .expect("bundle ingest should succeed");

    let error = store
        .activate_generation(&RepoMapActivateGenerationRequestV1 {
            repo_id: bundle.repo_id.clone(),
            revision_id: bundle.revision_id.clone(),
            manifest_generation: bundle.manifest_generation,
            manifest_digest: String::new(),
        })
        .expect_err("empty manifest digest should fail");

    match error {
        CoreError::InvalidContract(message) => {
            assert!(message.contains("manifest_digest must not be empty"));
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn activate_generation_requires_materialized_snapshot() {
    let store = RepoMapGenerationStore::default();
    let error = store
        .activate_generation(&RepoMapActivateGenerationRequestV1 {
            repo_id: RepoId::new("repo-missing"),
            revision_id: RevisionId::new("rev-missing"),
            manifest_generation: ManifestGeneration::new(99),
            manifest_digest: "manifest-digest-99".to_string(),
        })
        .expect_err("activation without snapshot should fail");

    match error {
        CoreError::NotFound(message) => {
            assert!(message.contains("repomap activate: no snapshot"));
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn persistent_store_reloads_snapshot_and_activation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bundle = sample_bundle();
    let store = RepoMapGenerationStore::with_persistence_root(dir.path())?;
    store.ingest_bundle(&bundle)?;
    store.activate_generation(&RepoMapActivateGenerationRequestV1 {
        repo_id: bundle.repo_id.clone(),
        revision_id: bundle.revision_id.clone(),
        manifest_generation: bundle.manifest_generation,
        manifest_digest: "manifest-digest-7".to_string(),
    })?;
    drop(store);

    let reloaded = RepoMapGenerationStore::with_persistence_root(dir.path())?;
    let response = reloaded.read_query_snapshot(&RepoMapQueryRequestV1 {
        repo_id: bundle.repo_id.clone(),
        revision_id: bundle.revision_id.clone(),
        manifest_generation: bundle.manifest_generation,
        query_text: "repo map".to_string(),
        top_k: 2,
        token_budget: 256,
        focus_subjects: Vec::new(),
    })?;

    assert_eq!(response.snapshot_meta.snapshot_id, "snap-7");
    assert_eq!(response.entries.len(), 3);
    let activated = reloaded
        .activated_generation_for(&bundle.repo_id, &bundle.revision_id)
        .expect("reloaded store should keep activated generation");
    assert_eq!(activated, 7);
    Ok(())
}

#[test]
fn persistent_store_rejects_orphaned_activation_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    let activations_dir = dir.path().join("activations");
    fs::create_dir_all(&activations_dir)?;
    fs::write(
        activations_dir.join("repo-missing--rev-missing.json"),
        r#"{
  "repo_id": "repo-missing",
  "revision_id": "rev-missing",
  "manifest_generation": 99
}"#,
    )?;

    let error = match RepoMapGenerationStore::with_persistence_root(dir.path()) {
        Ok(_store) => panic!("orphaned activation should fail closed"),
        Err(error) => error,
    };
    match error {
        CoreError::Storage(message) => {
            assert!(message.contains("activation persisted without snapshot"));
            assert!(message.contains("repo=repo-missing"));
            assert!(message.contains("revision=rev-missing"));
            assert!(message.contains("generation=99"));
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
    Ok(())
}
