//! Actual product commands against a persisted Semantic profile, with no key.
//! This qualifies offline profile admission, not installed recovery.
use super::*;

fn without_environment(args: &[String]) -> TestResult<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"));
    let _command = command.env_clear().args(args);
    bounded_output(
        |stdout, stderr| Ok(command.stdout(stdout).stderr(stderr).spawn()?),
        args,
        false,
    )
}

#[test]
fn native_semantic_offline_boot_needs_no_credentials_or_provider_calls() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let base = temp.path().canonicalize()?;
    let root = base.join("state");
    let socket = base.join("native.sock");
    let config_path = base.join("semantic.json");
    let receipt_path = base.join("boot.json");
    let stub = EmbeddingStub::start()?;
    let base_url = stub.base_url();
    let mut configuration = config_with_openai_semantic(&root, &socket)?;
    configure_semantic_endpoint(&mut configuration, &base_url)?;
    let config_bytes = serde_json::to_vec(&configuration)?;
    fs::write(&config_path, &config_bytes)?;
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))?;
    let config_digest = hex(&Sha256::digest(&config_bytes))?;
    let daemon = Daemon::start_with_api_key(&config_path, &socket, Some("test-key"))?;
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("Semantic offline workspace creation failed".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![text_spec(), openai_semantic_spec(&base_url)?],
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
    require(configured != genesis, "Semantic profile did not publish")?;
    daemon.stop()?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &0,
        "empty Source called provider",
    )?;
    let select = vec![
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
    let output = without_environment(&select)?;
    require(
        output.status.success(),
        format!(
            "credential-free Semantic select refused: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    let receipt: serde_json::Value = serde_json::from_slice(&fs::read(&receipt_path)?)?;
    let verify = identity_args(
        "verify-native-state",
        &config_path,
        &receipt,
        &config_digest,
    )?;
    let before = root_files(&root)?;
    let output = without_environment(&verify)?;
    require(
        output.status.success(),
        format!(
            "credential-free Semantic verify refused: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    require_equal(
        &root_files(&root)?,
        &before,
        "offline verify changed Native files",
    )?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &0,
        "offline boot called provider",
    )?;
    let serve = identity_args("serve-native", &config_path, &receipt, &config_digest)?;
    let output = without_environment(&serve)?;
    require_equal(
        &output.status.code(),
        &Some(70),
        "credential-free live Serve exit",
    )?;
    require(
        output.stdout.is_empty(),
        "refused live Serve emitted output",
    )?;
    require(
        String::from_utf8_lossy(&output.stderr)
            .contains("native OpenAI API key environment variable is missing"),
        "live Serve lost its credential requirement",
    )?;
    require_equal(
        &root_files(&root)?,
        &before,
        "refused live Serve changed Native files",
    )?;
    require(!socket.exists(), "refused live Serve created listener")?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &0,
        "refused live Serve called provider",
    )
}
