#[path = "native_boot/offline_semantic.rs"]
mod offline_semantic;

use super::*;
use std::{collections::BTreeMap, os::unix::fs::MetadataExt as _, path::PathBuf, process::Output};

use sha2::{Digest as _, Sha256};

#[cfg(feature = "legacy-tantivy")]
#[path = "../common/searchd_fixture_env.rs"]
mod searchd_fixture_env;

#[test]
fn recovery_helper_returns_a_bound_startup_refusal_over_inherited_fd() -> TestResult {
    recovery_helper_startup_refusal_with_history_fd(false)
}

#[cfg(target_os = "linux")]
#[test]
fn recovery_helper_rejects_malformed_history_fd_with_bound_terminal() -> TestResult {
    recovery_helper_startup_refusal_with_history_fd(true)
}

fn recovery_helper_startup_refusal_with_history_fd(malformed_history_fd: bool) -> TestResult {
    use quanta_explicit_fd_process::{ExplicitFdSpawnRequestV1, PinnedExecutableV1};
    use quanta_index_contract::native_cutover_evidence_v1 as wire;
    use std::os::{fd::AsFd, unix::net::UnixDatagram};

    let temp = tempfile::tempdir()?;
    let config = temp.path().join("config.json");
    let selection = temp.path().join("selection.json");
    let candidate = temp.path().join("candidate.json");
    fs::write(&config, b"{}")?;
    fs::write(&selection, b"active selection must stay untouched")?;
    let binary = Path::new(env!("CARGO_BIN_EXE_quanta-index-searchd"));
    let mut executable = fs::File::open(binary)?;
    let mut hash = Sha256::new();
    let _hashed = std::io::copy(&mut executable, &mut hash)?;
    let binary_sha256 = hex(&hash.finalize())?;
    let pinned = PinnedExecutableV1::try_pin_v1(&executable, &binary_sha256)?;
    let current = hex(&[1; 32])?;
    let proposed = hex(&[2; 32])?;
    let incorrect_config = hex(&[3; 32])?;
    let begin = wire::NativeCutoverControlBeginV1 {
        format_version: wire::NativeCutoverControlBeginV1::FORMAT_VERSION_V1,
        operation_id: "actual-helper-startup-refusal".to_owned(),
        mode: wire::NativeCutoverRecoveryModeV1::RestoreForward,
        current_selection_sha256: Digest([1; 32]),
        candidate_selection_sha256: Digest([2; 32]),
        deadline_ns: u64::MAX,
        evidence_length: 1,
        evidence_sha256: Digest([4; 32]),
        nonce: Digest([5; 32]),
    };
    let mut frame = Vec::new();
    ciborium::ser::into_writer(&begin, &mut frame)?;
    let (resident, helper) = UnixDatagram::pair()?;
    // Exercise the no-History layout with FD4-6 mapped. A second case maps a
    // connected datagram to FD7 and requires a bound refusal before config IO.
    let inherited = (0..3)
        .map(|index| {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(temp.path().join(format!("inherited-{index}")))
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    resident.set_read_timeout(Some(NATIVE_STARTUP_LIMIT))?;
    let (malformed, _peer) = UnixDatagram::pair()?;
    let spawn = ExplicitFdSpawnRequestV1::try_new_v1(
        &pinned,
        [
            binary.as_os_str(),
            "recover-native".as_ref(),
            "--selection-path".as_ref(),
            selection.as_os_str(),
            "--expected-current-sha256".as_ref(),
            current.as_ref(),
            "--candidate-path".as_ref(),
            candidate.as_os_str(),
            "--expected-candidate-sha256".as_ref(),
            proposed.as_ref(),
            "--mode".as_ref(),
            "restore-forward".as_ref(),
            "--config".as_ref(),
            config.as_os_str(),
            "--config-digest".as_ref(),
            incorrect_config.as_ref(),
            "--parent-executable".as_ref(),
            binary.as_os_str(),
            "--parent-executable-sha256".as_ref(),
            binary_sha256.as_ref(),
        ],
        std::iter::empty::<(&str, &str)>(),
    )?
    .with_child_unix_datagram_v1(helper.as_fd(), 3)?
    .with_child_retained_regular_file_v1(inherited[0].as_fd(), 4)?
    .with_child_retained_regular_file_v1(inherited[1].as_fd(), 5)?
    .with_child_retained_regular_file_v1(inherited[2].as_fd(), 6)?;
    let spawn = if malformed_history_fd {
        spawn.with_child_unix_datagram_v1(malformed.as_fd(), 7)?
    } else {
        spawn
    };
    let mut child = spawn.spawn_v1()?;
    drop(helper);
    require_equal(&resident.send(&frame)?, &frame.len(), "Begin length")?;
    let mut response = [0; wire::MAX_NATIVE_CUTOVER_CONTROL_BYTES_V1 + 1];
    let len = resident.recv(&mut response)?;
    let observed = wire::decode_native_cutover_control_terminal_v1(
        response.get(..len).ok_or("oversized helper terminal")?,
    )?;
    let refusal = if malformed_history_fd {
        quanta_explicit_fd_process::ExplicitFdProcessErrorV1::InvalidInheritedRegularFileDescriptor
            .to_string()
    } else if cfg!(target_os = "linux") {
        quanta_index_core::index::IndexError::DigestConflict.to_string()
    } else {
        "recovery requires the original shared Linux monotonic deadline".to_owned()
    };
    require_equal(
        &observed,
        &wire::NativeCutoverControlTerminalV1::RefusedBeforeRename {
            operation_id: begin.operation_id,
            nonce: begin.nonce,
            code: "REFUSED_BEFORE_RENAME".to_owned(),
            message: refusal,
        },
        "bound startup terminal",
    )?;
    require_equal(&child.wait_v1()?.code(), &Some(2), "helper exit status")?;
    require_equal(
        &fs::read(&selection)?.as_slice(),
        &b"active selection must stay untouched".as_slice(),
        "selection bytes",
    )
}

fn owned_launcher_group(parent: u32) -> TestResult<Option<u32>> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid="])
        .output()?;
    require(
        output.status.success(),
        "cannot inspect launcher child group",
    )?;
    let listing = String::from_utf8(output.stdout)?;
    let mut found = None;
    for line in listing.lines() {
        let mut columns = line.split_whitespace();
        let (Some(process_id), Some(parent_id), Some(group_id), None) = (
            columns.next(),
            columns.next(),
            columns.next(),
            columns.next(),
        ) else {
            continue;
        };
        if parent_id.parse::<u32>()? != parent {
            continue;
        }
        let process_id = process_id.parse::<u32>()?;
        let group_id = group_id.parse::<u32>()?;
        require_equal(
            &process_id,
            &group_id,
            "launcher child is not its own process group",
        )?;
        require(
            found.replace(process_id).is_none(),
            "launcher has multiple direct children",
        )?;
    }
    Ok(found)
}

fn kill_owned_launcher_group(parent: u32, expected: Option<u32>) {
    // Recheck the direct-parent relationship immediately before signaling.
    // A stale recorded PGID must not target an unrelated reused process.
    if let Ok(Some(group)) = owned_launcher_group(parent)
        && expected.is_none_or(|known| known == group)
        && let Ok(raw) = i32::try_from(group)
        && let Some(pid) = rustix::process::Pid::from_raw(raw)
    {
        let _signal = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
}

fn hex(bytes: &[u8]) -> TestResult<String> {
    use std::fmt::Write as _;
    let mut encoded = String::new();
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}")?;
    }
    Ok(encoded)
}

fn bounded_output(mut command: Command, args: &[String], launcher: bool) -> TestResult<Output> {
    // A launcher child inherits stdout/stderr. Anonymous files keep a parent
    // exit from blocking forever while a grandchild still owns either FD.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = command
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .spawn()?;
    let deadline = Instant::now()
        .checked_add(NATIVE_STARTUP_LIMIT)
        .ok_or("native boot command deadline overflow")?;
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout_offset = std::io::Seek::seek(&mut stdout, std::io::SeekFrom::Start(0))?;
            let stderr_offset = std::io::Seek::seek(&mut stderr, std::io::SeekFrom::Start(0))?;
            require_equal(&stdout_offset, &0, "native command stdout rewind")?;
            require_equal(&stderr_offset, &0, "native command stderr rewind")?;
            let mut stdout_bytes = Vec::new();
            let mut stderr_bytes = Vec::new();
            let _read = std::io::Read::read_to_end(
                &mut std::io::Read::take(stdout, 16_384),
                &mut stdout_bytes,
            )?;
            let _read = std::io::Read::read_to_end(
                &mut std::io::Read::take(stderr, 16_384),
                &mut stderr_bytes,
            )?;
            return Ok(Output {
                status,
                stdout: stdout_bytes,
                stderr: stderr_bytes,
            });
        }
        if Instant::now() >= deadline {
            // The launcher forwards SIGTERM to its selected child. Give that
            // cooperative drain a bounded chance before forcing the parent.
            let _term = Command::new("kill")
                .arg("-TERM")
                .arg(child.id().to_string())
                .status();
            let drain_deadline = Instant::now()
                .checked_add(Duration::from_secs(5))
                .ok_or("native boot drain deadline overflow")?;
            while Instant::now() < drain_deadline {
                if child.try_wait()?.is_some() {
                    return Err(format!("native boot command timed out: {args:?}").into());
                }
                thread::sleep(Duration::from_millis(10));
            }
            if launcher {
                kill_owned_launcher_group(child.id(), None);
            }
            let _killed = child.kill();
            let _waited = child.wait();
            return Err(format!("native boot command did not finish: {args:?}").into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_one_shot(args: &[String]) -> TestResult<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"));
    let _args = command.args(args);
    bounded_output(command, args, false)
}

#[cfg(feature = "legacy-tantivy")]
fn run_private_pipe_one_shot(args: &[&str]) -> TestResult<Output> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now()
        .checked_add(NATIVE_STARTUP_LIMIT)
        .ok_or("History exporter process deadline overflow")?;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        if Instant::now() >= deadline {
            let _killed = child.kill();
            let _waited = child.wait();
            return Err("History exporter process exceeded startup deadline".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn launcher_script() -> TestResult<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/deployment/launcher.py")
        .canonicalize()?)
}

fn run_launcher(args: &[String]) -> TestResult<Output> {
    let mut command = Command::new("python3");
    let _args = command.arg(launcher_script()?).args(args);
    bounded_output(command, args, true)
}

fn launcher_succeeded(args: &[String]) -> TestResult {
    let output = run_launcher(args)?;
    require(
        output.status.success(),
        format!(
            "native launcher command failed: {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn launcher_refused(args: &[String]) -> TestResult {
    let output = run_launcher(args)?;
    require_equal(
        &output.status.code(),
        &Some(1),
        "native launcher refusal exit",
    )?;
    require(
        output.stdout.is_empty(),
        "native launcher refusal emitted output",
    )
}

fn successful(args: &[String]) -> TestResult<Output> {
    let output = run_one_shot(args)?;
    require(
        output.status.success(),
        format!(
            "native boot command failed: {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    Ok(output)
}

fn refused(args: &[String]) -> TestResult {
    let output = run_one_shot(args)?;
    require_equal(&output.status.code(), &Some(70), "native boot refusal exit")?;
    require(
        output.stdout.is_empty(),
        "refused command emitted a receipt",
    )
}

fn with_flag(args: &[String], flag: &str, value: String) -> TestResult<Vec<String>> {
    let mut changed = args.to_vec();
    let index = changed
        .iter()
        .position(|arg| arg == flag)
        .ok_or_else(|| format!("missing fixture flag {flag}"))?;
    *changed
        .get_mut(index.saturating_add(1))
        .ok_or_else(|| format!("missing fixture value for {flag}"))? = value;
    Ok(changed)
}

type RootFiles = BTreeMap<PathBuf, (u64, u64, Vec<u8>)>;

fn root_files(root: &Path) -> TestResult<RootFiles> {
    fn visit(root: &Path, path: &Path, files: &mut RootFiles) -> TestResult {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                visit(root, &path, files)?;
            } else {
                require(
                    metadata.is_file(),
                    format!("unexpected root entry: {}", path.display()),
                )?;
                let relative = path.strip_prefix(root)?.to_path_buf();
                let previous =
                    files.insert(relative, (metadata.dev(), metadata.ino(), fs::read(&path)?));
                require(previous.is_none(), "duplicate root entry")?;
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

fn identity_args(
    mode: &str,
    config: &Path,
    receipt: &serde_json::Value,
    config_digest: &str,
) -> TestResult<Vec<String>> {
    let field = |name: &str| -> TestResult<String> {
        Ok(receipt
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("missing receipt field {name}"))?
            .to_owned())
    };
    let mut args = vec![
        mode.into(),
        "--config".into(),
        config.to_string_lossy().into_owned(),
        "--root-nonce".into(),
        field("root_nonce_hex")?,
        "--manifest-operation".into(),
        serde_json::to_string(
            receipt
                .get("manifest_operation")
                .ok_or("missing manifest operation")?,
        )?,
        "--manifest-object".into(),
        field("manifest_object_hex")?,
        "--source-cut-digest".into(),
        field("source_cut_digest_hex")?,
        "--config-digest".into(),
        config_digest.into(),
    ];
    if matches!(mode, "verify-native-state" | "verify-native-candidate") {
        args.extend([
            "--catalog-content-digest".into(),
            field("install_catalog_digest_hex")?,
        ]);
    }
    Ok(args)
}

fn wait_for_socket(child: &mut Child, socket: &Path) -> TestResult {
    let deadline = Instant::now()
        .checked_add(NATIVE_STARTUP_LIMIT)
        .ok_or("selected native startup deadline overflow")?;
    loop {
        if UnixStream::connect(socket).is_ok() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _bytes = std::io::Read::read_to_string(&mut pipe, &mut stderr)?;
            }
            return Err(format!("selected native daemon exited {status}: {stderr}").into());
        }
        if Instant::now() >= deadline {
            return Err("selected native daemon did not bind UDS".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn selected_daemon(args: &[String], socket: &Path) -> TestResult<Daemon> {
    let child = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut daemon = Daemon(child, None);
    wait_for_socket(&mut daemon.0, socket)?;
    Ok(daemon)
}

struct LauncherDaemon {
    child: Child,
    owned_group: Option<u32>,
    stopped: bool,
}

impl LauncherDaemon {
    fn stop(mut self) -> TestResult {
        let signal = Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status()?;
        require(signal.success(), "failed to signal native launcher")?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or("native launcher shutdown deadline overflow")?;
        loop {
            if let Some(status) = self.child.try_wait()? {
                require(status.success(), format!("native launcher exited {status}"))?;
                self.stopped = true;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("native launcher did not drain after SIGTERM".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for LauncherDaemon {
    fn drop(&mut self) {
        if !self.stopped {
            kill_owned_launcher_group(self.child.id(), self.owned_group);
            let _killed = self.child.kill();
            let _reaped = self.child.wait();
        }
    }
}

fn launcher_daemon(selection: &Path, socket: &Path) -> TestResult<LauncherDaemon> {
    let child = Command::new("python3")
        .arg(launcher_script()?)
        .arg("serve")
        .arg("--selection")
        .arg(selection)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut daemon = LauncherDaemon {
        child,
        owned_group: None,
        stopped: false,
    };
    wait_for_socket(&mut daemon.child, socket)?;
    daemon.owned_group = owned_launcher_group(daemon.child.id())?;
    require(
        daemon.owned_group.is_some(),
        "launcher has no owned searchd child",
    )?;
    Ok(daemon)
}

#[test]
fn native_candidate_retains_predecessors_under_installed_retention_config() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let base = temp.path().canonicalize()?;
    let root = base.join("state");
    let socket = base.join("native.sock");
    let config_path = base.join("native-config.json");
    let mut configuration = config_with_reclaim(&root, &socket)?;
    let _prior = configuration.as_object_mut().ok_or("candidate fixture config")?.insert(
        "retention".into(),
        serde_json::json!({
            "scopes": [{ "workspace": workspace(), "epoch": 1 }],
            "sequence_window": 1, "page_limit": 1,
            "interval_ms": 20, "round_timeout_ms": 2000, "head_probe_every": 2,
            "work_limits": {"reads": 1_000_000, "writes": 1_000_000, "bytes": 536_870_912, "mutations": 1_000_000}
        }),
    );
    let config_bytes = serde_json::to_vec(&configuration)?;
    fs::write(&config_path, &config_bytes)?;
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))?;
    let digest = hex(&Sha256::digest(&config_bytes))?;
    let args = vec![
        "serve-native-candidate".into(),
        "--config".into(),
        config_path.to_string_lossy().into_owned(),
        "--config-digest".into(),
        digest,
    ];
    let candidate = selected_daemon(&args, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("candidate workspace creation failed".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![text_spec()],
        },
        &budget(),
    )?;
    let head = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    // Give the installed retention schedule multiple rounds before restarting
    // the unselected candidate with the same configuration.
    thread::sleep(Duration::from_millis(300));
    candidate.stop()?;
    let restarted = selected_daemon(&args, &socket)?;
    require_equal(
        &client
            .read_view(ViewSelection::Exact(genesis), &budget())?
            .identity,
        &genesis,
        "candidate retired its predecessor before BOOT",
    )?;
    require_equal(
        &client
            .read_view(
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget(),
            )?
            .identity,
        &head,
        "candidate restart changed the latest publication",
    )?;
    restarted.stop()?;

    // The same configuration still retires old views in regular serving mode.
    let active = Daemon::start(&config_path, &socket)?;
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        match client.read_view(ViewSelection::Exact(genesis), &budget()) {
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable =>
            {
                break;
            }
            Ok(_) if Instant::now() < until => thread::sleep(Duration::from_millis(20)),
            other => {
                return Err(
                    format!("active retention did not retire the predecessor: {other:?}").into(),
                );
            }
        }
    }
    active.stop()?;
    Ok(())
}

#[test]
fn native_boot_selection_verification_and_selected_serve_preserve_real_publication() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let base = temp.path().canonicalize()?;
    let root = base.join("state");
    let socket = base.join("native.sock");
    let config_path = base.join("native-config.json");
    let receipt_path = base.join("native-boot-receipt.json");
    let configuration = config(&root, &socket);
    let config_bytes = serde_json::to_vec(&configuration)?;
    fs::write(&config_path, &config_bytes)?;
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))?;
    let config_digest = hex(&Sha256::digest(&config_bytes))?;

    let serve_candidate_args = vec![
        "serve-native-candidate".into(),
        "--config".into(),
        config_path.to_string_lossy().into_owned(),
        "--config-digest".into(),
        config_digest.clone(),
    ];
    let daemon = selected_daemon(&serve_candidate_args, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("native boot workspace creation failed".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![text_spec()],
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
    daemon.stop()?;

    let select_args = vec![
        "select-native-state".into(),
        "--config".into(),
        config_path.to_string_lossy().into_owned(),
        "--manifest-operation".into(),
        serde_json::to_string(&operation(1))?,
        "--receipt".into(),
        receipt_path.to_string_lossy().into_owned(),
        "--config-digest".into(),
        config_digest.clone(),
    ];
    require(!socket.exists(), "plain native stop retained its socket")?;
    fs::write(&receipt_path, b"existing private receipt\n")?;
    fs::set_permissions(&receipt_path, fs::Permissions::from_mode(0o600))?;
    let existing_receipt = fs::read(&receipt_path)?;
    let existing_metadata = fs::metadata(&receipt_path)?;
    let before_existing_target = root_files(&root)?;
    refused(&select_args)?;
    let after_existing_metadata = fs::metadata(&receipt_path)?;
    require_equal(
        &fs::read(&receipt_path)?,
        &existing_receipt,
        "existing receipt bytes after refused select",
    )?;
    require_equal(
        &(after_existing_metadata.dev(), after_existing_metadata.ino()),
        &(existing_metadata.dev(), existing_metadata.ino()),
        "existing receipt identity after refused select",
    )?;
    require_equal(
        &root_files(&root)?,
        &before_existing_target,
        "existing receipt refusal root bytes/inodes",
    )?;
    require(
        !socket.exists(),
        "existing receipt refusal created a socket",
    )?;
    fs::remove_file(&receipt_path)?;
    let _selected = successful(&select_args)?;
    let receipt_bytes = fs::read(&receipt_path)?;
    let receipt: serde_json::Value = serde_json::from_slice(&receipt_bytes)?;
    let fields = receipt.as_object().ok_or("boot receipt is not an object")?;
    require_equal(&fields.len(), &5, "boot receipt field count")?;
    for name in [
        "root_nonce_hex",
        "manifest_operation",
        "manifest_object_hex",
        "source_cut_digest_hex",
        "install_catalog_digest_hex",
    ] {
        require(
            fields.contains_key(name),
            format!("missing receipt field {name}"),
        )?;
    }
    require_equal(
        &serde_json::from_value::<OperationKey>(
            receipt
                .get("manifest_operation")
                .ok_or("missing receipt field manifest_operation")?
                .clone(),
        )?,
        &operation(1),
        "receipt published operation",
    )?;
    let receipt_mode = fs::metadata(&receipt_path)?.permissions().mode();
    require(
        receipt_mode.trailing_zeros() >= 6,
        "boot receipt is not private",
    )?;

    let candidate_receipt_path = base.join("candidate-native-boot-receipt.json");
    let candidate_args = vec![
        "select-native-candidate".into(),
        "--config".into(),
        config_path.to_string_lossy().into_owned(),
        "--manifest-operation".into(),
        serde_json::to_string(&operation(1))?,
        "--receipt".into(),
        candidate_receipt_path.to_string_lossy().into_owned(),
        "--config-digest".into(),
        config_digest.clone(),
        "--root-nonce".into(),
        receipt["root_nonce_hex"]
            .as_str()
            .ok_or("missing receipt root nonce")?
            .into(),
        "--source-cut-digest".into(),
        receipt["source_cut_digest_hex"]
            .as_str()
            .ok_or("missing receipt SourceCut digest")?
            .into(),
    ];
    let mut wrong_source_cut = receipt["source_cut_digest_hex"]
        .as_str()
        .ok_or("missing receipt SourceCut digest")?
        .to_owned();
    wrong_source_cut.replace_range(
        0..1,
        if wrong_source_cut.starts_with('1') {
            "2"
        } else {
            "1"
        },
    );
    let wrong_candidate_args = with_flag(&candidate_args, "--source-cut-digest", wrong_source_cut)?;
    let before_wrong_candidate = root_files(&root)?;
    refused(&wrong_candidate_args)?;
    require_equal(
        &root_files(&root)?,
        &before_wrong_candidate,
        "wrong candidate SourceCut changed root",
    )?;
    require(
        !candidate_receipt_path.exists(),
        "wrong candidate SourceCut wrote receipt",
    )?;
    let mut wrong_root_nonce = receipt["root_nonce_hex"]
        .as_str()
        .ok_or("missing receipt root nonce")?
        .to_owned();
    wrong_root_nonce.replace_range(
        0..1,
        if wrong_root_nonce.starts_with('1') {
            "2"
        } else {
            "1"
        },
    );
    let wrong_nonce_args = with_flag(&candidate_args, "--root-nonce", wrong_root_nonce)?;
    refused(&wrong_nonce_args)?;
    require_equal(
        &root_files(&root)?,
        &before_wrong_candidate,
        "wrong candidate root nonce changed root",
    )?;
    require(
        !candidate_receipt_path.exists(),
        "wrong candidate root nonce wrote receipt",
    )?;
    let _selected = successful(&candidate_args)?;
    let candidate_receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&candidate_receipt_path)?)?;
    require_equal(&candidate_receipt, &receipt, "same candidate retry receipt")?;
    let before_candidate_restart = root_files(&root)?;
    refused(&serve_candidate_args)?;
    require_equal(
        &root_files(&root)?,
        &before_candidate_restart,
        "candidate serve must not reopen a selected BOOT as writable",
    )?;

    let verify_args = identity_args(
        "verify-native-state",
        &config_path,
        &receipt,
        &config_digest,
    )?;
    let verify_candidate_args = identity_args(
        "verify-native-candidate",
        &config_path,
        &receipt,
        &config_digest,
    )?;
    let serve_args = identity_args("serve-native", &config_path, &receipt, &config_digest)?;
    let before_verify = root_files(&root)?;
    let _verified = successful(&verify_args)?;
    let _verified_candidate = successful(&verify_candidate_args)?;
    require_equal(
        &root_files(&root)?,
        &before_verify,
        "verify root bytes/inodes",
    )?;

    let selection_path = base.join("native-selection.json");
    let binary = Path::new(env!("CARGO_BIN_EXE_quanta-index-searchd")).canonicalize()?;
    let install_args = vec![
        "install-native".into(),
        "--selection".into(),
        selection_path.to_string_lossy().into_owned(),
        "--binary".into(),
        binary.to_string_lossy().into_owned(),
        "--state-root".into(),
        root.to_string_lossy().into_owned(),
        "--config".into(),
        config_path.to_string_lossy().into_owned(),
        "--receipt".into(),
        receipt_path.to_string_lossy().into_owned(),
    ];
    launcher_succeeded(&install_args)?;
    let selection_bytes = fs::read(&selection_path)?;
    let selection: serde_json::Value = serde_json::from_slice(&selection_bytes)?;
    require_equal(
        &selection
            .as_object()
            .ok_or("native selection is not an object")?
            .len(),
        &9,
        "native selection field count",
    )?;
    require_equal(
        selection
            .get("schema_version")
            .ok_or("missing selection field schema_version")?,
        &serde_json::json!(2),
        "selection version",
    )?;
    require_equal(
        selection
            .get("mode")
            .ok_or("missing selection field mode")?,
        &serde_json::json!("native"),
        "selection mode",
    )?;
    require_equal(
        selection
            .get("binary")
            .ok_or("missing selection field binary")?,
        &serde_json::json!(binary),
        "selection binary",
    )?;
    require_equal(
        selection
            .get("state_root")
            .ok_or("missing selection field state_root")?,
        &serde_json::json!(root),
        "selection root",
    )?;
    require_equal(
        selection
            .get("native_config")
            .ok_or("missing selection field native_config")?,
        &serde_json::json!(config_path),
        "selection config",
    )?;
    require_equal(
        selection
            .get("native_config_sha256")
            .ok_or("missing selection field native_config_sha256")?,
        &serde_json::json!(config_digest),
        "selection raw config digest",
    )?;
    require_equal(
        selection
            .get("native_boot_receipt")
            .ok_or("missing selection field native_boot_receipt")?,
        &receipt,
        "selection receipt",
    )?;
    require(
        fs::metadata(&selection_path)?
            .permissions()
            .mode()
            .trailing_zeros()
            >= 6,
        "native selection is not private",
    )?;

    let changed = |flag: &str, value: String| with_flag(&verify_args, flag, value);
    for args in [
        changed("--root-nonce", "aa".repeat(32))?,
        changed("--manifest-object", "bb".repeat(32))?,
        changed("--source-cut-digest", "cc".repeat(32))?,
        changed("--catalog-content-digest", "dd".repeat(32))?,
        changed("--config-digest", "ee".repeat(32))?,
    ] {
        refused(&args)?;
        require_equal(
            &root_files(&root)?,
            &before_verify,
            "refused verify root bytes/inodes",
        )?;
        require(!socket.exists(), "refused verify created a socket")?;
    }
    for (flag, value) in [
        ("--root-nonce", "aa".repeat(32)),
        ("--manifest-object", "bb".repeat(32)),
        ("--source-cut-digest", "cc".repeat(32)),
        ("--config-digest", "ee".repeat(32)),
    ] {
        refused(&with_flag(&serve_args, flag, value)?)?;
        require_equal(
            &root_files(&root)?,
            &before_verify,
            "refused selected serve root bytes/inodes",
        )?;
        require(!socket.exists(), "refused selected serve created a socket")?;
    }
    let changed_config_path = base.join("changed-native-config.json");
    fs::write(
        &changed_config_path,
        serde_json::to_vec_pretty(&configuration)?,
    )?;
    refused(&with_flag(
        &verify_args,
        "--config",
        changed_config_path.to_string_lossy().into_owned(),
    )?)?;
    require_equal(
        &root_files(&root)?,
        &before_verify,
        "wrong config root bytes/inodes",
    )?;

    let missing_root = base.join("missing-root");
    let mut missing_config = configuration.clone();
    set_fixture_json(
        &mut missing_config,
        "/state_root",
        serde_json::json!(missing_root.to_string_lossy()),
    )?;
    let missing_config_path = base.join("missing-native-config.json");
    let missing_bytes = serde_json::to_vec(&missing_config)?;
    fs::write(&missing_config_path, &missing_bytes)?;
    let missing_args = identity_args(
        "verify-native-state",
        &missing_config_path,
        &receipt,
        &hex(&Sha256::digest(&missing_bytes))?,
    )?;
    refused(&missing_args)?;
    require(
        !missing_root.exists() && !socket.exists(),
        "missing root refusal created state or socket",
    )?;
    require_equal(
        &root_files(&root)?,
        &before_verify,
        "missing root changed selected state",
    )?;

    let manifest_hex = receipt
        .get("manifest_object_hex")
        .ok_or("missing receipt field manifest_object_hex")?
        .as_str()
        .ok_or("manifest object digest missing")?;
    require(
        manifest_hex.len() == 64
            && manifest_hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "manifest object digest is not canonical hex",
    )?;
    let shard = manifest_hex
        .get(..2)
        .ok_or("manifest object shard missing")?;
    let object_path = root
        .join("native-objects/objects")
        .join(shard)
        .join(manifest_hex);
    let original_object = fs::read(&object_path)?;
    let clean_object_root = root_files(&root)?;
    let held_object = base.join("held-manifest-object");
    fs::rename(&object_path, &held_object)?;
    let missing_object_root = root_files(&root)?;
    for args in [&verify_args, &serve_args] {
        refused(args)?;
        require_equal(
            &root_files(&root)?,
            &missing_object_root,
            "missing manifest object refusal changed state",
        )?;
        require(!socket.exists(), "missing manifest object created a socket")?;
    }
    fs::rename(&held_object, &object_path)?;
    require_equal(
        &root_files(&root)?,
        &clean_object_root,
        "missing manifest object restore changed state",
    )?;

    let mut corrupt_object = original_object.clone();
    let first_byte = corrupt_object
        .first_mut()
        .ok_or("manifest object has empty wire")?;
    *first_byte ^= 1;
    fs::write(&object_path, &corrupt_object)?;
    let corrupt_object_root = root_files(&root)?;
    for args in [&verify_args, &serve_args] {
        refused(args)?;
        require_equal(
            &root_files(&root)?,
            &corrupt_object_root,
            "corrupt manifest object refusal changed state",
        )?;
        require(!socket.exists(), "corrupt manifest object created a socket")?;
    }
    fs::write(&object_path, &original_object)?;
    require_equal(
        &root_files(&root)?,
        &clean_object_root,
        "corrupt manifest object restore changed state",
    )?;

    let linked_root = base.join("linked-state-root");
    std::os::unix::fs::symlink(&root, &linked_root)?;
    let mut linked_config = configuration;
    set_fixture_json(
        &mut linked_config,
        "/state_root",
        serde_json::json!(linked_root.to_string_lossy()),
    )?;
    let linked_config_path = base.join("linked-native-config.json");
    let linked_config_bytes = serde_json::to_vec(&linked_config)?;
    fs::write(&linked_config_path, &linked_config_bytes)?;
    let linked_digest = hex(&Sha256::digest(&linked_config_bytes))?;
    for mode in ["verify-native-state", "serve-native"] {
        refused(&identity_args(
            mode,
            &linked_config_path,
            &receipt,
            &linked_digest,
        )?)?;
        require_equal(
            &root_files(&root)?,
            &clean_object_root,
            "linked root refusal changed real state",
        )?;
        require(!socket.exists(), "linked root refusal created a socket")?;
    }
    require(
        fs::symlink_metadata(&linked_root)?.file_type().is_symlink(),
        "linked root refusal replaced the link",
    )?;
    let _reverified = successful(&verify_args)?;

    let active = selected_daemon(&serve_args, &socket)?;
    let live_owner = run_one_shot(&verify_args)?;
    require_equal(&live_owner.status.code(), &Some(70), "live owner refusal")?;
    let client = NativeUdsClient::new(socket.clone());
    let ingress = client.begin_input(operation(2), &budget())?;
    let sources = client.stage_sources(
        ingress,
        vec![SourceInput {
            path: "selected.rs".into(),
            bytes: b"selected needle".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        }],
        &budget(),
    )?;
    let [source] = sources.as_slice() else {
        return Err("selected native source staging failed".into());
    };
    let before = client.read_view(ViewSelection::Exact(configured), &budget())?;
    let owner = OwnerKey {
        producer: operation(2).producer,
        stable_id: StableOwnerId([1; 16]),
    };
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner,
            expected: None,
            next: Some(*source),
        }],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: before.source,
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
    let updated = published(&client.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    let updated_source_cut = quanta_index_contract::native_boot_source_cut_digest_v1(
        &client
            .read_view(ViewSelection::Exact(updated), &budget())?
            .source,
    )?;
    active.stop()?;

    // An exact candidate retry loses authority when the Catalog head moves.
    // Reject before reopening a writer or rewriting the original receipt.
    let before_stale_candidate = root_files(&root)?;
    let candidate_receipt_bytes = fs::read(&candidate_receipt_path)?;
    let candidate_receipt_metadata = fs::metadata(&candidate_receipt_path)?;
    // Use an absent receipt name so target admission cannot mask a missing
    // current-head check with its existing-file refusal.
    let stale_candidate_receipt_path = base.join("stale-candidate-native-boot-receipt.json");
    require(
        !stale_candidate_receipt_path.exists(),
        "stale candidate fixture requires an absent receipt",
    )?;
    let stale_candidate_args = with_flag(
        &candidate_args,
        "--receipt",
        stale_candidate_receipt_path.to_string_lossy().into_owned(),
    )?;
    refused(&stale_candidate_args)?;
    require(
        !stale_candidate_receipt_path.exists(),
        "stale candidate wrote a fresh receipt",
    )?;
    require_equal(
        &root_files(&root)?,
        &before_stale_candidate,
        "stale candidate with a fresh receipt changed root",
    )?;
    refused(&candidate_args)?;
    require_equal(
        &root_files(&root)?,
        &before_stale_candidate,
        "stale candidate retry changed root",
    )?;
    require_equal(
        &fs::read(&candidate_receipt_path)?,
        &candidate_receipt_bytes,
        "stale candidate retry changed receipt bytes",
    )?;
    let after_stale_candidate_receipt = fs::metadata(&candidate_receipt_path)?;
    require_equal(
        &(
            after_stale_candidate_receipt.dev(),
            after_stale_candidate_receipt.ino(),
        ),
        &(
            candidate_receipt_metadata.dev(),
            candidate_receipt_metadata.ino(),
        ),
        "stale candidate retry changed receipt identity",
    )?;

    let different_candidate_receipt_path = base.join("different-native-boot-receipt.json");
    let mut different_candidate_args = with_flag(
        &candidate_args,
        "--manifest-operation",
        serde_json::to_string(&operation(2))?,
    )?;
    different_candidate_args = with_flag(
        &different_candidate_args,
        "--source-cut-digest",
        hex(&updated_source_cut.0)?,
    )?;
    different_candidate_args = with_flag(
        &different_candidate_args,
        "--receipt",
        different_candidate_receipt_path
            .to_string_lossy()
            .into_owned(),
    )?;
    let before_different_candidate = root_files(&root)?;
    refused(&different_candidate_args)?;
    require_equal(
        &root_files(&root)?,
        &before_different_candidate,
        "different candidate replaced selected BOOT",
    )?;
    require(
        !different_candidate_receipt_path.exists(),
        "different candidate wrote receipt",
    )?;

    require(!socket.exists(), "selected native stop retained its socket")?;
    let before_stale_verify = root_files(&root)?;
    let receipt_metadata = fs::metadata(&receipt_path)?;
    refused(&verify_args)?;
    let after_receipt_metadata = fs::metadata(&receipt_path)?;
    require_equal(
        &root_files(&root)?,
        &before_stale_verify,
        "stale receipt verify root bytes/inodes",
    )?;
    require_equal(
        &fs::read(&receipt_path)?,
        &receipt_bytes,
        "stale receipt verify changed receipt bytes",
    )?;
    require_equal(
        &(after_receipt_metadata.dev(), after_receipt_metadata.ino()),
        &(receipt_metadata.dev(), receipt_metadata.ino()),
        "stale receipt verify changed receipt identity",
    )?;
    require(!socket.exists(), "stale receipt verify created a socket")?;

    let selection_metadata = fs::metadata(&selection_path)?;
    launcher_refused(&install_args)?;
    let after_stale_install = fs::metadata(&selection_path)?;
    require_equal(
        &fs::read(&selection_path)?,
        &selection_bytes,
        "stale receipt install changed selection bytes",
    )?;
    require_equal(
        &(after_stale_install.dev(), after_stale_install.ino()),
        &(selection_metadata.dev(), selection_metadata.ino()),
        "stale receipt install changed selection identity",
    )?;
    require_equal(
        &root_files(&root)?,
        &before_stale_verify,
        "stale receipt install changed root",
    )?;
    require(!socket.exists(), "stale receipt install created a socket")?;

    let renewed_receipt_path = base.join("renewed-native-boot-receipt.json");
    let renewed_select = with_flag(
        &select_args,
        "--receipt",
        renewed_receipt_path.to_string_lossy().into_owned(),
    )?;
    let _renewed = successful(&renewed_select)?;
    let renewed_receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&renewed_receipt_path)?)?;
    let renewed_verify = identity_args(
        "verify-native-state", &config_path, &renewed_receipt, &config_digest,
    )?;
    let renewed_candidate_verify = identity_args(
        "verify-native-candidate", &config_path, &renewed_receipt, &config_digest,
    )?;
    let before_renewed_verification = root_files(&root)?;
    let _verified_historical_boot = successful(&renewed_verify)?;
    refused(&renewed_candidate_verify)?;
    require_equal(
        &root_files(&root)?,
        &before_renewed_verification,
        "candidate verification of a historical BOOT changed root",
    )?;
    let renewed_install = with_flag(
        &install_args,
        "--receipt",
        renewed_receipt_path.to_string_lossy().into_owned(),
    )?;
    launcher_refused(&renewed_install)?;
    let updated_selection: serde_json::Value = serde_json::from_slice(&fs::read(&selection_path)?)?;
    require_equal(
        updated_selection
            .get("native_boot_receipt")
            .ok_or("missing updated selection field native_boot_receipt")?,
        &receipt,
        "same-root reinstall preserved installed BOOT receipt",
    )?;
    require_equal(
        updated_selection
            .get("native_config_sha256")
            .ok_or("missing updated selection field native_config_sha256")?,
        &serde_json::json!(config_digest),
        "same-root reinstall preserved installed config digest",
    )?;
    require(
        renewed_receipt != receipt,
        "fixture did not change install digest",
    )?;

    let reopened = launcher_daemon(&selection_path, &socket)?;
    let client = NativeUdsClient::new(socket);
    require_equal(
        &client
            .read_view(ViewSelection::Exact(updated), &budget())?
            .identity,
        &updated,
        "selected serve later publication survives restart",
    )?;
    let query = TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "needle".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 1,
        cursor: None,
    };
    let ranked = client.search_text(
        ViewSelection::Exact(updated),
        text_spec().instance,
        query,
        &budget(),
    )?;
    require_equal(&ranked.matched, &1, "selected serve text result")?;
    let hit = ranked.hits.first().ok_or("selected serve hit missing")?;
    require_equal(&hit.path.as_str(), &"selected.rs", "selected serve hit")?;
    reopened.stop()?;
    Ok(())
}

struct RebuiltNativeRoot {
    root: PathBuf,
    socket: PathBuf,
    config: PathBuf,
    receipt: PathBuf,
    #[cfg(feature = "legacy-tantivy")]
    published_operation_json: String,
    #[cfg(feature = "legacy-tantivy")]
    published_source_cut_digest_hex: String,
}

impl RebuiltNativeRoot {
    fn launcher_args(&self, mode: &str, selection: &Path) -> TestResult<Vec<String>> {
        let binary = Path::new(env!("CARGO_BIN_EXE_quanta-index-searchd")).canonicalize()?;
        Ok(vec![
            mode.into(),
            "--selection".into(),
            selection.to_string_lossy().into_owned(),
            "--binary".into(),
            binary.to_string_lossy().into_owned(),
            "--state-root".into(),
            self.root.to_string_lossy().into_owned(),
            "--config".into(),
            self.config.to_string_lossy().into_owned(),
            "--receipt".into(),
            self.receipt.to_string_lossy().into_owned(),
        ])
    }
}

fn rebuild_fixed_native_source(base: &Path, name: &str) -> TestResult<RebuiltNativeRoot> {
    // Use the production SDK ingress/build path, never copy a Catalog, object
    // store, index root or an old view into the fresh incarnation.
    let root = base.join(format!("{name}-state"));
    let socket = base.join(format!("{name}.sock"));
    let config_path = base.join(format!("{name}.json"));
    let receipt_path = base.join(format!("{name}-receipt.json"));
    fs::write(&config_path, serde_json::to_vec(&config(&root, &socket))?)?;
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))?;
    let daemon = Daemon::start(&config_path, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("fresh-root workspace refusal".into());
    };
    let input = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        input,
        IndexProfile {
            instances: vec![text_spec()],
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
    let input = client.begin_input(operation(2), &budget())?;
    let records = client.stage_sources(
        input,
        vec![SourceInput {
            path: "rebuild.rs".into(),
            bytes: b"fixed rebuild needle".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        }],
        &budget(),
    )?;
    let [source] = records.as_slice() else {
        return Err("fresh source count differs".into());
    };
    let before = client.read_view(ViewSelection::Exact(configured), &budget())?;
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
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: before.source,
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
    let producer_operation = operation(2);
    #[cfg(feature = "legacy-tantivy")]
    let published_operation_json = serde_json::to_string(&producer_operation)?;
    let rebuilt = published(&client.submit(
        UpdateRequest {
            operation: producer_operation,
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    let ranked = client.search_text(
        ViewSelection::Exact(rebuilt),
        text_spec().instance,
        TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        },
        &budget(),
    )?;
    require_equal(&ranked.matched, &1, "fresh SDK rebuild fixed match")?;
    require_equal(
        &ranked
            .hits
            .first()
            .ok_or("fresh SDK rebuild hit absent")?
            .path
            .as_str(),
        &"rebuild.rs",
        "fresh SDK rebuild fixed path",
    )?;
    #[cfg(feature = "legacy-tantivy")]
    let published_source_cut_digest_hex =
        hex(&quanta_index_contract::native_boot_source_cut_digest_v1(
            &client
                .read_view(ViewSelection::Exact(rebuilt), &budget())?
                .source,
        )?
        .0)?;
    daemon.stop()?;
    let _selected = successful(&[
        "select-native-state".into(),
        "--config".into(),
        config_path.to_string_lossy().into_owned(),
        "--manifest-operation".into(),
        serde_json::to_string(&operation(2))?,
        "--receipt".into(),
        receipt_path.to_string_lossy().into_owned(),
    ])?;
    Ok(RebuiltNativeRoot {
        root,
        socket,
        config: config_path,
        receipt: receipt_path,
        #[cfg(feature = "legacy-tantivy")]
        published_operation_json,
        #[cfg(feature = "legacy-tantivy")]
        published_source_cut_digest_hex,
    })
}

fn inspected_selection_digest(selection: &Path) -> TestResult<String> {
    let output = run_launcher(&[
        "inspect-selection".into(),
        "--selection".into(),
        selection.to_string_lossy().into_owned(),
    ])?;
    require(output.status.success(), "inspect selection refused")?;
    let wire: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(wire
        .get("selection_sha256")
        .and_then(serde_json::Value::as_str)
        .ok_or("inspection digest missing")?
        .to_owned())
}

fn require_rebuilt_query(socket: &Path) -> TestResult {
    let client = NativeUdsClient::new(socket.to_path_buf());
    let ranked = client.search_text(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        text_spec().instance,
        TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        },
        &budget(),
    )?;
    require_equal(&ranked.matched, &1, "rebuilt source fixed match")?;
    let hit = ranked
        .hits
        .first()
        .ok_or("rebuilt source fixed hit missing")?;
    require_equal(
        &hit.path.as_str(),
        &"rebuild.rs",
        "rebuilt source fixed path",
    )?;
    Ok(())
}

#[cfg(feature = "legacy-tantivy")]
const LEGACY_BOOT_PATH: &str = "src/legacy-cutover.rs";
#[cfg(feature = "legacy-tantivy")]
const LEGACY_BOOT_TOKEN: &str = "legacy_boot_needle_47";
#[cfg(feature = "legacy-tantivy")]
const LEGACY_BOOT_BODY: &str = "pub fn legacy_boot_needle_47() -> bool { true }";
// SHA-256 of the public CodeSearch file framing for repo-e2e and the fixed path.
#[cfg(feature = "legacy-tantivy")]
const LEGACY_BOOT_FILE_ID: &str =
    "file:98f1a21b71fc15f0456e9b0ac28176f1c4261e2eb0788b61d7ee4c0adbd922b1";
// SHA-256 of the fixed seed body followed by the harness's framing newline.
#[cfg(feature = "legacy-tantivy")]
const LEGACY_BOOT_SOURCE_SHA256: &str =
    "c11c4b4ecb8ae5a07425bf8531bbd61bb31f69f54275640474b215fce1adef46";

#[cfg(feature = "legacy-tantivy")]
fn legacy_boot_environment(base: &Path) -> TestResult<Vec<(String, String)>> {
    let home = base.join("legacy-home");
    let temporary = base.join("legacy-tmp");
    fs::create_dir(&home)?;
    fs::create_dir(&temporary)?;
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700))?;
    let mut environment = vec![
        ("HOME".into(), home.to_string_lossy().into_owned()),
        ("TMPDIR".into(), temporary.to_string_lossy().into_owned()),
        ("PATH".into(), "/usr/bin:/bin".into()),
    ];
    let mut source = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"));
    searchd_fixture_env::apply_searchd_env(
        &mut source,
        quanta_index_searchd_harness::DEFAULT_HISTORY_MAX_GENERATIONS,
    );
    for (name, value) in source.get_envs() {
        let name = name
            .to_str()
            .ok_or("Legacy fixture env name is not UTF-8")?;
        let value = value
            .ok_or("Legacy fixture env unexpectedly removes a variable")?
            .to_str()
            .ok_or("Legacy fixture env value is not UTF-8")?;
        environment.push((name.to_owned(), value.to_owned()));
    }
    Ok(environment)
}

#[cfg(feature = "legacy-tantivy")]
fn successful_legacy_state_command(
    args: &[String],
    environment: &[(String, String)],
) -> TestResult {
    let mut command = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"));
    let _configured = command
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .args(args);
    let output = bounded_output(command, args, false)?;
    require(
        output.status.success(),
        format!(
            "Legacy state command failed: {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

#[cfg(feature = "legacy-tantivy")]
fn require_legacy_boot_query(root: &Path) -> TestResult {
    let client = quanta_index_sdk::QuantaIndex::connect_legacy_query_only(
        quanta_index_sdk::ConnectOptions::from_state_root(root),
    )?;
    let repo = quanta_index_contract::RepoId::new("repo-e2e")?;
    let revision = quanta_index_contract::RevisionId::new("rev-e2e")?;
    let expected_pin = quanta_index_contract::GenerationPin::new(
        repo.clone(),
        revision.clone(),
        quanta_index_contract::ManifestGeneration::new(1),
    );
    let answer = client
        .lexical()
        .query()
        .code_search(LEGACY_BOOT_TOKEN)
        .active(repo, revision)
        .top_k(10)
        .execute()?;
    require_equal(
        &answer.generation,
        &expected_pin,
        "Legacy selected generation",
    )?;
    require_equal(&answer.results.len(), &1, "Legacy selected hit count")?;
    let row = answer.results.first().ok_or("Legacy selected hit absent")?;
    require_equal(
        &row.candidate_id.as_str(),
        &LEGACY_BOOT_FILE_ID,
        "Legacy selected file ID",
    )?;
    require_equal(
        &row.repo_relative_path.as_str(),
        &LEGACY_BOOT_PATH,
        "Legacy selected path",
    )?;
    require_equal(
        &row.manifest_generation,
        &expected_pin.manifest_generation,
        "Legacy hit generation",
    )?;
    let source = row
        .source
        .as_ref()
        .ok_or("Legacy source revision missing")?;
    let actual_source_sha256 = hex(&source.source_sha256)?;
    require_equal(
        &actual_source_sha256.as_str(),
        &LEGACY_BOOT_SOURCE_SHA256,
        "Legacy source bytes",
    )?;
    require(
        row.snippet.contains(LEGACY_BOOT_TOKEN),
        "Legacy selected snippet lost fixed source token",
    )
}

#[test]
#[cfg(all(feature = "legacy-tantivy", target_os = "linux"))]
fn offline_legacy_history_export_streams_real_rows_and_terminal_ack() -> TestResult {
    use quanta_index_contract::HistoryIngestBatch;
    use quanta_index_contract::index::AuxiliaryGenerationKeyV1;
    use quanta_index_contract::lex::{CommitRecord, CommitSha};
    use quanta_index_sdk::{ConnectOptions, QuantaIndex};

    let temp = tempfile::Builder::new()
        .prefix("q-history-export-")
        .tempdir_in("/tmp")?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let base = temp.path().canonicalize()?;
    let environment = legacy_boot_environment(&base)?;
    let seed = base.join("legacy-seed");
    fs::create_dir(&seed)?;
    fs::set_permissions(&seed, fs::Permissions::from_mode(0o700))?;
    let mut bootstrap = quanta_index_searchd_harness::E2eRuntime::boot_in(&seed)?;
    bootstrap.start()?;
    bootstrap.stop()?;

    // Install the manifest-backed root before ordinary Legacy writes. The
    // exporter must read the selected live root, including its internal
    // hardlinks, without asking the backup tool to rewrite its data.
    let backup = base.join("legacy-backup");
    let old_root = base.join("legacy-current");
    for (operation, source, destination) in [
        ("backup-state", seed.as_path(), backup.as_path()),
        ("restore-state", backup.as_path(), old_root.as_path()),
    ] {
        successful_legacy_state_command(
            &[
                operation.into(),
                "--source".into(),
                source.to_string_lossy().into_owned(),
                "--destination".into(),
                destination.to_string_lossy().into_owned(),
            ],
            &environment,
        )?;
    }
    let binary = Path::new(env!("CARGO_BIN_EXE_quanta-index-searchd")).canonicalize()?;
    let selection = base.join("selection.json");
    let mut install = vec![
        "install".into(),
        "--selection".into(),
        selection.to_string_lossy().into_owned(),
        "--binary".into(),
        binary.to_string_lossy().into_owned(),
        "--state-root".into(),
        old_root.to_string_lossy().into_owned(),
    ];
    for (name, value) in &environment {
        install.extend(["--env".into(), format!("{name}={value}")]);
    }
    launcher_succeeded(&install)?;
    let selected_bytes = fs::read(&selection)?;
    let selected_sha = inspected_selection_digest(&selection)?;

    let mut runtime = quanta_index_searchd_harness::E2eRuntime::boot_in(&old_root)?;
    runtime.ingest_text(
        "repo-e2e",
        "src/history_export.rs",
        "fn history_export() {}",
    )?;
    let sealed = runtime.seal()?;
    require_equal(
        &sealed,
        &quanta_index_contract::ManifestGeneration::new(1),
        "Legacy History fixture generation",
    )?;
    runtime.activate_last_sealed_generation()?;
    runtime.start()?;
    let (query, control, ingest) = runtime.socket_paths().ok_or("Legacy sockets absent")?;
    let publisher = QuantaIndex::connect_legacy(
        ConnectOptions::from_state_root(runtime.state_root())
            .with_query_socket(query)
            .with_control_socket(control)
            .with_ingest_socket(ingest),
    )?;
    let commit = CommitRecord {
        wire_version: 1,
        sha: CommitSha::from_bytes([42; 20]),
        parents: Vec::new(),
        author_time_ms: 10_000,
        committer_time_ms: 10_000,
        applied_at_ms: 10_000,
        author: "legacy-author".into(),
        author_name: Some("Legacy Author".into()),
        author_email: None,
        committer: "legacy-author".into(),
        committer_name: Some("Legacy Author".into()),
        committer_email: None,
        message: "exported original commit".into(),
        is_merge: false,
        tags: Vec::new(),
    };
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: runtime.repo(),
        revision_id: runtime.revision(),
        generation: sealed,
    };
    let manifest = runtime
        .last_sealed_search_corpus_identity()
        .ok_or("first sealed Legacy Source identity absent")?;
    let mut batch = HistoryIngestBatch {
        repo_id: generation.repo_id.clone(),
        revision_id: generation.revision_id.clone(),
        generation: generation.generation,
        manifest_digest: Some(manifest.lexical.manifest_digest),
        batch_digest: String::new(),
        commits: vec![commit.clone()],
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    };
    quanta_index_ipc::stamp_batch_digest_v1(&mut batch)?;
    let ack = publisher.history().publish_history_borrowed_v1(&batch)?;
    require(
        ack.applied && ack.durable_sequence > 0,
        "Legacy History ACK was not durable",
    )?;
    runtime.ingest_text(
        "repo-e2e",
        "src/history_export.rs",
        "fn history_export() { let _next = 2; }",
    )?;
    let next_sealed = runtime.seal()?;
    require_equal(
        &next_sealed,
        &quanta_index_contract::ManifestGeneration::new(2),
        "second Legacy History fixture generation",
    )?;
    runtime.activate_last_sealed_generation()?;
    let next_generation = AuxiliaryGenerationKeyV1 {
        generation: next_sealed,
        ..generation.clone()
    };
    let next_manifest = runtime
        .last_sealed_search_corpus_identity()
        .ok_or("second sealed Legacy Source identity absent")?;
    let next_commit = CommitRecord {
        sha: CommitSha::from_bytes([43; 20]),
        message: "exported second-generation commit".into(),
        ..commit.clone()
    };
    let mut next_batch = HistoryIngestBatch {
        generation: next_generation.generation,
        manifest_digest: Some(next_manifest.lexical.manifest_digest),
        commits: vec![next_commit.clone()],
        batch_digest: String::new(),
        ..batch
    };
    quanta_index_ipc::stamp_batch_digest_v1(&mut next_batch)?;
    let next_ack = publisher.history().publish_history_borrowed_v1(&next_batch)?;
    require(
        next_ack.applied && next_ack.durable_sequence > ack.durable_sequence,
        "second Legacy History ACK must follow the first",
    )?;
    drop(publisher);
    runtime.stop()?;

    require_equal(&fs::read(&selection)?, &selected_bytes, "Legacy publication selection")?;
    let old_files = root_files(&old_root)?;

    let native_root = base.join("candidate-state");
    fs::create_dir(&native_root)?;
    fs::set_permissions(&native_root, fs::Permissions::from_mode(0o700))?;
    let native_config = base.join("candidate.json");
    fs::write(
        &native_config,
        serde_json::to_vec(&config(&native_root, &base.join("candidate.sock")))?,
    )?;
    fs::set_permissions(&native_config, fs::Permissions::from_mode(0o600))?;
    let config_sha = hex(&Sha256::digest(fs::read(&native_config)?))?;
    let inventory = run_private_pipe_one_shot(&[
        "export-native-history-checkpoint",
        "--selection-path",
        selection.to_str().ok_or("selection is not UTF-8")?,
        "--expected-current-sha256",
        &selected_sha,
        "--inventory-repo-id",
        generation.repo_id.as_str(),
        "--config",
        native_config.to_str().ok_or("config is not UTF-8")?,
        "--config-digest",
        &config_sha,
    ])?;
    require(
        inventory.status.success(),
        format!(
            "real History inventory refused: {}",
            String::from_utf8_lossy(&inventory.stderr)
        ),
    )?;
    assert_legacy_history_inventory_v1(
        &inventory.stdout,
        &generation.repo_id,
        &[generation.clone(), next_generation.clone()],
    )?;
    for (generation, commit, ack) in [
        (&generation, &commit, &ack),
        (&next_generation, &next_commit, &next_ack),
    ] {
        let generation_json = serde_json::to_string(generation)?;
        let output = run_private_pipe_one_shot(&[
            "export-native-history-checkpoint",
            "--selection-path",
            selection.to_str().ok_or("selection is not UTF-8")?,
            "--expected-current-sha256",
            &selected_sha,
            "--generation-json",
            &generation_json,
            "--config",
            native_config.to_str().ok_or("config is not UTF-8")?,
            "--config-digest",
            &config_sha,
        ])?;
        require(
            output.status.success(),
            format!(
                "real History export refused for generation {:?}: {}",
                generation.generation,
                String::from_utf8_lossy(&output.stderr)
            ),
        )?;
        assert_legacy_history_export_v1(&output.stdout, generation, commit, ack, &old_root)?;
        require_equal(
            &fs::read(&selection)?,
            &selected_bytes,
            "History export selection",
        )?;
        require_equal(
            &root_files(&old_root)?,
            &old_files,
            "History export Legacy root",
        )?;
    }
    Ok(())
}

#[cfg(all(feature = "legacy-tantivy", target_os = "linux"))]
fn assert_legacy_history_inventory_v1(
    mut bytes: &[u8],
    repo: &quanta_index_contract::RepoId,
    expected: &[quanta_index_contract::index::AuxiliaryGenerationKeyV1],
) -> TestResult {
    use quanta_index_contract::index::{
        HistorySnapshotExportFrameV1, MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1,
    };

    let mut count = 0_usize;
    let mut completed = false;
    while !bytes.is_empty() {
        require(
            !completed && bytes.len() >= 4,
            "History inventory has trailing or truncated frame",
        )?;
        let length = usize::try_from(u32::from_le_bytes(bytes[..4].try_into()?))?;
        require(
            length <= MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1 + 4096 && bytes.len() >= length + 4,
            "History inventory frame exceeds bound or is truncated",
        )?;
        let frame: HistorySnapshotExportFrameV1 = serde_json::from_slice(&bytes[4..4 + length])?;
        bytes = &bytes[4 + length..];
        match frame {
            HistorySnapshotExportFrameV1::InventoryKey { generation } => {
                require_equal(
                    &Some(&generation),
                    &expected.get(count),
                    "History inventory key",
                )?;
                count += 1;
            }
            HistorySnapshotExportFrameV1::InventoryComplete {
                repo_id,
                count: observed,
            } => {
                require_equal(&repo_id, repo, "History inventory repository")?;
                require_equal(
                    &observed,
                    &u64::try_from(expected.len())?,
                    "History inventory count",
                )?;
                require_equal(&count, &expected.len(), "History inventory key count")?;
                completed = true;
            }
            HistorySnapshotExportFrameV1::Page(_)
            | HistorySnapshotExportFrameV1::Complete { .. } => {
                return Err("History inventory emitted a checkpoint frame".into());
            }
        }
    }
    require(completed, "History inventory completion absent")?;
    Ok(())
}

#[cfg(all(feature = "legacy-tantivy", target_os = "linux"))]
fn assert_legacy_history_export_v1(
    mut bytes: &[u8],
    generation: &quanta_index_contract::index::AuxiliaryGenerationKeyV1,
    commit: &quanta_index_contract::lex::CommitRecord,
    ack: &quanta_index_contract::BatchPublishReceipt,
    old_root: &Path,
) -> TestResult {
    use quanta_index_contract::AuxEpochV1;
    use quanta_index_contract::index::{
        HistorySnapshotExportFrameV1, MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1,
    };
    use quanta_index_core::{AuxiliaryRowFamilyV1, domains::auxiliary::HistorySnapshotDigestV1};

    let mut next_ordinal = 1_u32;
    let mut rows = 0_u64;
    let mut digest = HistorySnapshotDigestV1::new(generation);
    let expected_commit = quanta_index_ipc::encode_cbor_payload(commit)?;
    let mut saw_commit = false;
    let mut completion = None;
    while !bytes.is_empty() {
        require(
            completion.is_none() && bytes.len() >= 4,
            "History export has trailing or truncated frame",
        )?;
        let length = u32::from_le_bytes(bytes[..4].try_into()?);
        let length = usize::try_from(length)?;
        require(
            length <= MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1 + 4096 && bytes.len() >= length + 4,
            "History export frame exceeds bound or is truncated",
        )?;
        let frame: HistorySnapshotExportFrameV1 = serde_json::from_slice(&bytes[4..4 + length])?;
        bytes = &bytes[4 + length..];
        match frame {
            HistorySnapshotExportFrameV1::Page(page) => {
                page.validate_shape()?;
                require_equal(
                    &page.generation,
                    generation,
                    "History export page generation",
                )?;
                require_equal(&page.ordinal, &next_ordinal, "History export page ordinal")?;
                next_ordinal += 1;
                for row in page.rows {
                    let family = AuxiliaryRowFamilyV1::from_code_str(&row.family)
                        .ok_or("History export family is unknown")?;
                    if family == AuxiliaryRowFamilyV1::Commit {
                        require_equal(
                            &row.row_key,
                            &commit.sha.as_bytes().to_vec(),
                            "History commit key",
                        )?;
                        require_equal(&row.value, &expected_commit, "History commit bytes")?;
                        saw_commit = true;
                    }
                    digest.push_row(family, &row.row_key, &row.value);
                    rows += 1;
                }
            }
            frame @ HistorySnapshotExportFrameV1::Complete { .. } => completion = Some(frame),
            HistorySnapshotExportFrameV1::InventoryKey { .. }
            | HistorySnapshotExportFrameV1::InventoryComplete { .. } => {
                return Err("History checkpoint emitted an inventory frame".into());
            }
        }
    }
    let Some(HistorySnapshotExportFrameV1::Complete {
        generation: observed_generation,
        legacy_root,
        durable_epoch,
        snapshot_digest,
        row_count,
        terminal_ack,
    }) = completion
    else {
        return Err("History export completion absent".into());
    };
    require(
        saw_commit && next_ordinal > 1,
        "History export omitted page or commit",
    )?;
    require_equal(
        &observed_generation,
        generation,
        "History completion generation",
    )?;
    require_equal(
        &legacy_root,
        &old_root.to_string_lossy().into_owned(),
        "History completion root",
    )?;
    require_equal(
        &durable_epoch,
        &AuxEpochV1::new(1),
        "History completion epoch",
    )?;
    require_equal(&row_count, &rows, "History completion row count")?;
    require_equal(
        &snapshot_digest,
        &digest.finish(),
        "History reconstructed digest",
    )?;
    require_equal(&terminal_ack, ack, "History original terminal ACK")?;
    Ok(())
}

#[test]
#[cfg(feature = "legacy-tantivy")]
fn legacy_to_fresh_native_preserves_old_root_and_refuses_unverified_rollback() -> TestResult {
    // Q-only fixture: the actual external producer's S publication tuple is a
    // separate handoff qualification. This test keeps the unsupported reverse
    // transition closed after proving the selected Native root serves.
    let temp = tempfile::Builder::new().prefix("qbl-").tempdir_in("/tmp")?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let base = temp.path().canonicalize()?;
    let environment = legacy_boot_environment(&base)?;
    let seed = base.join("legacy-seed");
    fs::create_dir(&seed)?;
    fs::set_permissions(&seed, fs::Permissions::from_mode(0o700))?;
    let mut runtime = quanta_index_searchd_harness::E2eRuntime::boot_in(&seed)?;
    runtime.ingest_text("repo-e2e", LEGACY_BOOT_PATH, LEGACY_BOOT_BODY)?;
    runtime.ingest_text("repo-e2e", "src/decoy.rs", "pub fn unrelated() {}")?;
    let generation = runtime.seal()?;
    require_equal(
        &generation,
        &quanta_index_contract::ManifestGeneration::new(1),
        "Legacy sealed fixture generation",
    )?;
    runtime.activate_last_sealed_generation()?;
    runtime.stop()?;

    let backup = base.join("legacy-backup");
    let old_root = base.join("legacy-current");
    for (operation, source, destination) in [
        ("backup-state", seed.as_path(), backup.as_path()),
        ("restore-state", backup.as_path(), old_root.as_path()),
    ] {
        successful_legacy_state_command(
            &[
                operation.into(),
                "--source".into(),
                source.to_string_lossy().into_owned(),
                "--destination".into(),
                destination.to_string_lossy().into_owned(),
            ],
            &environment,
        )?;
    }
    require(
        old_root.join("state-root-manifest-v1.txt").is_file(),
        "restored Legacy root has no manifest",
    )?;
    successful_legacy_state_command(
        &[
            "verify-state".into(),
            "--source".into(),
            old_root.to_string_lossy().into_owned(),
        ],
        &environment,
    )?;

    let candidate = rebuild_fixed_native_source(&base, "candidate")?;
    let selection = base.join("selection.json");
    let binary = Path::new(env!("CARGO_BIN_EXE_quanta-index-searchd")).canonicalize()?;
    let mut install = vec![
        "install".into(),
        "--selection".into(),
        selection.to_string_lossy().into_owned(),
        "--binary".into(),
        binary.to_string_lossy().into_owned(),
        "--state-root".into(),
        old_root.to_string_lossy().into_owned(),
    ];
    for (name, value) in &environment {
        install.extend(["--env".into(), format!("{name}={value}")]);
    }
    launcher_succeeded(&install)?;
    launcher_succeeded(&[
        "verify-bootstrap".into(),
        "--selection".into(),
        selection.to_string_lossy().into_owned(),
    ])?;
    let old_wire = fs::read(&selection)?;
    let old_digest = inspected_selection_digest(&selection)?;
    let before_export = root_files(&old_root)?;
    // The real child must refuse a stale active-selection binding before it
    // emits any provisional History page. Use a pipe, as the exporter requires.
    let config_digest = hex(&Sha256::digest(fs::read(&candidate.config)?))?;
    let history_generation =
        serde_json::to_string(&quanta_index_contract::index::AuxiliaryGenerationKeyV1 {
            repo_id: quanta_index_contract::RepoId::new("repo-e2e")?,
            revision_id: quanta_index_contract::RevisionId::new("rev-e2e")?,
            generation: quanta_index_contract::ManifestGeneration::new(1),
        })?;
    let wrong_digest = "00".repeat(32);
    require(
        wrong_digest != old_digest,
        "fixture selected SHA unexpectedly zero",
    )?;
    let export_refusal = run_private_pipe_one_shot(&[
        "export-native-history-checkpoint",
        "--selection-path",
        selection.to_str().ok_or("selection is not UTF-8")?,
        "--expected-current-sha256",
        &wrong_digest,
        "--generation-json",
        &history_generation,
        "--config",
        candidate.config.to_str().ok_or("config is not UTF-8")?,
        "--config-digest",
        &config_digest,
    ])?;
    require_equal(
        &export_refusal.status.code(),
        &Some(70),
        "stale History export exit",
    )?;
    require(
        export_refusal.stdout.is_empty(),
        "stale History export emitted a page",
    )?;
    require(
        String::from_utf8_lossy(&export_refusal.stderr)
            .contains("active selection fingerprint changed"),
        "History export refusal did not reach active selection CAS",
    )?;
    require_equal(
        &fs::read(&selection)?,
        &old_wire,
        "stale History export selection",
    )?;
    require_equal(
        &root_files(&old_root)?,
        &before_export,
        "stale History export root",
    )?;
    let legacy_socket = old_root.join("search-plane/query.sock");
    let daemon = launcher_daemon(&selection, &legacy_socket)?;
    require_legacy_boot_query(&old_root)?;
    daemon.stop()?;
    require(
        UnixStream::connect(&legacy_socket).is_err(),
        "old Legacy serve did not drain",
    )?;
    require_equal(&fs::read(&selection)?, &old_wire, "Legacy serve selection")?;
    let old_quiescent_files = root_files(&old_root)?;

    let previous = base.join("legacy-previous.json");
    let preserved = run_launcher(&[
        "preserve-legacy".into(),
        "--selection".into(),
        selection.to_string_lossy().into_owned(),
        "--previous-selection".into(),
        previous.to_string_lossy().into_owned(),
        "--expected-selection-sha256".into(),
        old_digest.clone(),
    ])?;
    require(
        preserved.status.success(),
        format!(
            "preserve-legacy refused: {}",
            String::from_utf8_lossy(&preserved.stderr)
        ),
    )?;
    let preservation: serde_json::Value = serde_json::from_slice(&preserved.stdout)?;
    require_equal(
        &preservation
            .get("previous_selection_sha256")
            .and_then(serde_json::Value::as_str),
        &Some(old_digest.as_str()),
        "preserved Legacy CAS",
    )?;
    require_equal(&fs::read(&previous)?, &old_wire, "saved Legacy selection")?;

    let mut switch = candidate.launcher_args("switch-native", &selection)?;
    switch.extend([
        "--expected-selection-sha256".into(),
        old_digest.clone(),
        "--previous-selection".into(),
        previous.to_string_lossy().into_owned(),
        "--expected-previous-selection-sha256".into(),
        old_digest.clone(),
        "--target-operation".into(),
        serde_json::to_string(&operation(2))?,
        "--published-operation".into(),
        candidate.published_operation_json.clone(),
        "--published-source-cut-digest".into(),
        candidate.published_source_cut_digest_hex.clone(),
    ]);
    launcher_succeeded(&switch)?;
    require(
        UnixStream::connect(&legacy_socket).is_err(),
        "inactive Legacy root still serves",
    )?;
    for _ in 0..2 {
        let daemon = launcher_daemon(&selection, &candidate.socket)?;
        require_rebuilt_query(&candidate.socket)?;
        daemon.stop()?;
        require(
            UnixStream::connect(&candidate.socket).is_err(),
            "Native serve did not drain",
        )?;
    }
    require_equal(
        &root_files(&old_root)?,
        &old_quiescent_files,
        "Native serve changed inactive Legacy root",
    )?;
    let native_quiescent_files = root_files(&candidate.root)?;
    let selected_native = fs::read(&selection)?;
    let rollback = run_launcher(&[
        "rollback-legacy".into(),
        "--selection".into(),
        selection.to_string_lossy().into_owned(),
        "--expected-selection-sha256".into(),
        inspected_selection_digest(&selection)?,
        "--previous-selection".into(),
        previous.to_string_lossy().into_owned(),
        "--expected-previous-selection-sha256".into(),
        old_digest,
    ])?;
    require_equal(
        &rollback.status.code(),
        &Some(1),
        "unverified rollback exit",
    )?;
    require(
        rollback.stdout.is_empty(),
        "unverified rollback emitted a receipt",
    )?;
    require(
        String::from_utf8_lossy(&rollback.stderr)
            .contains("Legacy rollback requires offline Catalog publication baseline and fenced producer outbox verification"),
        "rollback refused for a reason other than missing offline authorities",
    )?;
    require_equal(
        &fs::read(&selection)?,
        &selected_native,
        "refused rollback changed Native selection",
    )?;
    require_equal(
        &fs::read(&previous)?,
        &old_wire,
        "refused rollback changed saved Legacy selection",
    )?;
    require(
        UnixStream::connect(&legacy_socket).is_err(),
        "refused rollback reopened inactive Legacy serve",
    )?;
    require_equal(
        &root_files(&candidate.root)?,
        &native_quiescent_files,
        "Legacy rollback serve changed inactive Native root",
    )?;
    Ok(())
}

#[test]
fn native_cross_root_sdk_rebuild_refuses_unverified_switch() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let base = temp.path().canonicalize()?;
    let old = rebuild_fixed_native_source(&base, "old")?;
    let new = rebuild_fixed_native_source(&base, "new")?;
    let selection = base.join("selection.json");
    launcher_succeeded(&old.launcher_args("install-native", &selection)?)?;
    let old_wire = fs::read(&selection)?;
    let old_files = root_files(&old.root)?;
    let new_files = root_files(&new.root)?;
    let old_digest = inspected_selection_digest(&selection)?;
    let mut switch = new.launcher_args("switch-native", &selection)?;
    switch.extend(["--expected-selection-sha256".into(), old_digest]);
    // A plain install may not bypass the cross-root CAS contract.
    launcher_refused(&new.launcher_args("install-native", &selection)?)?;
    require_equal(
        &fs::read(&selection)?,
        &old_wire,
        "refused install selection",
    )?;
    let refused = run_launcher(&switch)?;
    require_equal(
        &refused.status.code(),
        &Some(1),
        "unverified Native switch exit",
    )?;
    require(
        refused.stdout.is_empty(),
        "unverified Native switch emitted a receipt",
    )?;
    require(
        String::from_utf8_lossy(&refused.stderr)
            .contains("Native cross-root switching requires offline Catalog publication baseline and fenced producer outbox verification"),
        "Native switch refused for a reason other than missing offline authorities",
    )?;
    require_equal(
        &fs::read(&selection)?,
        &old_wire,
        "refused Native switch selection",
    )?;
    require_equal(
        &root_files(&old.root)?,
        &old_files,
        "refused Native switch old root bytes/inodes",
    )?;
    require_equal(
        &root_files(&new.root)?,
        &new_files,
        "refused Native switch candidate root bytes/inodes",
    )?;
    let daemon = launcher_daemon(&selection, &old.socket)?;
    require_rebuilt_query(&old.socket)?;
    daemon.stop()?;
    require_equal(
        &fs::read(&selection)?,
        &old_wire,
        "selected Native serve selection",
    )?;
    Ok(())
}
