use quanta_index_contract::{
    BundleArtifactRef, BundleEncoding, BundleMode, GenerationId, ManifestDigest,
    ManifestGeneration, PreparedBundleOutbox, PublishedGenerationSet,
    PublishedSearchBundlePrepareRequest, PublishedSearchGenerationActivateRequest, RepoId,
    RevisionId,
};
use quanta_index_core::{
    PublishedSearchBundlePreparePort, PublishedSearchGenerationActivatePort,
    PublishedSearchGenerationReadinessPort,
};
use tempfile::tempdir;

use super::SqliteControlPlane;

#[test]
fn bootstraps_schema_and_accepts_outbox_row() {
    let temp = match tempdir() {
        Ok(temp) => temp,
        Err(error) => {
            assert!(false, "tempdir failed: {error}");
            return;
        }
    };
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = match SqliteControlPlane::open(&db_path) {
        Ok(store) => store,
        Err(error) => {
            assert!(false, "open store failed: {error}");
            return;
        }
    };

    let response_result = store.prepare_bundle(PublishedSearchBundlePrepareRequest {
        outbox: PreparedBundleOutbox {
            outbox_id: "outbox-1".into(),
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_digest: ManifestDigest::new("digest"),
            bundle_schema_version: 1,
            prepared_at_ms: 1,
            mode: BundleMode::ServeOnly,
            manifest_ref: BundleArtifactRef {
                relative_path: "bundle/manifest.json".into(),
                encoding: BundleEncoding::Json,
                byte_length: 128,
                content_digest: ManifestDigest::new("digest"),
            },
            base_generation: None,
            changed_artifact_mask: 1,
        },
    });
    let response = match response_result {
        Ok(response) => response,
        Err(error) => {
            assert!(false, "prepare failed: {error}");
            return;
        }
    };

    assert!(response.accepted);
    assert_eq!(response.state, "prepared");
}

#[test]
fn activates_generation_and_exposes_readiness() {
    let temp = match tempdir() {
        Ok(temp) => temp,
        Err(error) => {
            assert!(false, "tempdir failed: {error}");
            return;
        }
    };
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = match SqliteControlPlane::open(&db_path) {
        Ok(store) => store,
        Err(error) => {
            assert!(false, "open store failed: {error}");
            return;
        }
    };

    let generation = PublishedGenerationSet {
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        manifest_generation: ManifestGeneration::new(7),
        lexical_generation: GenerationId::new(10),
        symbol_generation: GenerationId::new(11),
        structural_generation: None,
        history_generation: None,
        semantic_generation: Some(GenerationId::new(12)),
        metadata_generation: Some(GenerationId::new(13)),
    };

    let activation_result = store.activate_generation(PublishedSearchGenerationActivateRequest {
        generation: generation.clone(),
        lexical_ready: true,
        semantic_ready: true,
        active_at_ms: 42,
    });
    assert!(
        activation_result.is_ok(),
        "activate failed: {activation_result:?}"
    );

    let readiness_result = store.read_readiness(&RepoId::new("repo"), &RevisionId::new("rev"));
    let readiness = match readiness_result {
        Ok(readiness) => readiness,
        Err(error) => {
            assert!(false, "read readiness failed: {error}");
            return;
        }
    };

    assert!(readiness.lexical_ready);
    assert!(readiness.semantic_ready);
    assert_eq!(readiness.active_generation, Some(generation));
}
