//! Physical readiness of the pinned external Legacy reference process.
//! These results do not qualify Native process readiness.

use std::{error::Error, fs, os::unix::fs::PermissionsExt as _, time::Duration};

use quanta_index_contract::{
    GenerationPin, MetricsSnapshotV1, ProcessReadinessReasonV1, ProcessReadinessV1,
    QueryConstraintSetV1, RepoId, RevisionId, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_sdk::QuantaIndex;
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, daemon_socket_paths, publish_legacy_source,
};

use crate::fail_closed_wait::{RealTicker, wait_for};

type TestResult = Result<(), Box<dyn Error>>;

fn require_eq<T: std::fmt::Debug + PartialEq>(actual: &T, expected: &T, field: &str) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{field}: expected {expected:?}, got {actual:?}").into())
    }
}

fn private_short_root() -> Result<tempfile::TempDir, Box<dyn Error>> {
    let root = tempfile::Builder::new().prefix("qlp-").tempdir_in("/tmp")?;
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
    Ok(root)
}

fn owner() -> Result<(RepoId, RevisionId), Box<dyn Error>> {
    Ok((RepoId::new("repo-e2e")?, RevisionId::new("rev-e2e")?))
}

fn wait_ready(client: &QuantaIndex) -> Result<ProcessReadinessV1, Box<dyn Error>> {
    Ok(wait_for(
        &RealTicker::new(),
        Duration::from_secs(20),
        Duration::from_millis(50),
        "external Legacy reference active backend ready",
        || client.observability().process_readiness(),
        |report| report.ready,
        |_| true,
    )?)
}

fn wait_not_ready(client: &QuantaIndex) -> Result<ProcessReadinessV1, Box<dyn Error>> {
    Ok(wait_for(
        &RealTicker::new(),
        Duration::from_secs(20),
        Duration::from_millis(50),
        "external Legacy reference active backend loss",
        || client.observability().process_readiness(),
        |report| !report.ready,
        |_| true,
    )?)
}

fn counter(snapshot: &MetricsSnapshotV1, name: &str) -> Option<u64> {
    snapshot
        .counters
        .iter()
        .find(|point| point.name == name)
        .map(|point| point.value)
}

#[test]
fn active_repository_requires_physical_candidate_proof() -> TestResult {
    let parent = private_short_root()?;
    let state_root = parent.path().join("state");
    let legacy = LegacySearchdBinary::from_environment()?;
    let (repo, revision) = owner()?;
    let process = legacy.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let _published = publish_legacy_source(
            &client,
            &repo,
            &revision,
            1,
            None,
            &[("src/ready.rs", "readiness probe", 1)],
            None,
        )?;
        let report = wait_ready(&client)?;
        require_eq(&report.active_repositories, &1, "active repositories")?;
        require_eq(
            &report.active_candidate_integrity,
            &Some(true),
            "active integrity",
        )?;
        require_eq(
            &report
                .not_ready_reasons
                .contains(&ProcessReadinessReasonV1::ActiveCandidateIntegrityFailed),
            &false,
            "active integrity failure reason",
        )
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    legacy.verify()?;
    Ok(())
}

#[test]
fn lost_active_track_root_invalidates_backend_readiness_and_restores() -> TestResult {
    let parent = private_short_root()?;
    let state_root = parent.path().join("state");
    let legacy = LegacySearchdBinary::from_environment()?;
    let (repo, revision) = owner()?;
    let process = legacy.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let _published = publish_legacy_source(
            &client,
            &repo,
            &revision,
            1,
            None,
            &[("src/ready.rs", "fn ready() {}", 1)],
            None,
        )?;
        require_eq(&wait_ready(&client)?.ready, &true, "initial readiness")?;
        for track in ["lexical", "semantic"] {
            let root = state_root.join("indexes").join(track);
            let hidden = state_root.join("indexes").join(format!("{track}-hidden"));
            fs::rename(&root, &hidden)?;
            let lost = (|| -> TestResult {
                let report = wait_not_ready(&client)?;
                if !report
                    .not_ready_reasons
                    .contains(&ProcessReadinessReasonV1::RequiredBackendOpenUnproven)
                {
                    return Err(
                        format!("{track} root loss lacks backend reason: {report:?}").into(),
                    );
                }
                Ok(())
            })();
            fs::rename(&hidden, &root)?;
            lost?;
            require_eq(&wait_ready(&client)?.ready, &true, "restored readiness")?;
        }
        Ok(())
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    legacy.verify()?;
    Ok(())
}

#[test]
fn binary_daemon_detects_lost_active_backend_root() -> TestResult {
    let parent = private_short_root()?;
    let state_root = parent.path().join("state");
    let legacy = LegacySearchdBinary::from_environment()?;
    let (repo, revision) = owner()?;
    let preparation = legacy.start(&state_root, 8)?;
    let published = (|| -> TestResult {
        let client = preparation.connect()?;
        let _published = publish_legacy_source(
            &client,
            &repo,
            &revision,
            1,
            None,
            &[("src/ready.rs", "fn ready() {}", 1)],
            None,
        )?;
        Ok(())
    })();
    let preparation_stopped = preparation.stop();
    published.and(preparation_stopped)?;

    let process = legacy.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let ready = wait_ready(&client)?;
        require_eq(&ready.active_repositories, &1, "binary active repositories")?;
        let root = state_root.join("indexes/lexical");
        let hidden = state_root.join("indexes/lexical-hidden");
        fs::rename(&root, &hidden)?;
        let lost = (|| -> TestResult {
            let report = wait_not_ready(&client)?;
            if !report
                .not_ready_reasons
                .contains(&ProcessReadinessReasonV1::RequiredBackendOpenUnproven)
            {
                return Err(format!("binary root loss lacks backend reason: {report:?}").into());
            }
            Ok(())
        })();
        fs::rename(&hidden, &root)?;
        lost?;
        require_eq(
            &wait_ready(&client)?.ready,
            &true,
            "binary restored readiness",
        )
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    legacy.verify()?;
    Ok(())
}

#[test]
fn binary_daemon_counts_inventory_read_failure_and_reopens_after_repair() -> TestResult {
    let parent = private_short_root()?;
    let state_root = parent.path().join("state");
    let legacy = LegacySearchdBinary::from_environment()?;
    let (repo, revision) = owner()?;
    let preparation = legacy.start(&state_root, 8)?;
    let published = (|| -> Result<_, Box<dyn Error>> {
        let client = preparation.connect()?;
        publish_legacy_source(
            &client,
            &repo,
            &revision,
            1,
            None,
            &[("src/ready.rs", "needle inventory", 1)],
            None,
        )
    })();
    let preparation_stopped = preparation.stop();
    let (head, _receipt) = published?;
    preparation_stopped?;
    let sealed = head.generation.lexical.manifest_generation;
    let pin = GenerationPin::new(repo.clone(), revision.clone(), sealed);
    let canonical = fs::canonicalize(&state_root)?;
    let identity = GenerationStorageKeyV1::for_repo_revision(&repo, &revision)
        .generation_dir(&canonical.join("indexes/lexical"), sealed)
        .join("search-corpus-generation-identity.cbor");

    let process = legacy.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let sockets = daemon_socket_paths(&state_root);
        let query = |request_id| -> TestResult {
            let request = SearchPlaneQueryIpcRequestEnvelope {
                request_id,
                diagnostic_attempt: quanta_index_contract::ipc::DiagnosticAttemptFieldV1::Absent,
                payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "needle".to_owned(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    top_k: 5,
                    cursor: None,
                }),
            };
            let response: SearchPlaneQueryIpcResponseEnvelope = quanta_index_ipc::send_request(
                &sockets[0],
                &request,
                quanta_index_ipc::ClientIoPolicy::default(),
            )?;
            if !matches!(response.payload, SearchPlaneQueryIpcResponse::Text(_)) {
                return Err(
                    format!("binary inventory query returned {:?}", response.payload).into(),
                );
            }
            Ok(())
        };
        query(0x1a11)?;
        let before = client.observability().metrics_snapshot()?;
        let metric = "maintenance_inventory_admission_failures_total";
        let initial = counter(&before, metric).ok_or("inventory failure metric absent")?;
        if !before
            .gauges
            .iter()
            .any(|point| point.name == "snapshot_registry_lexical_entries" && point.value >= 1.0)
        {
            return Err("lexical query did not leave a resident handle to revalidate".into());
        }

        let permissions = fs::metadata(&identity)?.permissions();
        fs::set_permissions(&identity, fs::Permissions::from_mode(0o000))?;
        let denied = (|| -> TestResult {
            let after = wait_for(
                &RealTicker::new(),
                Duration::from_secs(20),
                Duration::from_millis(50),
                "binary inventory read failure counter",
                || client.observability().metrics_snapshot(),
                |snapshot| counter(snapshot, metric).is_some_and(|value| value > initial),
                |_| true,
            )?;
            if counter(&after, metric).is_none_or(|value| value <= initial) {
                return Err("inventory read failure was not counted".into());
            }
            Ok(())
        })();
        fs::set_permissions(&identity, permissions)?;
        denied?;
        query(0x1a12)
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    legacy.verify()?;
    Ok(())
}

#[test]
fn reactivated_generation_reproves_physical_authority_after_aba() -> TestResult {
    let parent = private_short_root()?;
    let state_root = parent.path().join("state");
    let legacy = LegacySearchdBinary::from_environment()?;
    let (repo, revision) = owner()?;
    let process = legacy.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let (first_head, _first_receipt) = publish_legacy_source(
            &client,
            &repo,
            &revision,
            1,
            None,
            &[("src/ready.rs", "fn first() {}", 1)],
            None,
        )?;
        let first = first_head.generation.clone();
        require_eq(&wait_ready(&client)?.ready, &true, "first readiness")?;

        let (second_head, _second_receipt) = publish_legacy_source(
            &client,
            &repo,
            &revision,
            2,
            Some(("src/ready.rs", "fn first() {}", 1)),
            &[("src/ready.rs", "fn second() {}", 2)],
            Some(first_head.clone()),
        )?;
        if second_head.generation == first {
            return Err("second activation did not change the generation".into());
        }
        // Do not poll readiness at B: the cached proof must still be for A.
        let rollback =
            client
                .generations()
                .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                    expected_active: second_head,
                    target: first.clone(),
                })?;
        require_eq(
            &rollback.active.generation,
            &first,
            "rolled-back generation",
        )?;
        let returned_head = client
            .generations()
            .active_head(repo.clone(), revision.clone())?
            .ok_or("first generation is active again")?;
        require_eq(&returned_head.generation, &first, "returned generation")?;
        if returned_head.activation_token == first_head.activation_token {
            return Err("A -> B -> A must issue a new activation token".into());
        }

        let storage_key = GenerationStorageKeyV1::for_repo_revision(&repo, &revision);
        let manifest = fs::canonicalize(&state_root)?
            .join("indexes/lexical")
            .join(storage_key.as_str())
            .join(format!("g{}", first.lexical.manifest_generation.get()))
            .join("text-authority/manifest.cbor");
        let original = fs::read(&manifest)?;
        let mut corrupted = original.clone();
        let last = corrupted.last_mut().ok_or("sealed manifest is empty")?;
        *last ^= 0xff;
        fs::write(&manifest, corrupted)?;
        let damaged_result = (|| -> TestResult {
            let report = client.observability().process_readiness()?;
            require_eq(
                &report.ready,
                &false,
                "damaged returned generation readiness",
            )?;
            require_eq(
                &report.active_candidate_integrity,
                &Some(false),
                "damaged returned generation physical proof",
            )?;
            if !report
                .not_ready_reasons
                .contains(&ProcessReadinessReasonV1::ActiveCandidateIntegrityFailed)
            {
                return Err(format!("missing active integrity reason: {report:?}").into());
            }
            Ok(())
        })();
        fs::write(&manifest, original)?;
        damaged_result?;
        require_eq(&wait_ready(&client)?.ready, &true, "restored readiness")
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    legacy.verify()?;
    Ok(())
}
