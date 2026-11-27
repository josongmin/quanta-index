//! Real XFS project quota fixtures for the stock SQLite Native process.

use std::os::unix::fs::{PermissionsExt as _, symlink};

use quanta_index_searchd::app::{
    runtime::StateRootLease,
    state_root_quota::{STOCK_SQLITE_TEMP_DIR_NAME, StateRootProjectQuotaV1},
};
use quanta_index_searchd_harness::product_xfs::{MAX_PROCESS_FDS, SCRATCH_RESERVED_BYTES};
use quanta_state_root_quota::read_xfs_project_quota_via;
use rustix::{
    fs::{FallocateFlags, fallocate, fstatvfs},
    io::Errno,
};

use super::*;

#[path = "../../common/xfs_inode_pressure.rs"]
mod xfs_inode_pressure;
#[path = "../../common/xfs_project_pressure.rs"]
mod xfs_project_pressure;

pub(super) struct Fixture {
    directory: tempfile::TempDir,
    _control: tempfile::TempDir,
    pub(super) socket: PathBuf,
    config_path: PathBuf,
    sqlite_temp: PathBuf,
    binary: PathBuf,
    quota: StateRootProjectQuotaV1,
    verifier_socket: PathBuf,
    hard_bytes: u64,
    hard_inodes: u64,
}

impl Fixture {
    pub(super) fn new() -> TestResult<Self> {
        let binary = std::env::var_os("QUANTA_INDEX_SEARCHD_BIN").map_or_else(
            || PathBuf::from(env!("CARGO_BIN_EXE_quanta-index-searchd")),
            PathBuf::from,
        );
        require(
            binary.is_absolute(),
            "stock XFS daemon path must be absolute",
        )?;
        let project = PathBuf::from(
            std::env::var_os("QIDX_NATIVE_XFS_TEST_PARENT")
                .ok_or("missing QIDX_NATIVE_XFS_TEST_PARENT")?,
        )
        .canonicalize()?;
        let directory = tempfile::Builder::new()
            .prefix("qidx-stock-")
            .tempdir_in(project)?;
        let state_root = directory.path().join("state");
        fs::create_dir(&state_root)?;
        fs::set_permissions(&state_root, fs::Permissions::from_mode(0o700))?;
        drop(StateRootLease::acquire(&state_root)?);
        let sqlite_temp = state_root.join(STOCK_SQLITE_TEMP_DIR_NAME);
        fs::create_dir(&sqlite_temp)?;
        fs::set_permissions(&sqlite_temp, fs::Permissions::from_mode(0o700))?;

        let control = quanta_index_searchd_harness::private_tempdir()?;
        let socket_dir = control.path().join("uds");
        fs::create_dir(&socket_dir)?;
        fs::set_permissions(&socket_dir, fs::Permissions::from_mode(0o700))?;
        let socket = socket_dir.join("native.sock");
        let config_path = control.path().join("native-stock-xfs.json");
        let mut configuration = config_with_reclaim(&state_root, &socket)?;
        set_fixture_json(
            &mut configuration,
            "/resources/scratch_reserved_bytes",
            serde_json::json!(SCRATCH_RESERVED_BYTES),
        )?;
        let verifier_socket = PathBuf::from(
            std::env::var_os("QIDX_NATIVE_XFS_VERIFIER_SOCKET")
                .ok_or("missing QIDX_NATIVE_XFS_VERIFIER_SOCKET")?,
        );
        let project_id = std::env::var("QIDX_NATIVE_XFS_PROJECT_ID")?.parse::<u32>()?;
        let hard_bytes = std::env::var("QIDX_NATIVE_XFS_HARD_BYTES")?.parse::<u64>()?;
        let hard_inodes = std::env::var("QIDX_NATIVE_XFS_HARD_INODES")?.parse::<u64>()?;
        let quota = StateRootProjectQuotaV1::new(
            verifier_socket.clone(),
            project_id,
            hard_bytes,
            hard_inodes,
        )?;
        configuration["state_root_quota"] = serde_json::json!({
            "verifier_socket": verifier_socket,
            "project_id": project_id,
            "hard_bytes": hard_bytes,
            "hard_inodes": hard_inodes,
        });
        configuration["stock_sqlite_max_process_fds"] = serde_json::json!(MAX_PROCESS_FDS);
        fs::write(&config_path, serde_json::to_vec(&configuration)?)?;
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))?;

        Ok(Self {
            directory,
            _control: control,
            socket,
            config_path,
            sqlite_temp,
            binary,
            quota,
            verifier_socket,
            hard_bytes,
            hard_inodes,
        })
    }

    pub(super) fn launch(&self) -> TestResult<Daemon> {
        self.preflight()?;
        let command = ProductXfsProfile::command(&self.binary, &self.sqlite_temp);
        Daemon::start_command_before(command, &self.config_path, &self.socket, None, None, None)
    }

    pub(super) fn preflight(&self) -> TestResult {
        ProductXfsProfile::preflight(&self.binary, &self.config_path, &self.sqlite_temp)?;
        Ok(())
    }

    fn exhaust_blocks(&self) -> TestResult<Pressure> {
        require(
            self.hard_bytes <= 4_u64 << 30,
            "the disposable quota pressure fixture is bounded to 4 GiB",
        )?;
        // A sibling of StateRoot shares its XFS project without becoming a
        // foreign Native layout entry. Verify both this directory and filler
        // with the same kernel/project checks used by the product.
        let directory = tempfile::Builder::new()
            .prefix("quota-pressure-")
            .tempdir_in(self.directory.path())?;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))?;
        let lease = StateRootLease::acquire(directory.path())?;
        let filler = tempfile::NamedTempFile::new_in(lease.state_root_identity_v1())?;
        self.quota.require(&lease, &self.socket, 4, &budget())?;
        let project = lease.borrow_root_directory_read_v1()?;
        let block_bytes = fstatvfs(filler.as_file())?.f_frsize;
        xfs_project_pressure::fill_until_quota(
            filler.as_file(),
            &project,
            &self.verifier_socket,
            self.hard_bytes,
            block_bytes,
        )?;
        Ok(Pressure {
            directory,
            filler,
            block_bytes,
        })
    }

    fn exhaust_inodes(&self) -> TestResult<InodePressure> {
        require(
            self.hard_inodes <= xfs_inode_pressure::MAX_INODE_PRESSURE_LIMIT,
            "the disposable XFS inode fixture requires a fixed limit at most 4096",
        )?;
        let directory = tempfile::Builder::new()
            .prefix("inode-pressure-")
            .tempdir_in(self.directory.path())?;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))?;
        let lease = StateRootLease::acquire(directory.path())?;
        self.quota.require(&lease, &self.socket, 4, &budget())?;
        let project = lease.borrow_root_directory_read_v1()?;
        let created = xfs_inode_pressure::fill_inodes_until_quota(
            directory.path(),
            &project,
            &self.verifier_socket,
            self.hard_bytes,
            self.hard_inodes,
        )?;
        Ok(InodePressure {
            directory,
            created,
            verifier_socket: self.verifier_socket.clone(),
            hard_inodes: self.hard_inodes,
        })
    }
}

#[test]
#[ignore = "requires a disposable enforced XFS project and its privileged verifier socket"]
fn xfs_writer_preflight_rejects_special_inode_then_recovers() -> TestResult {
    let fixture = Fixture::new()?;
    let root = fixture.directory.path().join("state");
    let target = fixture._control.path().join("outside-target");
    fs::write(&target, b"unchanged target")?;
    let leaf = root.join("redirected-leaf");
    symlink(&target, &leaf)?;
    let refusal =
        ProductXfsProfile::preflight(&fixture.binary, &fixture.config_path, &fixture.sqlite_temp)
            .err()
            .ok_or("symlinked StateRoot passed writer preflight")?;
    require(
        refusal
            .to_string()
            .contains("StateRoot project tree contains a special inode"),
        format!("symlinked StateRoot refused for another reason: {refusal:#}"),
    )?;
    require(
        fs::symlink_metadata(&leaf)?.file_type().is_symlink()
            && fs::read(&target)? == b"unchanged target"
            && !fixture.socket.exists(),
        "writer preflight changed the symlink, target or service socket",
    )?;
    fs::remove_file(&leaf)?;
    fixture.preflight()?;
    require(
        !fixture.socket.exists(),
        "recovered writer preflight created a service socket",
    )
}

struct Pressure {
    directory: tempfile::TempDir,
    filler: tempfile::NamedTempFile,
    block_bytes: u64,
}

impl Pressure {
    fn release(self) -> TestResult {
        let parent = fs::File::open(self.directory.path())
            .map_err(|error| format!("open XFS pressure directory: {error}"))?;
        self.filler
            .close()
            .map_err(|error| format!("remove XFS quota filler: {error}"))?;
        parent
            .sync_all()
            .map_err(|error| format!("sync XFS quota filler removal: {error}"))?;
        let probe = tempfile::NamedTempFile::new_in(self.directory.path())
            .map_err(|error| format!("create XFS quota refund probe: {error}"))?;
        fallocate(
            probe.as_file(),
            FallocateFlags::empty(),
            0,
            self.block_bytes,
        )
        .map_err(|error| format!("allocate one XFS block after filler removal: {error}"))?;
        probe
            .close()
            .map_err(|error| format!("remove XFS quota refund probe: {error}"))?;
        parent
            .sync_all()
            .map_err(|error| format!("sync XFS quota refund probe removal: {error}"))?;
        Ok(())
    }
}

struct InodePressure {
    directory: tempfile::TempDir,
    created: u64,
    verifier_socket: PathBuf,
    hard_inodes: u64,
}

impl InodePressure {
    fn release(self) -> TestResult {
        let parent = fs::File::open(self.directory.path())?;
        for index in 0..self.created {
            fs::remove_file(
                self.directory
                    .path()
                    .join(format!("inode-pressure-{index:04}")),
            )?;
        }
        parent.sync_all()?;
        let refunded = read_xfs_project_quota_via(&self.verifier_socket, &parent)?;
        require(
            refunded.used_inodes < self.hard_inodes,
            "synced XFS inode removal did not refund the project",
        )?;
        let probe = tempfile::NamedTempFile::new_in(self.directory.path())?;
        probe.close()?;
        parent.sync_all()?;
        eprintln!(
            "XFS_INODE_QUOTA_REFUND used_inodes={} hard_inodes={} created={} probe=ok",
            refunded.used_inodes, self.hard_inodes, self.created,
        );
        Ok(())
    }
}

pub(super) fn publish_baseline(client: &NativeUdsClient) -> TestResult<(ViewRef, ObjectRef)> {
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("XFS Native daemon did not create its workspace".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![literal_spec(), text_spec()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    read_view_lifetime::publish_text(
        client,
        configured,
        1,
        1,
        None,
        "quota.rs",
        "xfsneedle xfsneedle",
    )
}

fn run_pre_admission_quota_case<P>(
    exhaust: impl FnOnce(&Fixture) -> TestResult<P>,
    release: impl FnOnce(P) -> TestResult,
) -> TestResult {
    let fixture = Fixture::new()?;
    let daemon = fixture.launch()?;
    let client = NativeUdsClient::new(fixture.socket.clone());
    let (view, old_source) = publish_baseline(&client)?;
    let latest = ViewSelection::Latest {
        workspace: workspace(),
        epoch: 1,
    };
    let baseline = client.read_view(latest, &budget())?;
    require_equal(&baseline.identity, &view, "selected stock XFS baseline")?;
    let search = |selected, text: &str| {
        client.search_text(
            ViewSelection::Exact(selected),
            text_spec().instance,
            TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: text.into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: 2,
                cursor: None,
            },
            &budget(),
        )
    };
    let baseline_ranked = search(view, "xfsneedle")?;
    require_equal(&baseline_ranked.matched, &1, "stock XFS baseline hit count")?;

    let (request, [new_source]) = read_view_lifetime::stage_text_update(
        &client,
        view,
        2,
        [read_view_lifetime::TextOwnerUpdate {
            owner: 1,
            expected: Some(old_source),
            path: "quota.rs",
            text: "xfsreplacement xfsreplacement",
        }],
    )?;
    let key = request.operation;
    let pressure =
        exhaust(&fixture).map_err(|error| format!("reach enforced XFS pressure: {error}"))?;
    let refused = client.submit_borrowed(&request, &budget());
    release(pressure)?;
    let space_errnos =
        [Errno::DQUOT, Errno::NOSPC].map(|errno| format!("os error {}", errno.raw_os_error()));
    // Submission can hit SQLite's ingress transaction before writing the
    // request object. Stock SQLite reports that failure as SQLITE_FULL.
    if !matches!(
        &refused,
        Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::Storage
                && (space_errnos.iter().any(|errno| error.message.contains(errno))
                    || error.message.contains("database or disk is full"))
    ) {
        return Err(format!(
            "expected Native storage refusal under verified XFS quota, got {refused:?}"
        )
        .into());
    }
    require_equal(
        &client.status(key, &budget())?,
        &OperationStatus::Unknown,
        "quota refusal before durable operation admission",
    )?;
    require_equal(
        &client.read_view(latest, &budget())?,
        &baseline,
        "quota refusal preserved the original view and Source frontier",
    )?;
    require_equal(
        &search(view, "xfsneedle")?,
        &baseline_ranked,
        "original exact view runs the same search after quota refusal",
    )?;

    let published = submit_and_settle(&client, &request)?;
    require(published != view, "same-key retry did not advance the view")?;
    let ranked = search(published, "xfsreplacement")?;
    require_equal(&ranked.matched, &1, "same-key retry published one hit")?;
    let [hit] = ranked.hits.as_slice() else {
        return Err("quota retry did not return exactly one hit".into());
    };
    require_equal(
        &hit.source,
        &new_source,
        "same-key retry published the staged Source",
    )?;
    daemon.stop()
}

#[test]
#[ignore = "requires a disposable enforced XFS project and its privileged verifier socket"]
fn xfs_block_quota_before_admission_preserves_view_and_retries_same_request() -> TestResult {
    run_pre_admission_quota_case(Fixture::exhaust_blocks, Pressure::release)
}

#[test]
#[ignore = "requires a disposable enforced XFS project and its privileged verifier socket"]
fn xfs_inode_quota_before_admission_preserves_view_and_retries_same_request() -> TestResult {
    run_pre_admission_quota_case(Fixture::exhaust_inodes, InodePressure::release)
}
