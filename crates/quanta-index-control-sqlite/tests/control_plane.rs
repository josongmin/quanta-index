pub mod support;

use quanta_index_contract::{
    PublishedSearchBundlePrepareRequest, PublishedSearchGenerationActivateRequest, RepoId,
    RevisionId,
};
use quanta_index_control_sqlite::SqliteControlPlane;
use quanta_index_core::{
    PublishedSearchBundleInspectPort, PublishedSearchBundlePreparePort,
    PublishedSearchGenerationActivatePort, PublishedSearchGenerationReadinessPort,
};
use tempfile::tempdir;

use self::support::{sample_generation, sample_outbox};

#[test]
fn prepared_bundle_is_visible_in_readiness_count() {
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

    let prepare_result = store.prepare_bundle(PublishedSearchBundlePrepareRequest {
        outbox: sample_outbox(),
    });
    assert!(prepare_result.is_ok(), "prepare failed: {prepare_result:?}");

    let readiness_result = store.read_readiness(&RepoId::new("repo"), &RevisionId::new("rev"));
    let readiness = match readiness_result {
        Ok(readiness) => readiness,
        Err(error) => {
            assert!(false, "read readiness failed: {error}");
            return;
        }
    };

    assert_eq!(readiness.prepared_bundle_count, 1);
    assert!(readiness.active_generation.is_none());
}

#[test]
fn activated_generation_is_exposed_through_public_ports() {
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
    let generation = sample_generation();

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
    let inspected_result = store.inspect_bundle(&generation);
    let inspected = match inspected_result {
        Ok(inspected) => inspected,
        Err(error) => {
            assert!(false, "inspect failed: {error}");
            return;
        }
    };

    assert_eq!(readiness.active_generation, Some(generation.clone()));
    assert_eq!(inspected.manifest.repo_id, generation.repo_id);
    assert_eq!(inspected.artifacts.len(), 3);
}
