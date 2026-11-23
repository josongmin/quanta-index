#[path = "native_boot/offline_semantic.rs"]
mod offline_semantic;

use super::*;
use std::{collections::BTreeMap, os::unix::fs::MetadataExt as _, path::PathBuf, process::Output};

use sha2::{Digest as _, Sha256};

use crate::legacy_searchd_binary::LegacySearchdBinary;
use quanta_index_contract::index::{
    NATIVE_INDEX_STATE_MARKER_CONTENT, NATIVE_INDEX_STATE_MARKER_NAME, ViewRef,
};
use quanta_index_searchd_harness::legacy_reference::publish_legacy_source;

#[path = "../common/searchd_fixture_env.rs"]
mod searchd_fixture_env;

#[test]
fn a_world_writable_pre_created_state_root_is_refused_typed_before_any_socket() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    fs::create_dir(&state_root)?;
    fs::set_permissions(&state_root, fs::Permissions::from_mode(0o777))?;
    let socket = parent.path().join("native.sock");
    let config_path = parent.path().join("native-config.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&config(&state_root, &socket))?,
    )?;
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))?;

    let args = vec![
        "serve-native".to_owned(),
        "--config".to_owned(),
        config_path.to_string_lossy().into_owned(),
    ];
    let mut command = Command::new("sh");
    let _configured = command
        .arg("-c")
        .arg(r#"umask "$1" && exec "$2" serve-native --config "$3""#)
        .arg("native-umask-hardening")
        .arg("000")
        .arg(env!("CARGO_BIN_EXE_quanta-index-searchd"))
        .arg(&config_path);
    let output = bounded_output(
        |stdout, stderr| Ok(command.stdout(stdout).stderr(stderr).spawn()?),
        &args,
        false,
    )?;
    require(
        !output.status.success(),
        "a world-writable state root must refuse boot",
    )?;
    require(
        output.stdout.is_empty(),
        "a refused Native boot emitted stdout",
    )?;
    let stderr = String::from_utf8(output.stderr)?;
    require(
        stderr.contains("STATE_ROOT_INSECURE") && stderr.contains("mode 0777"),
        format!("the refusal must name the typed code and the mode: {stderr}"),
    )?;
    require(!socket.exists(), "a refused Native boot created a socket")?;
    require_equal(
        &(fs::metadata(&state_root)?.permissions().mode() & 0o7777),
        &0o777,
        "refused state-root mode",
    )?;
    require_equal(
        &fs::read_dir(&state_root)?.count(),
        &0,
        "a refused boot leaves the state root untouched",
    )
}

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

fn bounded_output(
    spawn: impl FnOnce(Stdio, Stdio) -> TestResult<Child>,
    args: &[String],
    launcher: bool,
) -> TestResult<Output> {
    // A launcher child inherits stdout/stderr. Anonymous files keep a parent
    // exit from blocking forever while a grandchild still owns either FD.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = spawn(stdout.try_clone()?.into(), stderr.try_clone()?.into())?;
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
    run_one_shot_from(Path::new(env!("CARGO_BIN_EXE_quanta-index-searchd")), args)
}

fn run_one_shot_from(binary: &Path, args: &[String]) -> TestResult<Output> {
    let mut command = Command::new(binary);
    let _args = command.args(args);
    bounded_output(
        |stdout, stderr| Ok(command.stdout(stdout).stderr(stderr).spawn()?),
        args,
        false,
    )
}

fn run_private_pipe_one_shot(args: &[&str]) -> TestResult<Output> {
    fn drain(mut pipe: impl std::io::Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
        let mut bytes = Vec::new();
        let mut overflow = false;
        let mut chunk = [0_u8; 64 * 1024];
        loop {
            let count = pipe.read(&mut chunk)?;
            if count == 0 {
                return Ok((bytes, overflow));
            }
            let keep = count.min(limit.saturating_sub(bytes.len()));
            bytes.extend_from_slice(&chunk[..keep]);
            overflow |= keep != count;
        }
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or("History exporter stdout pipe absent")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("History exporter stderr pipe absent")?;
    let stdout_reader = thread::spawn(move || drain(stdout, 128 * 1024 * 1024));
    let stderr_reader = thread::spawn(move || drain(stderr, 1024 * 1024));
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(120))
        .ok_or("History exporter process deadline overflow")?;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _killed = child.kill();
            let _waited = child.wait();
            let _stdout = stdout_reader.join();
            let _stderr = stderr_reader.join();
            return Err("History exporter process exceeded startup deadline".into());
        }
        thread::sleep(Duration::from_millis(10));
    };
    let (stdout, stdout_overflow) = stdout_reader
        .join()
        .map_err(|_| "History exporter stdout reader panicked")??;
    let (stderr, stderr_overflow) = stderr_reader
        .join()
        .map_err(|_| "History exporter stderr reader panicked")??;
    require(
        !stdout_overflow && !stderr_overflow,
        "History exporter output exceeded bound",
    )?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn launcher_script() -> TestResult<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/deployment/launcher.py")
        .canonicalize()?)
}

fn run_launcher(args: &[String]) -> TestResult<Output> {
    let mut command = Command::new("python3");
    let _args = command.arg(launcher_script()?).args(args);
    bounded_output(
        |stdout, stderr| Ok(command.stdout(stdout).stderr(stderr).spawn()?),
        args,
        true,
    )
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
    successful_from(Path::new(env!("CARGO_BIN_EXE_quanta-index-searchd")), args)
}

fn successful_from(binary: &Path, args: &[String]) -> TestResult<Output> {
    let output = run_one_shot_from(binary, args)?;
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
    require_boot_refusal(run_one_shot(args)?)
}

fn require_boot_refusal(output: Output) -> TestResult {
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

pub(super) fn require_native_only_legacy_offline_refusal(binary: &Path) -> TestResult {
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let source = temp.path().join("legacy-source");
    fs::create_dir(&source)?;
    fs::write(
        source.join("state-root-manifest-v1.txt"),
        b"legacy state sentinel",
    )?;
    let original_root = fs::symlink_metadata(&source)?;
    let original = root_files(&source)?;
    let destination = temp.path().join("destination");
    for operation in ["backup-state", "restore-state", "verify-state"] {
        let mut args = vec![
            operation.into(),
            "--source".into(),
            source.to_string_lossy().into_owned(),
        ];
        if operation != "verify-state" {
            args.extend([
                "--destination".into(),
                destination.to_string_lossy().into_owned(),
            ]);
        }
        let output = run_one_shot_from(binary, &args)?;
        let stderr = std::str::from_utf8(&output.stderr)?;
        require(
            stderr.contains("legacy state command unavailable in Native-only build"),
            format!("{operation} reached the wrong refusal boundary: {stderr}"),
        )?;
        require_boot_refusal(output)?;
        match fs::symlink_metadata(&destination) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => return Err(format!("{operation} created a destination").into()),
        }
        let root = fs::symlink_metadata(&source)?;
        require(
            root.is_dir() && root.dev() == original_root.dev() && root.ino() == original_root.ino(),
            format!("{operation} replaced the source directory"),
        )?;
        require_equal(&root_files(&source)?, &original, operation)?;
        let entries = fs::read_dir(&source)?.collect::<std::io::Result<Vec<_>>>()?;
        require_equal(&entries.len(), &1, operation)?;
    }
    Ok(())
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
    selected_daemon_from(
        Path::new(env!("CARGO_BIN_EXE_quanta-index-searchd")),
        args,
        socket,
    )
}

fn selected_daemon_from(binary: &Path, args: &[String], socket: &Path) -> TestResult<Daemon> {
    let child = Command::new(binary)
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
            language: None,
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
        "verify-native-state",
        &config_path,
        &renewed_receipt,
        &config_digest,
    )?;
    let renewed_candidate_verify = identity_args(
        "verify-native-candidate",
        &config_path,
        &renewed_receipt,
        &config_digest,
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

const REBUILD_SOURCE_PATH: &str = "rebuild.rs";
const CANDIDATE_REBUILD_SOURCE_PATH: &str = "candidate-rebuild.rs";

struct RebuiltNativeRoot {
    binary: PathBuf,
    root: PathBuf,
    socket: PathBuf,
    config: PathBuf,
    receipt: PathBuf,
    published_view: ViewRef,
    published_source: ObjectRef,
    published_operation_json: String,
    published_source_cut_digest_hex: String,
    startup_observation: Option<read_view_daemon_witness::Observation>,
}

impl RebuiltNativeRoot {
    fn launcher_args(&self, mode: &str, selection: &Path) -> TestResult<Vec<String>> {
        let binary = self
            .binary
            .to_str()
            .ok_or("rebuild daemon path is not UTF-8")?;
        Ok(vec![
            mode.into(),
            "--selection".into(),
            selection.to_string_lossy().into_owned(),
            "--binary".into(),
            binary.into(),
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
    rebuild_fixed_native_source_from(
        base,
        name,
        Path::new(env!("CARGO_BIN_EXE_quanta-index-searchd")),
        REBUILD_SOURCE_PATH,
        false,
    )
}

fn rebuild_fixed_native_source_from(
    base: &Path,
    name: &str,
    binary: &Path,
    source_path: &str,
    observe_startup: bool,
) -> TestResult<RebuiltNativeRoot> {
    // Use the production SDK ingress/build path, never copy a Catalog, object
    // store, index root or an old view into the fresh incarnation.
    let binary = binary.canonicalize()?;
    let root = base.join(format!("{name}-state"));
    let socket = base.join(format!("{name}.sock"));
    let config_path = base.join(format!("{name}.json"));
    let receipt_path = base.join(format!("{name}-receipt.json"));
    fs::write(&config_path, serde_json::to_vec(&config(&root, &socket))?)?;
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))?;
    let daemon = Daemon::start_binary(&binary, &config_path, &socket, None, None)?;
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
            path: source_path.into(),
            bytes: b"fixed rebuild needle".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
            language: None,
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
    let published_operation_json = serde_json::to_string(&producer_operation)?;
    let rebuilt = published(&client.submit(
        UpdateRequest {
            operation: producer_operation,
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    require_equal(
        &rebuilt_query_source(&client, ViewSelection::Exact(rebuilt), source_path)?,
        source,
        "fresh SDK rebuild hit Source",
    )?;
    let published_source_cut_digest_hex =
        hex(&quanta_index_contract::native_boot_source_cut_digest_v1(
            &client
                .read_view(ViewSelection::Exact(rebuilt), &budget())?
                .source,
        )?
        .0)?;
    let startup_observation = if observe_startup {
        read_view_daemon_witness::observe(daemon.0.id())?
    } else {
        None
    };
    daemon.stop()?;
    let _selected = successful_from(
        &binary,
        &[
            "select-native-state".into(),
            "--config".into(),
            config_path.to_string_lossy().into_owned(),
            "--manifest-operation".into(),
            serde_json::to_string(&operation(2))?,
            "--receipt".into(),
            receipt_path.to_string_lossy().into_owned(),
        ],
    )?;
    Ok(RebuiltNativeRoot {
        binary,
        root,
        socket,
        config: config_path,
        receipt: receipt_path,
        published_view: rebuilt,
        published_source: *source,
        published_operation_json,
        published_source_cut_digest_hex,
        startup_observation,
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
    let _source = rebuilt_query_source(
        &client,
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        REBUILD_SOURCE_PATH,
    )?;
    Ok(())
}

fn rebuilt_query_source(
    client: &NativeUdsClient,
    selection: ViewSelection,
    expected_path: &str,
) -> TestResult<ObjectRef> {
    let ranked = client.search_text(
        selection,
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
        &expected_path,
        "rebuilt source fixed path",
    )?;
    Ok(hit.source)
}

const LEGACY_BOOT_PATH: &str = "src/legacy-cutover.rs";
const LEGACY_FIXTURE_HISTORY_KEEP: usize = 8;
const LEGACY_BOOT_TOKEN: &str = "legacy_boot_needle_47";
const LEGACY_BOOT_BODY: &str = "pub fn legacy_boot_needle_47() -> bool { true }";
// SHA-256 of the public CodeSearch file framing for repo-e2e and the fixed path.
const LEGACY_BOOT_FILE_ID: &str =
    "file:98f1a21b71fc15f0456e9b0ac28176f1c4261e2eb0788b61d7ee4c0adbd922b1";
// SHA-256 of the fixed seed body followed by the harness's framing newline.
const LEGACY_BOOT_SOURCE_SHA256: &str =
    "c11c4b4ecb8ae5a07425bf8531bbd61bb31f69f54275640474b215fce1adef46";

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
    searchd_fixture_env::apply_searchd_env(&mut source, LEGACY_FIXTURE_HISTORY_KEEP);
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

fn successful_legacy_state_command(
    binary: &LegacySearchdBinary,
    args: &[String],
    environment: &[(String, String)],
) -> TestResult {
    let mut command = binary.command()?;
    let _configured = command
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .args(args);
    let output = bounded_output(
        |stdout, stderr| Ok(command.stdout(stdout).stderr(stderr).spawn()?),
        args,
        false,
    )?;
    binary.verify()?;
    require(
        output.status.success(),
        format!(
            "Legacy state command failed: {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

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
#[cfg(target_os = "linux")]
fn offline_legacy_history_export_streams_real_rows_and_terminal_ack() -> TestResult {
    use quanta_index_contract::index::AuxiliaryGenerationKeyV1;
    use quanta_index_contract::lex::DiffHunkRecord;
    use quanta_index_contract::lex::{CommitRecord, CommitSha};
    use quanta_index_contract::results::DiffHunkSide;
    use quanta_index_contract::{HistoryDiffHunkUpsert, HistoryIngestBatch};

    let temp = tempfile::Builder::new()
        .prefix("q-history-export-")
        .tempdir_in("/tmp")?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let base = temp.path().canonicalize()?;
    let legacy = LegacySearchdBinary::from_environment()?;
    let environment = legacy_boot_environment(&base)?;
    let seed = base.join("legacy-seed");
    fs::create_dir(&seed)?;
    fs::set_permissions(&seed, fs::Permissions::from_mode(0o700))?;
    let bootstrap = legacy.start(&seed, LEGACY_FIXTURE_HISTORY_KEEP)?;
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
            &legacy,
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
    let installed_old_binary = legacy.installer_binary()?;
    let binary = installed_old_binary.path();
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
    installed_old_binary.verify()?;
    legacy.verify()?;
    let selected_bytes = fs::read(&selection)?;
    let selected_sha = inspected_selection_digest(&selection)?;

    let old_process = legacy.start(&old_root, LEGACY_FIXTURE_HISTORY_KEEP)?;
    let publisher = old_process.connect()?;
    let (first_head, _) = publish_legacy_source(
        &publisher,
        &quanta_index_contract::RepoId::new("repo-e2e")?,
        &quanta_index_contract::RevisionId::new("rev-e2e")?,
        1,
        None,
        &[("src/history_export.rs", "fn history_export() {}", 1)],
        None,
    )?;
    let sealed = first_head.generation.lexical.manifest_generation;
    require_equal(
        &sealed,
        &quanta_index_contract::ManifestGeneration::new(1),
        "Legacy History fixture generation",
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
        repo_id: quanta_index_contract::RepoId::new("repo-e2e")?,
        revision_id: quanta_index_contract::RevisionId::new("rev-e2e")?,
        generation: sealed,
    };
    let manifest = first_head.generation.clone();
    let hunk = |sha, width: usize| -> TestResult<HistoryDiffHunkUpsert> {
        Ok(HistoryDiffHunkUpsert {
            commit_sha: sha,
            file_path: "src/history_export.rs".into(),
            record: DiffHunkRecord {
                wire_version: 1,
                hunk_header: "@@".into(),
                side: DiffHunkSide::After,
                added_text: "x".repeat(width).into_boxed_str(),
                removed_text: "".into(),
                touched_text: "".into(),
                byte_start: 0,
                byte_end: u32::try_from(width)?,
            },
        })
    };
    let first_hunk = hunk(commit.sha, 3 * 1024 * 1024)?;
    let mut batch = HistoryIngestBatch {
        repo_id: generation.repo_id.clone(),
        revision_id: generation.revision_id.clone(),
        generation: generation.generation,
        manifest_digest: Some(manifest.lexical.manifest_digest),
        batch_digest: String::new(),
        commits: vec![commit.clone()],
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: vec![first_hunk.clone()],
    };
    quanta_index_ipc::stamp_batch_digest_v1(&mut batch)?;
    let ack = publisher.history().publish_history_borrowed_v1(&batch)?;
    require(
        ack.applied && ack.durable_sequence > 0,
        "Legacy History ACK was not durable",
    )?;
    let (second_head, _) = publish_legacy_source(
        &publisher,
        &quanta_index_contract::RepoId::new("repo-e2e")?,
        &quanta_index_contract::RevisionId::new("rev-e2e")?,
        2,
        Some(("src/history_export.rs", "fn history_export() {}", 1)),
        &[(
            "src/history_export.rs",
            "fn history_export() { let _next = 2; }",
            2,
        )],
        Some(first_head),
    )?;
    let next_sealed = second_head.generation.lexical.manifest_generation;
    require_equal(
        &next_sealed,
        &quanta_index_contract::ManifestGeneration::new(2),
        "second Legacy History fixture generation",
    )?;
    let next_generation = AuxiliaryGenerationKeyV1 {
        generation: next_sealed,
        ..generation.clone()
    };
    let next_manifest = second_head.generation;
    let next_commit = CommitRecord {
        sha: CommitSha::from_bytes([43; 20]),
        message: "exported second-generation commit".into(),
        ..commit.clone()
    };
    let shared_commit = CommitRecord {
        sha: CommitSha::from_bytes([44; 20]),
        message: "exported shared-part commit".into(),
        ..commit.clone()
    };
    let next_hunk = hunk(next_commit.sha, 5 * 1024 * 1024)?;
    let shared_hunk = hunk(shared_commit.sha, 5 * 1024 * 1024)?;
    require_equal(
        &next_hunk.record,
        &shared_hunk.record,
        "fixture large History rows share their value",
    )?;
    let mut next_batch = HistoryIngestBatch {
        generation: next_generation.generation,
        manifest_digest: Some(next_manifest.lexical.manifest_digest),
        commits: vec![next_commit.clone(), shared_commit.clone()],
        diff_hunks: vec![next_hunk.clone(), shared_hunk.clone()],
        batch_digest: String::new(),
        ..batch
    };
    quanta_index_ipc::stamp_batch_digest_v1(&mut next_batch)?;
    let next_ack = publisher
        .history()
        .publish_history_borrowed_v1(&next_batch)?;
    require(
        next_ack.applied && next_ack.durable_sequence > ack.durable_sequence,
        "second Legacy History ACK must follow the first",
    )?;
    drop(publisher);
    old_process.stop()?;

    require_equal(
        &fs::read(&selection)?,
        &selected_bytes,
        "Legacy publication selection",
    )?;
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
    for (generation, commits, hunks, ack, expect_shared_part) in [
        (&generation, &[&commit][..], &[&first_hunk][..], &ack, false),
        (
            &next_generation,
            &[&next_commit, &shared_commit][..],
            &[&next_hunk, &shared_hunk][..],
            &next_ack,
            true,
        ),
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
        assert_legacy_history_export_v1(
            &output.stdout,
            generation,
            commits,
            hunks,
            ack,
            &old_root,
            expect_shared_part,
        )?;
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

#[cfg(target_os = "linux")]
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
            | HistorySnapshotExportFrameV1::Complete { .. }
            | HistorySnapshotExportFrameV1::NativeObjectBegin { .. }
            | HistorySnapshotExportFrameV1::NativeObjectChunk { .. }
            | HistorySnapshotExportFrameV1::NativeObjectEnd { .. }
            | HistorySnapshotExportFrameV1::NativeHistory { .. }
            | HistorySnapshotExportFrameV1::NativeInventoryComplete { .. }
            | HistorySnapshotExportFrameV1::NativeProtectedComplete { .. } => {
                return Err("History inventory emitted a checkpoint frame".into());
            }
        }
    }
    require(completed, "History inventory completion absent")?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn assert_legacy_history_export_v1(
    mut bytes: &[u8],
    generation: &quanta_index_contract::index::AuxiliaryGenerationKeyV1,
    commits: &[&quanta_index_contract::lex::CommitRecord],
    hunks: &[&quanta_index_contract::HistoryDiffHunkUpsert],
    ack: &quanta_index_contract::BatchPublishReceipt,
    old_root: &Path,
    expect_shared_part: bool,
) -> TestResult {
    use quanta_index_contract::AuxEpochV1;
    use quanta_index_contract::index::{
        HistorySnapshotExportFrameV1, HistorySnapshotPageV1, MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1,
        ObjectRef, native_immutable_object_ref_v1,
    };
    use quanta_index_core::index::{
        IndexError, RegisteredObject, StoredObject, WorkBudget, WorkLimits,
        history_checkpoint::{
            LegacyHistoryCheckpointPageRefV2, decode_large_row_v2, is_large_row_v2,
        },
    };
    use quanta_index_core::{AuxiliaryRowFamilyV1, domains::auxiliary::HistorySnapshotDigestV1};

    let mut next_ordinal = 1_u32;
    let mut rows = 0_u64;
    let mut current_object: Option<(ObjectRef, usize, Vec<ObjectRef>, Vec<u8>)> = None;
    let mut objects: Vec<(ObjectRef, Vec<u8>, Vec<ObjectRef>)> = Vec::new();
    let mut seen_large_parts = BTreeMap::<ObjectRef, ()>::new();
    let mut saw_shared_part = false;
    let mut large_hunks = 0_usize;
    let mut digest = HistorySnapshotDigestV1::new(generation);
    let mut expected_commits = BTreeMap::<Vec<u8>, Vec<u8>>::new();
    for commit in commits {
        require(
            expected_commits
                .insert(
                    commit.sha.as_bytes().to_vec(),
                    quanta_index_ipc::encode_cbor_payload(*commit)?,
                )
                .is_none(),
            "History fixture repeats a commit key",
        )?;
    }
    let mut expected_hunks = BTreeMap::<Vec<u8>, Vec<u8>>::new();
    for hunk in hunks {
        let mut key = hunk.commit_sha.as_bytes().to_vec();
        key.push(0x1f);
        key.extend_from_slice(hunk.file_path.as_bytes());
        require(
            expected_hunks
                .insert(key, quanta_index_ipc::encode_cbor_payload(&hunk.record)?)
                .is_none(),
            "History fixture repeats a diff-hunk key",
        )?;
    }
    let expected_large_hunks = hunks
        .iter()
        .filter(|hunk| hunk.record.added_text.len() == 5 * 1024 * 1024)
        .count();
    let expected_epoch = quanta_index_ipc::encode_cbor_payload(&AuxEpochV1::new(1))?;
    let mut seen_epoch = false;
    let mut seen_meta = false;
    let mut previous_row: Option<(String, Vec<u8>)> = None;
    let mut check_row = |family: AuxiliaryRowFamilyV1, key: &[u8], value: &[u8]| -> TestResult {
        if rows == 0 {
            require_equal(&family, &AuxiliaryRowFamilyV1::Epoch, "first History row")?;
        } else {
            let current = (family.as_code_str().to_owned(), key.to_vec());
            require(
                previous_row.as_ref().is_none_or(|prior| prior < &current),
                "History rows differ from physical family/key order",
            )?;
            previous_row = Some(current);
        }
        match family {
            AuxiliaryRowFamilyV1::Epoch => {
                require(!seen_epoch && key.is_empty(), "History epoch repeated")?;
                require_equal(&value, &expected_epoch.as_slice(), "History original epoch")?;
                seen_epoch = true;
            }
            AuxiliaryRowFamilyV1::Commit => {
                let expected = expected_commits
                    .remove(key)
                    .ok_or("History export has a foreign or repeated commit")?;
                require_equal(&value, &expected.as_slice(), "History original commit")?;
            }
            AuxiliaryRowFamilyV1::DiffHunk => {
                let expected = expected_hunks
                    .remove(key)
                    .ok_or("History export has a foreign or repeated diff hunk")?;
                require_equal(&value, &expected.as_slice(), "History original diff hunk")?;
            }
            AuxiliaryRowFamilyV1::StateMeta => {
                require(!seen_meta && key.is_empty(), "History state meta repeated")?;
                seen_meta = true;
            }
            _ => return Err("History export has an unexpected row family".into()),
        }
        digest.push_row(family, key, value);
        rows += 1;
        Ok(())
    };
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
        let raw: serde_json::Value = serde_json::from_slice(&bytes[4..4 + length])?;
        bytes = &bytes[4 + length..];
        if raw.get("kind").and_then(serde_json::Value::as_str) == Some("checkpoint_page") {
            require(
                current_object.is_none(),
                "History page marker interrupted an object",
            )?;
            let marker: LegacyHistoryCheckpointPageRefV2 = serde_json::from_value(
                raw.get("body")
                    .ok_or("History page marker lacks body")?
                    .clone(),
            )?;
            require_equal(
                &marker.ordinal,
                &next_ordinal,
                "History page marker ordinal",
            )?;
            let payload = objects.last().ok_or("History page marker has no object")?;
            require_equal(&payload.0, &marker.reference, "History page root order")?;
            require_equal(
                &objects.len(),
                &(payload.2.len() + 1),
                "History page closure has an extra or missing object",
            )?;
            require(
                objects[..objects.len() - 1]
                    .iter()
                    .all(|(reference, _, _)| payload.2.contains(reference)),
                "History page closure has an unbound leaf",
            )?;
            if is_large_row_v2(&payload.1) {
                require_equal(&marker.row_count, &1, "large History page row count")?;
                large_hunks += 1;
                for child in &payload.2 {
                    saw_shared_part |= seen_large_parts.insert(*child, ()).is_some();
                }
                let storage = StoredObject::new(
                    RegisteredObject::unscoped(
                        marker.reference,
                        u64::try_from(payload.1.len())?,
                        payload.2.clone(),
                    ),
                    payload.1.clone(),
                    None,
                );
                let mut budget = WorkBudget::new(
                    WorkLimits {
                        reads: 1_000_000,
                        writes: 1_000_000,
                        bytes: 1_000_000_000,
                        mutations: 1_000_000,
                    },
                    Instant::now() + Duration::from_secs(60),
                );
                let (row, _row_memory) = decode_large_row_v2(
                    generation,
                    marker.ordinal,
                    marker.reference,
                    &storage,
                    &mut budget,
                    &mut |reference, _budget| {
                        let (_, payload, children) = objects
                            .iter()
                            .find(|(observed, _, _)| *observed == reference)
                            .ok_or(IndexError::DigestConflict)?;
                        Ok(StoredObject::new(
                            RegisteredObject::unscoped(
                                reference,
                                payload.len() as u64,
                                children.clone(),
                            ),
                            payload.clone(),
                            None,
                        ))
                    },
                )?;
                require_equal(
                    &row.key.family,
                    &AuxiliaryRowFamilyV1::DiffHunk,
                    "large History row family",
                )?;
                check_row(row.key.family, &row.key.row_key, &row.value)?;
            } else {
                require(payload.2.is_empty(), "fixture V1 History page has children")?;
                let page: HistorySnapshotPageV1 = serde_json::from_slice(&payload.1)?;
                page.validate_shape()?;
                require_equal(
                    &payload.1,
                    &serde_json::to_vec(&page)?,
                    "History V1 page canonical bytes",
                )?;
                require_equal(
                    &page.generation,
                    generation,
                    "History export page generation",
                )?;
                require_equal(&page.ordinal, &next_ordinal, "History export page ordinal")?;
                require_equal(
                    &marker.row_count,
                    &u64::try_from(page.rows.len())?,
                    "History page row count",
                )?;
                for row in page.rows {
                    let family = AuxiliaryRowFamilyV1::from_code_str(&row.family)
                        .ok_or("History export family is unknown")?;
                    check_row(family, &row.row_key, &row.value)?;
                }
            }
            objects.clear();
            next_ordinal += 1;
            continue;
        }
        let frame: HistorySnapshotExportFrameV1 = serde_json::from_value(raw)?;
        match frame {
            HistorySnapshotExportFrameV1::NativeObjectBegin {
                reference,
                payload_bytes,
                children,
            } => {
                require(
                    current_object.is_none(),
                    "History object begins before prior end",
                )?;
                let size = usize::try_from(payload_bytes)?;
                require(
                    size <= MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1,
                    "History object exceeds bound",
                )?;
                require(
                    children
                        .iter()
                        .all(|child| objects.iter().any(|(reference, _, _)| reference == child)),
                    "History object child precedes parent",
                )?;
                current_object = Some((reference, size, children, Vec::new()));
            }
            HistorySnapshotExportFrameV1::NativeObjectChunk { offset, bytes } => {
                let current = current_object
                    .as_mut()
                    .ok_or("History object chunk lacks begin")?;
                require_equal(
                    &offset,
                    &u64::try_from(current.3.len())?,
                    "History chunk offset",
                )?;
                require(
                    current
                        .3
                        .len()
                        .checked_add(bytes.len())
                        .is_some_and(|len| len <= current.1),
                    "History chunk exceeds object length",
                )?;
                current.3.extend_from_slice(&bytes);
            }
            HistorySnapshotExportFrameV1::NativeObjectEnd { reference } => {
                let (expected, size, children, payload) = current_object
                    .take()
                    .ok_or("History object end lacks begin")?;
                require_equal(&reference, &expected, "History object end ref")?;
                require_equal(&payload.len(), &size, "History object length")?;
                require_equal(
                    &native_immutable_object_ref_v1(&payload, &children),
                    &Some(reference),
                    "History object hash",
                )?;
                require(
                    !objects.iter().any(|(seen, _, _)| *seen == reference),
                    "History object emitted twice",
                )?;
                objects.push((reference, payload, children));
            }
            frame @ HistorySnapshotExportFrameV1::Complete { .. } => {
                require(
                    current_object.is_none() && objects.is_empty(),
                    "History completion interrupted an object or page",
                )?;
                completion = Some(frame);
            }
            HistorySnapshotExportFrameV1::InventoryKey { .. }
            | HistorySnapshotExportFrameV1::InventoryComplete { .. }
            | HistorySnapshotExportFrameV1::Page(_)
            | HistorySnapshotExportFrameV1::NativeHistory { .. }
            | HistorySnapshotExportFrameV1::NativeInventoryComplete { .. }
            | HistorySnapshotExportFrameV1::NativeProtectedComplete { .. } => {
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
    drop(check_row);
    require(
        seen_epoch
            && seen_meta
            && expected_commits.is_empty()
            && expected_hunks.is_empty()
            && next_ordinal > 1,
        "History export omitted or changed an original row",
    )?;
    require_equal(
        &rows,
        &u64::try_from(commits.len() + hunks.len() + 2)?,
        "History original row count",
    )?;
    require_equal(
        &large_hunks,
        &expected_large_hunks,
        "History page V1 or large-row encoding",
    )?;
    require_equal(
        &saw_shared_part,
        &expect_shared_part,
        "History large-row raw part reuse",
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

fn assert_legacy_source_export_v1(
    mut bytes: &[u8],
    active: &quanta_index_contract::SearchCorpusActiveHeadV1,
    receipt: &quanta_index_contract::BatchPublishReceipt,
) -> TestResult {
    use quanta_index_contract::index::{
        MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1, SourceSnapshotExportFrameV1 as Frame,
    };
    use quanta_index_contract::{
        RepoId, RepoRelativePath, RevisionId, SourceFileKey, SourceFileRevision,
    };

    let mut frames = Vec::new();
    while !bytes.is_empty() {
        require(
            bytes.len() >= 4,
            "Source export has a truncated frame length",
        )?;
        let length = usize::try_from(u32::from_le_bytes(bytes[..4].try_into()?))?;
        require(
            length > 0
                && length <= MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1 + 4096
                && bytes.len() >= length + 4,
            "Source export frame exceeds bound or is truncated",
        )?;
        frames.push(Frame::decode_json_v1(&bytes[4..4 + length])?);
        bytes = &bytes[4 + length..];
    }
    let [
        Frame::HeadBegin {
            repo_id,
            revision_id,
            active: exported_active,
            applied_ack,
            source_publication_event,
            source_heads,
            repomap_active,
        },
        Frame::FileBegin {
            source: first,
            text_admitted: first_admitted,
            source_bytes: first_size,
        },
        Frame::FileChunk {
            offset: first_offset,
            bytes: first_bytes,
        },
        Frame::FileEnd { source: first_end },
        Frame::FileBegin {
            source: second,
            text_admitted: second_admitted,
            source_bytes: second_size,
        },
        Frame::FileChunk {
            offset: second_offset,
            bytes: second_bytes,
        },
        Frame::FileEnd { source: second_end },
        Frame::HeadComplete {
            repo_id: completed_repo,
            revision_id: completed_revision,
            file_count,
            source_heads: completed_heads,
        },
        Frame::InventoryComplete {
            head_count,
            repomap_head_count,
        },
    ] = frames.as_slice()
    else {
        return Err(
            "Source export frame order or count differs from fixed two-file inventory".into(),
        );
    };
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    require_equal(repo_id, &repo, "Source head repository")?;
    require_equal(revision_id, &revision, "Source head revision")?;
    require_equal(exported_active, active, "Source active head")?;
    require_equal(applied_ack, receipt, "Source original SDK ACK")?;
    require(applied_ack.sealed, "Source head ACK is not sealed")?;
    require_equal(
        &applied_ack.generation,
        &active.generation.lexical.manifest_generation,
        "Source head ACK generation",
    )?;
    require_equal(
        &applied_ack.manifest_digest.as_deref(),
        &Some(active.generation.lexical.manifest_digest.as_str()),
        "Source head ACK manifest",
    )?;
    require_equal(
        &source_publication_event.stream_id.as_str(),
        &"e2e-harness-source-v1",
        "Source publication stream",
    )?;
    require_equal(
        &source_publication_event.event_id.as_str(),
        &"e2e-source-event-1",
        "Source publication event",
    )?;
    require_equal(
        &source_publication_event.expected_base_event_id,
        &None,
        "Source full publication base event",
    )?;
    require(
        repomap_active.is_none(),
        "fixed Source head unexpectedly has RepoMap",
    )?;
    require(
        source_heads.is_canonical_v1(),
        "Source head commitment is invalid",
    )?;
    require_equal(completed_repo, repo_id, "Source completed repository")?;
    require_equal(completed_revision, revision_id, "Source completed revision")?;
    require_equal(completed_heads, source_heads, "Source completed commitment")?;
    require_equal(file_count, &2, "Source completed file count")?;
    require_equal(head_count, &1, "Source inventory head count")?;
    require_equal(repomap_head_count, &0, "Source inventory RepoMap count")?;

    for (source, admitted, size, offset, observed, end, path, body) in [
        (
            first,
            first_admitted,
            first_size,
            first_offset,
            first_bytes,
            first_end,
            "src/decoy.rs",
            "pub fn unrelated() {}\n",
        ),
        (
            second,
            second_admitted,
            second_size,
            second_offset,
            second_bytes,
            second_end,
            LEGACY_BOOT_PATH,
            "pub fn legacy_boot_needle_47() -> bool { true }\n",
        ),
    ] {
        let expected_bytes = body.as_bytes();
        let expected_source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: repo.clone(),
                repo_relative_path: RepoRelativePath::new(path),
            },
            revision_id: revision.clone(),
            source_sha256: Sha256::digest(expected_bytes).into(),
        };
        require_equal(source, &expected_source, "Source file identity")?;
        require(*admitted, "fixed Source text admission was lost")?;
        require_equal(
            size,
            &u64::try_from(expected_bytes.len())?,
            "Source file byte count",
        )?;
        require_equal(offset, &0, "Source first chunk offset")?;
        require_equal(
            &observed.as_slice(),
            &expected_bytes,
            "Source raw file bytes",
        )?;
        require_equal(end, source, "Source file completion identity")?;
    }
    Ok(())
}

#[test]
fn legacy_to_fresh_native_preserves_old_root_and_refuses_unverified_rollback() -> TestResult {
    // Q-only fixture: the external old-Q publishes this fixed Source through
    // its SDK, while production S authority remains a separate qualification.
    let temp = tempfile::Builder::new().prefix("qbl-").tempdir_in("/tmp")?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let base = temp.path().canonicalize()?;
    let legacy = LegacySearchdBinary::from_environment()?;
    let environment = legacy_boot_environment(&base)?;
    let seed = base.join("legacy-seed");
    fs::create_dir(&seed)?;
    fs::set_permissions(&seed, fs::Permissions::from_mode(0o700))?;
    let old_process = legacy.start(&seed, LEGACY_FIXTURE_HISTORY_KEEP)?;
    let publisher = old_process.connect()?;
    let (head, source_receipt) = publish_legacy_source(
        &publisher,
        &quanta_index_contract::RepoId::new("repo-e2e")?,
        &quanta_index_contract::RevisionId::new("rev-e2e")?,
        1,
        None,
        &[
            (LEGACY_BOOT_PATH, LEGACY_BOOT_BODY, 1),
            ("src/decoy.rs", "pub fn unrelated() {}", 2),
        ],
        None,
    )?;
    let generation = head.generation.lexical.manifest_generation;
    require_equal(
        &generation,
        &quanta_index_contract::ManifestGeneration::new(1),
        "Legacy sealed fixture generation",
    )?;
    drop(publisher);
    old_process.stop()?;

    let backup = base.join("legacy-backup");
    let old_root = base.join("legacy-current");
    for (operation, source, destination) in [
        ("backup-state", seed.as_path(), backup.as_path()),
        ("restore-state", backup.as_path(), old_root.as_path()),
    ] {
        successful_legacy_state_command(
            &legacy,
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
        &legacy,
        &[
            "verify-state".into(),
            "--source".into(),
            old_root.to_string_lossy().into_owned(),
        ],
        &environment,
    )?;

    let candidate = rebuild_fixed_native_source(&base, "candidate")?;
    let selection = base.join("selection.json");
    let installed_old_binary = legacy.installer_binary()?;
    let binary = installed_old_binary.path();
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
    installed_old_binary.verify()?;
    legacy.verify()?;
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
    let source_export_args = |digest: &str| -> TestResult<Output> {
        run_private_pipe_one_shot(&[
            "export-native-source-snapshot",
            "--selection-path",
            selection.to_str().ok_or("selection is not UTF-8")?,
            "--expected-current-sha256",
            digest,
            "--config",
            candidate.config.to_str().ok_or("config is not UTF-8")?,
            "--config-digest",
            &config_digest,
        ])
    };
    let stale_source_export = source_export_args(&wrong_digest)?;
    let source_export = source_export_args(&old_digest)?;
    require_equal(&fs::read(&selection)?, &old_wire, "Source export selection")?;
    require_equal(
        &root_files(&old_root)?,
        &before_export,
        "Source export Legacy root",
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
    if cfg!(target_os = "linux") {
        let native_digest = inspected_selection_digest(&selection)?;
        let native_inventory = run_private_pipe_one_shot(&[
            "export-native-history-checkpoint",
            "--selection-path",
            selection.to_str().ok_or("selection is not UTF-8")?,
            "--expected-current-sha256",
            &native_digest,
            "--inventory-repo-id",
            "repo-e2e",
            "--config",
            candidate.config.to_str().ok_or("config is not UTF-8")?,
            "--config-digest",
            &config_digest,
        ])?;
        require(
            native_inventory.status.success() && !native_inventory.stdout.is_empty(),
            format!(
                "selected Native History inventory refused: {}",
                String::from_utf8_lossy(&native_inventory.stderr)
            ),
        )?;

        let foreign_config = base.join("foreign-native-config.json");
        fs::copy(&candidate.config, &foreign_config)?;
        let foreign_path = foreign_config.to_str().ok_or("config is not UTF-8")?;
        let native_config_path = candidate.config.to_str().ok_or("config is not UTF-8")?;
        for (operation, extra) in [
            ("export-native-history-checkpoint", vec!["--inventory-repo-id", "repo-e2e"]),
            ("export-native-source-snapshot", vec!["--source-only-native"]),
        ] {
            let mut args = vec![
                operation,
                "--selection-path",
                selection.to_str().ok_or("selection is not UTF-8")?,
                "--expected-current-sha256",
                native_digest.as_str(),
                "--config",
                foreign_path,
                "--config-digest",
                config_digest.as_str(),
            ];
            args.extend(extra);
            let refused = run_private_pipe_one_shot(&args)?;
            require_equal(&refused.status.code(), &Some(70), "foreign Native config exit")?;
            require(refused.stdout.is_empty(), "foreign Native config emitted frames")?;
            require(
                String::from_utf8_lossy(&refused.stderr)
                    .contains("config differs from the held selection"),
                "foreign Native config refusal missed the selected config binding",
            )?;
        }
        let route_less = run_private_pipe_one_shot(&[
            "export-native-source-snapshot",
            "--source-only-native",
            "--selection-path",
            selection.to_str().ok_or("selection is not UTF-8")?,
            "--expected-current-sha256",
            &native_digest,
            "--config",
            native_config_path,
            "--config-digest",
            &config_digest,
        ])?;
        require_equal(&route_less.status.code(), &Some(70), "route-less Native export exit")?;
        require(route_less.stdout.is_empty(), "route-less Native export emitted frames")?;
        require(
            String::from_utf8_lossy(&route_less.stderr)
                .contains("requires protected Source routes"),
            "route-less Native export did not refuse before transcript",
        )?;
        require_equal(&fs::read(&selection)?, &selected_native, "Native export selection")?;
        require_equal(
            &root_files(&candidate.root)?,
            &native_quiescent_files,
            "Native export changed the selected root",
        )?;
    }
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
    if cfg!(target_os = "linux") {
        require(
            source_export.status.success(),
            format!(
                "selected Source export refused: {}",
                String::from_utf8_lossy(&source_export.stderr)
            ),
        )?;
        assert_legacy_source_export_v1(&source_export.stdout, &head, &source_receipt)?;
    } else {
        require_equal(
            &source_export.status.code(),
            &Some(70),
            "Source export requires Linux owner",
        )?;
        require(
            source_export.stdout.is_empty(),
            "Source export emitted frames without Linux owner",
        )?;
        require(
            String::from_utf8_lossy(&source_export.stderr)
                .contains("admitted boot inventory requires the installed Linux getdents owner"),
            "Source export did not refuse the missing Linux getdents owner",
        )?;
    }
    require_equal(
        &stale_source_export.status.code(),
        &Some(70),
        "stale Source export exit",
    )?;
    require(
        stale_source_export.stdout.is_empty(),
        "stale Source export emitted a frame",
    )?;
    require(
        String::from_utf8_lossy(&stale_source_export.stderr)
            .contains("active selection fingerprint changed"),
        "stale Source export did not reach active selection CAS",
    )?;
    Ok(())
}

#[test]
fn native_cross_root_sdk_rebuild_refuses_unverified_switch() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let base = temp.path().canonicalize()?;
    let release_proof = std::env::var_os("QUANTA_PROOF_DAEMON_WITNESS_DIR").is_some();
    let binary = if release_proof {
        let path = PathBuf::from(
            std::env::var_os("QUANTA_INDEX_SEARCHD_BIN")
                .ok_or("P10 requires the release daemon path in QUANTA_INDEX_SEARCHD_BIN")?,
        );
        require(path.is_absolute(), "P10 daemon path must be absolute")?;
        path
    } else {
        PathBuf::from(env!("CARGO_BIN_EXE_quanta-index-searchd"))
    };
    let old = rebuild_fixed_native_source_from(&base, "old", &binary, REBUILD_SOURCE_PATH, true)?;
    let new = rebuild_fixed_native_source_from(
        &base,
        "new",
        &binary,
        CANDIDATE_REBUILD_SOURCE_PATH,
        true,
    )?;
    require(
        old.published_view != new.published_view
            && old.published_source != new.published_source
            && old.published_source_cut_digest_hex != new.published_source_cut_digest_hex,
        "old root and rebuild candidate must have distinct Published Source",
    )?;
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
    let selected_pid = daemon
        .owned_group
        .ok_or("selected launcher has no owned daemon")?;
    if release_proof {
        use quanta_index_ipc::{KernelPeerCredentials, PeerCredentialsSource as _};
        let stream = std::os::unix::net::UnixStream::connect(&old.socket)?;
        require_equal(
            &KernelPeerCredentials.peer_credentials(&stream)?.pid,
            &Some(selected_pid),
            "selected launcher UDS peer",
        )?;
    }
    let selected_observation = read_view_daemon_witness::observe(selected_pid)?;
    daemon.stop()?;
    require_equal(
        &fs::read(&selection)?,
        &old_wire,
        "selected Native serve selection",
    )?;

    let old_preserved = root_files(&old.root)?;
    let receipt: serde_json::Value = serde_json::from_slice(&fs::read(&new.receipt)?)?;
    require_equal(
        &receipt["manifest_operation"],
        &serde_json::to_value(operation(2))?,
        "verified rebuild receipt operation",
    )?;
    require_equal(
        &receipt["source_cut_digest_hex"].as_str(),
        &Some(new.published_source_cut_digest_hex.as_str()),
        "verified rebuild receipt Source",
    )?;
    let config_digest = hex(&Sha256::digest(fs::read(&new.config)?))?;
    let old_receipt: serde_json::Value = serde_json::from_slice(&fs::read(&old.receipt)?)?;
    let foreign_verify = identity_args(
        "verify-native-state",
        &new.config,
        &old_receipt,
        &config_digest,
    )?;
    require_boot_refusal(run_one_shot_from(&new.binary, &foreign_verify)?)?;
    require_equal(
        &root_files(&new.root)?,
        &new_files,
        "foreign receipt refusal kept candidate root",
    )?;
    let verify_args = identity_args("verify-native-state", &new.config, &receipt, &config_digest)?;
    let _verified = successful_from(&new.binary, &verify_args)?;
    require_equal(
        &root_files(&new.root)?,
        &new_files,
        "verified rebuild root bytes/inodes",
    )?;
    require_equal(
        &fs::read(new.root.join(NATIVE_INDEX_STATE_MARKER_NAME))?.as_slice(),
        &NATIVE_INDEX_STATE_MARKER_CONTENT,
        "verified rebuild persisted state format",
    )?;

    let serve_args = identity_args("serve-native", &new.config, &receipt, &config_digest)?;
    let reopened = selected_daemon_from(&new.binary, &serve_args, &new.socket)?;
    let client = NativeUdsClient::new(new.socket.clone());
    let latest = client.read_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    require_equal(
        &latest.identity,
        &new.published_view,
        "reopened rebuild Latest",
    )?;
    let reopened_cut = quanta_index_contract::native_boot_source_cut_digest_v1(&latest.source)?;
    require_equal(
        &hex(&reopened_cut.0)?,
        &new.published_source_cut_digest_hex,
        "reopened rebuild Source",
    )?;
    require_equal(
        &client.status(operation(2), &budget())?,
        &OperationStatus::Published(new.published_view),
        "reopened rebuild operation",
    )?;
    require_equal(
        &rebuilt_query_source(
            &client,
            ViewSelection::Exact(new.published_view),
            CANDIDATE_REBUILD_SOURCE_PATH,
        )?,
        &new.published_source,
        "reopened rebuild hit Source",
    )?;
    let reopened_observation = read_view_daemon_witness::observe(reopened.0.id())?;
    reopened.stop()?;
    require_equal(
        &root_files(&old.root)?,
        &old_preserved,
        "independent rebuild kept the old root",
    )?;
    require_equal(
        &fs::read(&selection)?,
        &old_wire,
        "independent rebuild kept active selection",
    )?;
    if release_proof {
        require_native_only_legacy_offline_refusal(&new.binary)?;
    }
    read_view_daemon_witness::write(
        [
            old.startup_observation,
            new.startup_observation,
            selected_observation,
            reopened_observation,
        ],
        "native-p10-rebuild-daemon-witness.json",
        &nextest_test_event(
            module_path!(),
            "native_cross_root_sdk_rebuild_refuses_unverified_switch",
        ),
        None,
    )
}
