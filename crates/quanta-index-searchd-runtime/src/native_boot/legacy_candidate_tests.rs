//! Q-local negative: real SDK publication and BOOT do not validate Legacy.
//! No signed producer, Host/SDK custody or installed recovery is simulated.
use super::super::*;
use crate::deployment_selection::{
    DeploymentRecoveryModeV1, DeploymentSelectionRequestV1, with_locked_deployment_selection_v1,
};
use quanta_index_contract::{
    index::{
        CollectionId, GroupActivation, IndexInstanceId, IndexProfile, InputDomain, OperationKey,
        OperationStatus, OwnerKey, OwnerMutation, PublicationGroupId, SlotId, SourceEvent,
        SourceInput, SourceOperationMode, SourceTextAdmission, SourceTransition, StableOwnerId,
        StreamId, SubmitOutcome, UpdateKind, UpdateRequest, ViewRef, ViewSelection, WorktreeId,
    },
    native_cutover_evidence_v1::NativeCutoverEvidenceTargetV1,
    native_protocol::{NativeIndexRequest, NativeIndexResponse},
};
use quanta_index_sdk::native_uds::NativeUdsClient;
use std::{
    error::Error,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
};

#[path = "../../tests/common/native_physical_profile.rs"]
mod native_physical_profile;
#[path = "../../tests/common/legacy_physical_fixture.rs"]
mod legacy_physical_fixture;

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;
fn require(condition: bool, message: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
// Failure-only context shared by in-process BOOT fixture entry points.
// File properties are observations, not another verifier or an admission rule.
fn boot_fixture_failure_context(root: &Path) -> String {
    let mut context = String::from("offline BOOT fixture Catalog inputs");
    let catalog =
        quanta_index_catalog::catalog_dir(root).join(quanta_index_catalog::CATALOG_FILE_NAME);
    let wal = catalog.with_file_name(format!("{}-wal", quanta_index_catalog::CATALOG_FILE_NAME));
    for path in [&catalog, &wal] {
        let observed = match fs::symlink_metadata(path) {
            Ok(metadata) => format!(
                "mode={:o} uid={} effective_uid={} nlink={} regular={}",
                metadata.mode() & 0o7777,
                metadata.uid(),
                rustix::process::geteuid().as_raw(),
                metadata.nlink(),
                metadata.is_file()
            ),
            Err(error) => format!("metadata_error={error}"),
        };
        context.push_str(&format!("; Catalog input {}: {observed}", path.display()));
    }
    context
}
fn request() -> RequestBudgetV1 {
    RequestBudgetV1::for_duration(Duration::from_secs(60))
}
fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}
fn operation(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence,
    }
}
fn text_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([71; 16]),
        kind: IndexKind([72; 16]),
        group: PublicationGroupId([73; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([74; 16]),
        policy: TextRecipe::new(Digest([31; 32])).output_policy(),
        format: StorageFormatId([75; 16]),
    }
}
fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        _ => Err("actual fixture publication failed".into()),
    }
}
fn sha_file(path: &Path) -> TestResult<Digest> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut chunk = [0; 65536];
    loop {
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        hasher.update(&chunk[..count]);
    }
    Ok(Digest(hasher.finalize().into()))
}
fn hex(digest: Digest) -> String {
    digest.0.iter().map(|byte| format!("{byte:02x}")).collect()
}
struct RunningFixture {
    shutdown: quanta_index_ipc::ShutdownHandle,
    thread: Option<thread::JoinHandle<Result<()>>>,
}
impl RunningFixture {
    fn stop(&mut self) -> TestResult {
        self.shutdown.trigger();
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| "fixture service panicked")??;
        }
        Ok(())
    }
    fn finish(mut self) -> TestResult {
        self.stop()
    }
}
impl Drop for RunningFixture {
    fn drop(&mut self) {
        self.shutdown.trigger();
        if let Some(thread) = self.thread.take() {
            let _result = thread.join();
        }
    }
}

struct PreparedNativeFixture {
    _temp: tempfile::TempDir,
    base: std::path::PathBuf,
    root: std::path::PathBuf,
    legacy: std::path::PathBuf,
    config_path: std::path::PathBuf,
    receipt_path: std::path::PathBuf,
    binary: std::path::PathBuf,
    binary_digest: Digest,
    config_digest: Digest,
    config: ValidatedConfig,
    client: NativeUdsClient,
    running: RunningFixture,
    profile: quanta_index_contract::index::ObjectRef,
    configured: ViewRef,
    prior: quanta_index_contract::index::CatalogView,
    captured: UpdateRequest,
    native_view: ViewRef,
    record: quanta_index_contract::index::ObjectRef,
}
fn prepare_native_fixture() -> TestResult<PreparedNativeFixture> {
    // This in-process harness bypasses run_command_inner. Match its real
    // daemon entry condition before any adapter creates Catalog inputs.
    let _inherited_umask = quanta_index_searchd::app::harden_umask();
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let base = temp.path().canonicalize()?;
    let root = base.join("native");
    let legacy = base.join("legacy");
    let socket = base.join("native.sock");
    let config_path = base.join("native.json");
    let receipt_path = base.join("boot.json");
    let binary = std::env::current_exe()?.canonicalize()?;
    let binary_digest = sha_file(&binary)?;
    let mut configuration = config(&root, &socket);
    // A declared fixture IO envelope for three exact-image reads. This test
    // is not an SDK60 benchmark and changes no production or benchmark cap.
    let binary_bytes = fs::metadata(&binary)?.len();
    let image_work = binary_bytes
        .div_ceil(65536)
        .checked_add(1)
        .and_then(|units| units.checked_mul(65536))
        .and_then(|bytes| bytes.checked_mul(3))
        .and_then(|bytes| bytes.checked_add(64 * 1024 * 1024))
        .ok_or("fixture image work overflow")?;
    *configuration
        .pointer_mut("/work_limits/bytes")
        .ok_or("fixture byte limit absent")? = serde_json::json!(image_work);
    let config_bytes = serde_json::to_vec(&configuration)?;
    fs::write(&config_path, &config_bytes)?;
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))?;
    let config_digest = Digest(Sha256::digest(&config_bytes).into());
    let config = read_config_checked(&config_path, Some(config_digest))?;
    let service = bind(&config, compose(&config)?, None, &request(), false)?;
    let shutdown = service.shutdown_handle();
    let running = RunningFixture {
        shutdown,
        thread: Some(
            thread::Builder::new()
                .name("legacy-negative-native".into())
                .spawn(move || service.run(Duration::from_millis(10)))?,
        ),
    };
    let client = NativeUdsClient::new(socket);
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &request(),
    )?
    else {
        return Err("fixture workspace was not created".into());
    };
    let input = client.begin_input(operation(1), &request())?;
    let profile = client.stage_profile(
        input,
        IndexProfile {
            instances: vec![text_spec()],
        },
        &request(),
    )?;
    let configured = published(client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &request(),
    )?)?;
    let input = client.begin_input(operation(2), &request())?;
    let sources = client.stage_sources(
        input,
        vec![SourceInput {
            path: "legacy-negative.rs".into(),
            bytes: b"actual Q source needle".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
            language: None,
        }],
        &request(),
    )?;
    let [source] = sources.as_slice() else {
        return Err("fixture source count differs".into());
    };
    let prior = client.read_view(ViewSelection::Exact(configured), &request())?;
    let owners = client.stage_mutations(
        input,
        [OwnerMutation {
            owner: OwnerKey {
                producer: operation(2).producer,
                stable_id: StableOwnerId([1; 16]),
            },
            expected: None,
            next: Some(*source),
        }],
        &request(),
    )?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(2).producer,
        stream: operation(2).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let captured = UpdateRequest {
        operation: operation(2),
        expected_view: configured,
        kind: UpdateKind::SourceAdvance(transition),
    };
    let native_view = published(client.submit(captured.clone(), &request())?)?;
    require(native_view != configured, "actual Full did not publish")?;
    Ok(PreparedNativeFixture {
        _temp: temp,
        base,
        root,
        legacy,
        config_path,
        receipt_path,
        binary,
        binary_digest,
        config_digest,
        config,
        client,
        running,
        profile,
        configured,
        prior,
        captured,
        native_view,
        record: *source,
    })
}

#[test]
fn q_owned_native_full_cannot_authorize_an_empty_legacy_candidate() -> TestResult {
    let PreparedNativeFixture {
        _temp,
        base,
        root,
        legacy,
        config_path,
        receipt_path,
        binary,
        binary_digest,
        config_digest,
        config,
        client: _,
        running,
        profile,
        configured,
        prior,
        captured,
        native_view: _,
        record: _,
    } = prepare_native_fixture()?;
    running.finish()?;
    let registry = compose_boot_registry(&config)?;
    let select = NativeStateCommandV1 {
        mode: NativeCommandModeV1::Select,
        config_path: config_path.clone(),
        identity: None,
        config_digest: Some(config_digest),
        install_catalog_digest: None,
        select_operation: Some(operation(2)),
        receipt_path: Some(receipt_path.clone()),
    };
    require(
        boot::execute(&select, &config, &registry, request())
            .map_err(|error| error.context(boot_fixture_failure_context(&root)))?
            .is_none(),
        "offline Select returned live authority",
    )?;
    let receipt: serde_json::Value = serde_json::from_slice(&fs::read(receipt_path)?)?;
    // Create and retire only the real canonical lease. acquire_existing must
    // succeed later; a missing lock must not mask the semantic-verifier gap.
    drop(quanta_index_searchd::app::runtime::StateRootLease::acquire(
        &legacy,
    )?);
    let entries = fs::read_dir(&legacy)?.collect::<std::io::Result<Vec<_>>>()?;
    require(
        entries.len() == 1
            && entries[0].file_name()
                == quanta_index_searchd::app::state_format::STATE_ROOT_LEASE_FILE_NAME,
        "Legacy fixture contains semantic state or lacks its real lease",
    )?;
    let selection_path = base.join("selection.json");
    let candidate_path = base.join("candidate.json");
    let mut before = serde_json::to_vec(&serde_json::json!({
        "schema_version": 2, "mode": "native", "binary": binary, "binary_sha256": hex(binary_digest),
        "state_root": root, "environment": {}, "native_config": config_path,
        "native_config_sha256": hex(config_digest), "native_boot_receipt": receipt,
    }))?;
    before.push(b'\n');
    let mut after = serde_json::to_vec(&serde_json::json!({
        "schema_version": 2, "mode": "legacy", "binary": binary, "binary_sha256": hex(binary_digest),
        "state_root": legacy, "environment": {},
    }))?;
    after.push(b'\n');
    for (path, bytes) in [(&selection_path, &before), (&candidate_path, &after)] {
        fs::write(path, bytes)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    let target = NativeCutoverEvidenceTargetV1 {
        operation: operation(2),
        expected_view: configured,
        expected_source: prior.source,
        expected_profile: profile,
        source_mode: SourceOperationMode::Advance,
        event_predecessor: 0,
        event_sequence: 1,
        text_admission: SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        // Original-only fields are not fabricated into producer evidence:
        // this private Q test exercises captured Q request equality only.
        owner_identity_policy_digest: Digest([0; 32]),
        canonical_target_digest: Digest([0; 32]),
    };
    let source = boot::RecoverySourceInputs {
        target: &target,
        baseline_candidate_root_nonce: None,
        request: &captured,
        current_target: &target,
        current_request: &captured,
        legacy_publication: None,
        joined_source: None,
        ast_work: None,
    };
    let selection = DeploymentSelectionRequestV1 {
        selection_path_v1: &selection_path,
        expected_current_sha256_v1: Digest(Sha256::digest(&before).into()),
        candidate_path_v1: &candidate_path,
        expected_candidate_sha256_v1: Digest(Sha256::digest(&after).into()),
        mode_v1: DeploymentRecoveryModeV1::RollbackLegacy,
    };
    let result =
        boot::with_admitted_control_budget(&config, request(), false, |budget, _cache| {
            Ok(with_locked_deployment_selection_v1(
                &selection,
                budget,
                |locked, budget| {
                    let native = locked
                        .current_v1()
                        .native_command_v1()
                        .cloned()
                        .ok_or_else(|| {
                            DeploymentSelectionErrorV1::RefusedBeforeRename(anyhow!(
                                "actual Native command absent"
                            ))
                        })?;
                    // This Q-only negative uses its actual installed identity;
                    // no Source/Host/ACK permission is supplied by the fixture.
                    let source = boot::RecoverySourceInputs {
                        baseline_candidate_root_nonce: native
                            .identity
                            .as_ref()
                            .map(|identity| Digest(identity.root_nonce)),
                        ..source
                    };
                    boot::publish_rollback_with_source_capture_v1(
                        &native,
                        &config,
                        &registry,
                        &source,
                        locked,
                        budget,
                        &mut |budget| Ok(budget.checkpoint()?),
                    )
                },
            ))
        })?;
    // Check mutation first: the old factory reaches publish after authenticating
    // the real Q FULL, proving this is not an unrelated-precondition refusal.
    require(
        fs::read(&selection_path)? == before,
        "empty Legacy candidate changed actual selection",
    )?;
    match result {
        Err(DeploymentSelectionErrorV1::RefusedBeforeRename(error)) => {
            #[cfg(target_os = "linux")]
            let expected = error.is::<boot::MissingLegacyPublicationInputV1>();
            #[cfg(not(target_os = "linux"))]
            let expected = matches!(
                error.downcast_ref::<quanta_index_core::index::existing_file::ExistingFileReadFailureV1>(),
                Some(quanta_index_core::index::existing_file::ExistingFileReadFailureV1::Canonical(
                    quanta_index_core::CoreError::NotImplemented(_),
                ))
            );
            if expected {
                Ok(())
            } else {
                Err(format!("wrong typed Legacy refusal: {error:#}").into())
            }
        }
        _ => Err("missing Legacy Source verifier was accepted or uncertain".into()),
    }
}
fn config(root: &Path, socket: &Path) -> serde_json::Value {
    let unit = serde_json::json!({
        "cpu_units": 1,
        "io_bytes": 67_108_864,
        "memory_bytes": 67_108_864,
        "provider_units": 0
    });
    let physical = native_physical_profile::profile();
    serde_json::json!({
        "version": 1,
        "state_root": root.to_string_lossy(),
        "socket_path": socket.to_string_lossy(),
        "work_limits": {"reads": 1_000_000, "writes": 1_000_000, "bytes": 536_870_912, "mutations": 1_000_000},
        "resources": {
            "catalog_writer_cache_kib": 8192,
            "physical": {
                "inventory": {
                    "max_bytes": physical.inventory.max_bytes,
                    "max_files": physical.inventory.max_files,
                    "max_pending": physical.inventory.max_pending,
                    "max_entries": physical.inventory.max_entries,
                    "max_depth": physical.inventory.max_depth,
                    "max_pending_frames": physical.inventory.max_pending_frames
                },
                "max_reader_handles": physical.max_reader_handles,
                "max_tracked_entries": physical.max_tracked_entries,
                "max_process_roots": physical.max_process_roots,
                "global_max_sqlite_handles": physical.global_max_sqlite_handles,
                "maintenance_reserved_bytes": physical.maintenance_reserved_bytes,
                "maintenance_reserved_files": physical.maintenance_reserved_files
            },
            "scratch_reserved_bytes": 17_179_869_184_u64,
            "read_ticket_lease_ms": 30000,
            "execution": {
                "capacity": {"cpu_units": 4, "io_bytes": 268_435_456, "memory_bytes": 335_544_320, "provider_units": 0},
                "control": unit,
                "interactive": unit,
                "core": unit,
                "enrichment": unit
            }
        },
        "literal": {"kind": vec![8_u8; 16], "schema": vec![10_u8; 16], "format": vec![12_u8; 16]},
        "file_map": {"kind": vec![18_u8; 16], "schema": vec![10_u8; 16], "format": vec![19_u8; 16]},
        "text": {
            "descriptor": {"kind": vec![72_u8; 16], "schema": vec![74_u8; 16], "format": vec![75_u8; 16]},
            "admission_policy": vec![31_u8; 32],
            "bm25": {"k1": 1.2, "b": 0.75},
            "read_need": unit
        }
    })
}

#[path = "capture_tests.rs"]
mod capture_tests;

#[path = "root_scoped_view_tests.rs"]
mod root_scoped_view_tests;

#[path = "restore_forward_tests.rs"]
mod restore_forward_tests;

#[path = "repomap_recovery_tests.rs"]
mod repomap_recovery_tests;
