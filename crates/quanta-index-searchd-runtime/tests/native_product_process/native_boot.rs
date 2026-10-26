use super::*;
use std::{collections::BTreeMap, os::unix::fs::MetadataExt as _, path::PathBuf, process::Output};

use sha2::{Digest as _, Sha256};

#[path = "../common/searchd_fixture_env.rs"]
mod searchd_fixture_env;

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
    if mode == "verify-native-state" {
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

    let verify_args = identity_args(
        "verify-native-state",
        &config_path,
        &receipt,
        &config_digest,
    )?;
    let serve_args = identity_args("serve-native", &config_path, &receipt, &config_digest)?;
    let before_verify = root_files(&root)?;
    let _verified = successful(&verify_args)?;
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
    active.stop()?;

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
    let renewed_install = with_flag(
        &install_args,
        "--receipt",
        renewed_receipt_path.to_string_lossy().into_owned(),
    )?;
    launcher_succeeded(&renewed_install)?;
    let updated_selection: serde_json::Value = serde_json::from_slice(&fs::read(&selection_path)?)?;
    require_equal(
        updated_selection
            .get("native_boot_receipt")
            .ok_or("missing updated selection field native_boot_receipt")?,
        &renewed_receipt,
        "renewed installation receipt",
    )?;
    require_equal(
        updated_selection
            .get("native_config_sha256")
            .ok_or("missing updated selection field native_config_sha256")?,
        &serde_json::json!(config_digest),
        "renewed installation config digest",
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
}

impl RebuiltNativeRoot {
    fn renew_receipt(&mut self, receipt: PathBuf) -> TestResult {
        let _selected = successful(&[
            "select-native-state".into(),
            "--config".into(),
            self.config.to_string_lossy().into_owned(),
            "--manifest-operation".into(),
            serde_json::to_string(&operation(2))?,
            "--receipt".into(),
            receipt.to_string_lossy().into_owned(),
        ])?;
        self.receipt = receipt;
        Ok(())
    }

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
    let rebuilt = published(&client.submit(
        UpdateRequest {
            operation: operation(2),
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

const LEGACY_BOOT_PATH: &str = "src/legacy-cutover.rs";
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

fn require_legacy_boot_query(root: &Path) -> TestResult {
    let client = quanta_index_sdk::QuantaIndex::connect_query_only(
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
fn legacy_to_fresh_native_and_rollback_serves_fixed_old_root_query_twice() -> TestResult {
    // Q-only fixture: the actual external producer's S publication tuple is a
    // separate handoff qualification, not an assertion of this test.
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

    let mut candidate = rebuild_fixed_native_source(&base, "candidate")?;
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
    // A read-only Native serve can still change Catalog bytes. Reissue the
    // canonical current-root receipt before rollback's Native verification.
    candidate.renew_receipt(base.join("candidate-renewed-receipt.json"))?;
    launcher_succeeded(&candidate.launcher_args("install-native", &selection)?)?;
    let native_quiescent_files = root_files(&candidate.root)?;
    launcher_succeeded(&[
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
        &fs::read(&selection)?,
        &old_wire,
        "rolled-back Legacy selection",
    )?;
    require_equal(
        &fs::read(&previous)?,
        &old_wire,
        "rollback changed saved selection",
    )?;
    for _ in 0..2 {
        let daemon = launcher_daemon(&selection, &legacy_socket)?;
        require_legacy_boot_query(&old_root)?;
        daemon.stop()?;
        require(
            UnixStream::connect(&legacy_socket).is_err(),
            "rolled-back Legacy serve did not drain",
        )?;
        require_equal(
            &fs::read(&selection)?,
            &old_wire,
            "rolled-back Legacy selection drift",
        )?;
    }
    require_equal(
        &root_files(&candidate.root)?,
        &native_quiescent_files,
        "Legacy rollback serve changed inactive Native root",
    )?;
    Ok(())
}

#[test]
fn native_cross_root_sdk_rebuild_switch_and_rollback_preserve_fixed_query() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let base = temp.path().canonicalize()?;
    let mut old = rebuild_fixed_native_source(&base, "old")?;
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
    launcher_succeeded(&switch)?;
    require_equal(
        &root_files(&old.root)?,
        &old_files,
        "switch old root bytes/inodes",
    )?;
    require_equal(
        &root_files(&new.root)?,
        &new_files,
        "switch candidate root bytes/inodes",
    )?;
    let new_wire = fs::read(&selection)?;
    launcher_refused(&switch)?;
    require_equal(&fs::read(&selection)?, &new_wire, "stale CAS selection")?;
    // Rollback uses the exact same verified switch with the newly inspected CAS.
    let mut rollback = old.launcher_args("switch-native", &selection)?;
    rollback.extend([
        "--expected-selection-sha256".into(),
        inspected_selection_digest(&selection)?,
    ]);
    launcher_succeeded(&rollback)?;
    require_equal(
        &fs::read(&selection)?,
        &old_wire,
        "explicit rollback selection",
    )?;
    for _ in 0..2 {
        let daemon = launcher_daemon(&selection, &old.socket)?;
        require_rebuilt_query(&old.socket)?;
        daemon.stop()?;
        require_equal(
            &fs::read(&selection)?,
            &old_wire,
            "rollback serve/restart changed deployment selection",
        )?;
    }
    require_equal(
        &root_files(&new.root)?,
        &new_files,
        "rollback serve changed inactive candidate root",
    )?;
    // Serve may legitimately change Catalog bytes. Reissue its install receipt
    // through the existing offline path before verifying the current root again.
    old.renew_receipt(base.join("old-renewed-receipt.json"))?;
    launcher_succeeded(&old.launcher_args("install-native", &selection)?)?;
    let mut forward = new.launcher_args("switch-native", &selection)?;
    forward.extend([
        "--expected-selection-sha256".into(),
        inspected_selection_digest(&selection)?,
    ]);
    launcher_succeeded(&forward)?;
    let daemon = launcher_daemon(&selection, &new.socket)?;
    require_rebuilt_query(&new.socket)?;
    daemon.stop()?;
    Ok(())
}
