use std::path::PathBuf;

use anyhow::{Context as _, Result};

use super::native::{NativeCommandInput, NativeCommandModeV1, NativeStateCommandV1};
use crate::DiagnosticCaptureLimitsV1;
use crate::app::config::SearchdConfig;
use crate::app::state_migration::{OfflineStateCommandV1, OfflineStateOperationV1};

/// CLI subcommand surface: `serve [--state-root PATH]` plus the offline
/// state commands `backup-state`, `restore-state` and
/// `verify-state` (SEP-21 P10 / S21-11).
///
/// `--state-root` replaces only how the state root is resolved; every
/// policy family still comes from env through the one config chain
/// ([`SearchdConfig::from_env_with_state_root`]), so an explicit root never
/// silently drops a socket-access, envelope or admission knob (QI-BB-014,
/// QI-BB-016).
///
/// The offline commands are parsed here and executed by the composition root.
/// They are **not** a second daemon mode: an invocation that names one has no
/// serve config, so [`SearchdCommand::into_config`] refuses it rather than
/// booting a daemon that would ignore the request.
#[derive(Clone, Debug)]
pub struct SearchdCommand {
    state_root_override: Option<PathBuf>,
    diagnostic_trace_path: Option<PathBuf>,
    diagnostic_trace_max_output_bytes: Option<u64>,
    native_command: Option<NativeStateCommandV1>,
    offline: Option<OfflineStateCommandV1>,
}

impl SearchdCommand {
    pub fn from_env() -> Result<Self> {
        Self::from_args(std::env::args().skip(1))
    }

    fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut state_root_override: Option<PathBuf> = None;
        let mut diagnostic_trace_path: Option<PathBuf> = None;
        let mut diagnostic_trace_max_output_bytes: Option<u64> = None;
        let mut native_input = NativeCommandInput::default();
        let mut native_mode: Option<NativeCommandModeV1> = None;
        let mut legacy_serve = false;
        let mut source_root: Option<PathBuf> = None;
        let mut destination_root: Option<PathBuf> = None;
        let mut operation: Option<OfflineStateOperationV1> = None;
        let mut selected_command = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "serve" => {
                    select_command(&mut selected_command, "serve")?;
                    legacy_serve = true;
                }
                "serve-native" => {
                    select_command(&mut selected_command, "serve-native")?;
                    native_mode = Some(NativeCommandModeV1::Serve);
                }
                "verify-native-state" => {
                    select_command(&mut selected_command, "verify-native-state")?;
                    native_mode = Some(NativeCommandModeV1::Verify);
                }
                "select-native-state" => {
                    select_command(&mut selected_command, "select-native-state")?;
                    native_mode = Some(NativeCommandModeV1::Select);
                }
                "--config" => {
                    let next = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--config requires a path"))?;
                    if native_input
                        .config_path
                        .replace(PathBuf::from(next))
                        .is_some()
                    {
                        return Err(anyhow::anyhow!("--config may be specified only once"));
                    }
                }
                "--root-nonce" => {
                    set_once(&mut native_input.root_nonce, &mut args, "--root-nonce")?;
                }
                "--manifest-operation" => set_once(
                    &mut native_input.manifest_operation,
                    &mut args,
                    "--manifest-operation",
                )?,
                "--manifest-object" => set_once(
                    &mut native_input.manifest_object,
                    &mut args,
                    "--manifest-object",
                )?,
                "--source-cut-digest" => set_once(
                    &mut native_input.source_cut_digest,
                    &mut args,
                    "--source-cut-digest",
                )?,
                "--config-digest" => set_once(
                    &mut native_input.config_digest,
                    &mut args,
                    "--config-digest",
                )?,
                "--catalog-content-digest" => set_once(
                    &mut native_input.install_catalog_digest,
                    &mut args,
                    "--catalog-content-digest",
                )?,
                "--receipt" => {
                    let next = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--receipt requires a path"))?;
                    if native_input
                        .receipt_path
                        .replace(PathBuf::from(next))
                        .is_some()
                    {
                        return Err(anyhow::anyhow!("--receipt may be specified only once"));
                    }
                }
                "backup-state" => {
                    select_command(&mut selected_command, "backup-state")?;
                    operation = Some(OfflineStateOperationV1::Backup);
                }
                "restore-state" => {
                    select_command(&mut selected_command, "restore-state")?;
                    operation = Some(OfflineStateOperationV1::Restore);
                }
                "verify-state" => {
                    select_command(&mut selected_command, "verify-state")?;
                    operation = Some(OfflineStateOperationV1::Verify);
                }
                "--state-root" => {
                    let next = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--state-root requires a path"))?;
                    if state_root_override.replace(PathBuf::from(next)).is_some() {
                        return Err(anyhow::anyhow!("--state-root may be specified only once"));
                    }
                }
                "--diagnostic-trace" => {
                    let next = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--diagnostic-trace requires a path"))?;
                    if diagnostic_trace_path.replace(PathBuf::from(next)).is_some() {
                        return Err(anyhow::anyhow!(
                            "--diagnostic-trace may be specified only once"
                        ));
                    }
                }
                "--trace-max-output-bytes" => {
                    let raw = args.next().ok_or_else(|| {
                        anyhow::anyhow!("--trace-max-output-bytes requires a value")
                    })?;
                    let bytes = raw
                        .parse::<u64>()
                        .context("--trace-max-output-bytes must be a positive u64")?;
                    let _validated_limits = DiagnosticCaptureLimitsV1::default()
                        .with_max_output_bytes(bytes)
                        .context("--trace-max-output-bytes is outside capture limits")?;
                    if diagnostic_trace_max_output_bytes.replace(bytes).is_some() {
                        return Err(anyhow::anyhow!(
                            "--trace-max-output-bytes may be specified only once"
                        ));
                    }
                }
                "--source" => {
                    let next = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--source requires a path"))?;
                    if source_root.replace(PathBuf::from(next)).is_some() {
                        return Err(anyhow::anyhow!("--source may be specified only once"));
                    }
                }
                "--destination" => {
                    let next = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--destination requires a path"))?;
                    if destination_root.replace(PathBuf::from(next)).is_some() {
                        return Err(anyhow::anyhow!("--destination may be specified only once"));
                    }
                }
                other => return Err(anyhow::anyhow!("unknown argument: {other}")),
            }
        }
        if diagnostic_trace_max_output_bytes.is_some() && diagnostic_trace_path.is_none() {
            return Err(anyhow::anyhow!(
                "--trace-max-output-bytes requires --diagnostic-trace"
            ));
        }
        let offline = match operation {
            None => {
                if source_root.is_some() || destination_root.is_some() {
                    return Err(anyhow::anyhow!(
                        "--source/--destination require one of: backup-state, restore-state, verify-state"
                    ));
                }
                None
            }
            Some(operation) => {
                if diagnostic_trace_path.is_some() {
                    return Err(anyhow::anyhow!(
                        "--diagnostic-trace is only valid for serve"
                    ));
                }
                if state_root_override.is_some() {
                    return Err(anyhow::anyhow!(
                        "--state-root configures `serve`; an offline state command names its roots with --source/--destination"
                    ));
                }
                let source_root = source_root.ok_or_else(|| {
                    anyhow::anyhow!("{} requires --source PATH", operation.command_name())
                })?;
                if operation == OfflineStateOperationV1::Verify {
                    if destination_root.is_some() {
                        return Err(anyhow::anyhow!(
                            "verify-state names one root; it takes --source only"
                        ));
                    }
                } else if destination_root.is_none() {
                    return Err(anyhow::anyhow!(
                        "{} requires --destination PATH",
                        operation.command_name()
                    ));
                }
                Some(OfflineStateCommandV1 {
                    operation,
                    source_root,
                    destination_root,
                })
            }
        };
        let native_command = if let Some(mode) = native_mode {
            if legacy_serve || offline.is_some() || state_root_override.is_some() {
                return Err(anyhow::anyhow!(
                    "native command cannot be mixed with serve, offline or --state-root"
                ));
            }
            if diagnostic_trace_path.is_some() && mode != NativeCommandModeV1::Serve {
                return Err(anyhow::anyhow!(
                    "--diagnostic-trace requires serve or serve-native"
                ));
            }
            Some(native_input.finish(mode)?)
        } else if native_input.has_any() {
            return Err(anyhow::anyhow!("native flags require a native command"));
        } else {
            None
        };
        Ok(Self {
            state_root_override,
            diagnostic_trace_path,
            diagnostic_trace_max_output_bytes,
            native_command,
            offline,
        })
    }

    /// The offline state command this invocation names, if any.
    #[must_use]
    pub fn offline_operation(&self) -> Option<&OfflineStateCommandV1> {
        self.offline.as_ref()
    }

    #[must_use]
    pub fn diagnostic_trace_path(&self) -> Option<&PathBuf> {
        self.diagnostic_trace_path.as_ref()
    }

    #[must_use]
    pub fn diagnostic_trace_limits(&self) -> DiagnosticCaptureLimitsV1 {
        DiagnosticCaptureLimitsV1 {
            max_output_bytes: self
                .diagnostic_trace_max_output_bytes
                .unwrap_or_else(|| DiagnosticCaptureLimitsV1::default().max_output_bytes),
            ..DiagnosticCaptureLimitsV1::default()
        }
    }

    /// An explicit native product process uses this deployment-owned config.
    #[must_use]
    pub fn native_config_path(&self) -> Option<&PathBuf> {
        self.native_command.as_ref().and_then(|command| {
            (command.mode == NativeCommandModeV1::Serve).then_some(&command.config_path)
        })
    }

    #[must_use]
    pub fn native_command(&self) -> Option<&NativeStateCommandV1> {
        self.native_command.as_ref()
    }

    pub fn into_config(self) -> Result<SearchdConfig> {
        if self.native_command.is_some() {
            return Err(anyhow::anyhow!("native command has no legacy serve config"));
        }
        if let Some(offline) = &self.offline {
            return Err(anyhow::anyhow!(
                "this invocation names `{}`; it is an offline state command and has no serve config",
                offline.operation.command_name()
            ));
        }
        self.state_root_override.map_or_else(
            SearchdConfig::from_env,
            SearchdConfig::from_env_with_state_root,
        )
    }
}

fn set_once(
    target: &mut Option<String>,
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<()> {
    let next = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("{flag} requires a value"))?;
    if target.replace(next).is_some() {
        return Err(anyhow::anyhow!("{flag} may be specified only once"));
    }
    Ok(())
}

fn select_command(selected: &mut Option<&'static str>, command: &'static str) -> Result<()> {
    if let Some(previous) = selected {
        return Err(anyhow::anyhow!(
            "only one subcommand may be specified: {previous}, {command}"
        ));
    }
    *selected = Some(command);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::ensure;
    use quanta_index_contract::index::{
        CollectionId, OperationKey, ProducerId, SlotId, StreamId, TenantId, WorkspaceKey,
        WorktreeId,
    };

    fn parse(args: &[&str]) -> Result<SearchdCommand> {
        SearchdCommand::from_args(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn native_capture_is_explicit_and_only_allowed_for_service() -> Result<()> {
        let command = parse(&[
            "serve-native",
            "--config",
            "/tmp/native.json",
            "--diagnostic-trace",
            "/tmp/native.jsonl",
        ])?;
        ensure!(command.native_command().is_some());
        ensure!(command.diagnostic_trace_path().is_some());
        let offline = parse(&[
            "verify-native-state",
            "--config",
            "/tmp/native.json",
            "--diagnostic-trace",
            "/tmp/native.jsonl",
        ]);
        ensure!(offline.is_err_and(|error| error.to_string().contains("requires serve")));
        ensure!(parse(&["serve", "serve-native", "--config", "/tmp/native.json"]).is_err());
        Ok(())
    }

    #[test]
    fn trace_output_limit_requires_capture_and_preserves_default() -> Result<()> {
        let default = parse(&[
            "serve-native",
            "--config",
            "/tmp/native.json",
            "--diagnostic-trace",
            "/tmp/native.jsonl",
        ])?;
        ensure!(default.diagnostic_trace_limits().max_output_bytes == 67_108_864);
        let selected = parse(&[
            "serve-native",
            "--config",
            "/tmp/native.json",
            "--diagnostic-trace",
            "/tmp/native.jsonl",
            "--trace-max-output-bytes",
            "1073741824",
        ])?;
        ensure!(selected.diagnostic_trace_limits().max_output_bytes == 1_073_741_824);
        for value in [
            "0",
            "6143",
            "1073741825",
            "-1",
            "true",
            "18446744073709551616",
        ] {
            ensure!(
                parse(&[
                    "serve-native",
                    "--config",
                    "/tmp/native.json",
                    "--diagnostic-trace",
                    "/tmp/native.jsonl",
                    "--trace-max-output-bytes",
                    value,
                ])
                .is_err()
            );
        }
        ensure!(parse(&["serve", "--trace-max-output-bytes", "1073741824"]).is_err());
        ensure!(
            parse(&[
                "serve-native",
                "--config",
                "/tmp/native.json",
                "--diagnostic-trace",
                "/tmp/native.jsonl",
                "--trace-max-output-bytes",
                "6144",
                "--trace-max-output-bytes",
                "6144",
            ])
            .is_err()
        );
        ensure!(
            parse(&[
                "verify-native-state",
                "--config",
                "/tmp/native.json",
                "--diagnostic-trace",
                "/tmp/native.jsonl",
                "--trace-max-output-bytes",
                "6144",
            ])
            .is_err()
        );
        Ok(())
    }

    fn operation_json() -> Result<String> {
        let operation = OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 5,
            producer: ProducerId([6; 16]),
            stream: StreamId([7; 16]),
            stream_epoch: 8,
            sequence: 9,
        };
        serde_json::to_string(&operation).map_err(Into::into)
    }

    fn identity_args(command: &str) -> Result<Vec<String>> {
        Ok(vec![
            command.into(),
            "--config".into(),
            "/tmp/native.json".into(),
            "--root-nonce".into(),
            "11".repeat(32),
            "--manifest-operation".into(),
            operation_json()?,
            "--manifest-object".into(),
            "22".repeat(32),
            "--source-cut-digest".into(),
            "33".repeat(32),
            "--config-digest".into(),
            "44".repeat(32),
        ])
    }

    fn identity_fixture_arg(args: &[String], index: usize) -> Result<&str> {
        args.get(index)
            .map(String::as_str)
            .ok_or_else(|| anyhow::anyhow!("native identity fixture missing argument {index}"))
    }

    fn identity_fixture_arg_mut(args: &mut [String], index: usize) -> Result<&mut String> {
        args.get_mut(index)
            .ok_or_else(|| anyhow::anyhow!("native identity fixture missing argument {index}"))
    }

    #[test]
    fn single_serve_modes_keep_their_config_boundary() -> Result<()> {
        let implicit = parse(&[])?;
        ensure!(implicit.offline_operation().is_none());
        ensure!(implicit.native_config_path().is_none());
        let legacy = parse(&["serve", "--state-root", "/tmp/state"])?;
        ensure!(legacy.state_root_override == Some(PathBuf::from("/tmp/state")));
        ensure!(legacy.offline_operation().is_none());
        let native = parse(&["serve-native", "--config", "/tmp/native.json"])?;
        ensure!(native.native_config_path() == Some(&PathBuf::from("/tmp/native.json")));
        ensure!(native.offline_operation().is_none());
        ensure!(native.into_config().is_err());
        Ok(())
    }

    #[test]
    fn single_offline_commands_preserve_operation_and_paths() -> Result<()> {
        for (command, expected) in [
            ("backup-state", OfflineStateOperationV1::Backup),
            ("restore-state", OfflineStateOperationV1::Restore),
        ] {
            let parsed = parse(&[
                command,
                "--source",
                "/tmp/source",
                "--destination",
                "/tmp/dest",
            ])?;
            let offline = parsed
                .offline_operation()
                .ok_or_else(|| anyhow::anyhow!("offline command absent"))?;
            ensure!(offline.operation == expected);
            ensure!(offline.source_root == PathBuf::from("/tmp/source"));
            ensure!(offline.destination_root == Some(PathBuf::from("/tmp/dest")));
        }
        let verified = parse(&["verify-state", "--source", "/tmp/source"])?;
        ensure!(
            verified
                .offline_operation()
                .is_some_and(
                    |command| command.operation == OfflineStateOperationV1::Verify
                        && command.destination_root.is_none()
                )
        );
        Ok(())
    }

    #[test]
    fn duplicate_or_mixed_commands_refuse_before_dispatch() -> Result<()> {
        for args in [
            vec![
                "backup-state",
                "restore-state",
                "--source",
                "/tmp/source",
                "--destination",
                "/tmp/dest",
            ],
            vec![
                "restore-state",
                "backup-state",
                "--source",
                "/tmp/source",
                "--destination",
                "/tmp/dest",
            ],
            vec!["verify-state", "verify-state", "--source", "/tmp/source"],
            vec![
                "serve",
                "backup-state",
                "--source",
                "/tmp/source",
                "--destination",
                "/tmp/dest",
            ],
            vec![
                "backup-state",
                "serve",
                "--source",
                "/tmp/source",
                "--destination",
                "/tmp/dest",
            ],
            vec!["serve", "serve"],
            vec![
                "serve-native",
                "serve-native",
                "--config",
                "/tmp/native.json",
            ],
            vec!["serve", "serve-native", "--config", "/tmp/native.json"],
        ] {
            ensure!(
                parse(&args).is_err(),
                "ambiguous command accepted: {args:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn duplicate_route_paths_refuse_instead_of_overwriting() -> Result<()> {
        for args in [
            vec!["serve", "--state-root", "/tmp/a", "--state-root", "/tmp/b"],
            vec![
                "serve",
                "--diagnostic-trace",
                "/tmp/a",
                "--diagnostic-trace",
                "/tmp/b",
            ],
            vec!["verify-state", "--source", "/tmp/a", "--source", "/tmp/b"],
            vec![
                "backup-state",
                "--source",
                "/tmp/source",
                "--destination",
                "/tmp/a",
                "--destination",
                "/tmp/b",
            ],
            vec!["serve-native", "--config", "/tmp/a", "--config", "/tmp/b"],
        ] {
            ensure!(parse(&args).is_err(), "duplicate route accepted: {args:?}");
        }
        Ok(())
    }

    #[test]
    fn selected_native_launch_and_verification_keep_typed_identity() -> Result<()> {
        let serve_args = identity_args("serve-native")?;
        let serve = SearchdCommand::from_args(serve_args)?;
        let command = serve
            .native_command()
            .ok_or_else(|| anyhow::anyhow!("native command absent"))?;
        ensure!(command.mode == NativeCommandModeV1::Serve);
        ensure!(command.identity.is_some());
        ensure!(command.config_digest.is_some());
        ensure!(command.install_catalog_digest.is_none());
        ensure!(command.select_operation.is_none());

        let mut verify_args = identity_args("verify-native-state")?;
        verify_args.extend(["--catalog-content-digest".into(), "55".repeat(32)]);
        let verify = SearchdCommand::from_args(verify_args)?;
        let verified = verify
            .native_command()
            .ok_or_else(|| anyhow::anyhow!("verification command absent"))?;
        ensure!(verified.mode == NativeCommandModeV1::Verify);
        ensure!(verified.identity == command.identity);
        ensure!(
            verified.install_catalog_digest
                == Some(quanta_index_contract::index::Digest([0x55; 32]))
        );
        ensure!(verified.config_digest == command.config_digest);
        ensure!(verify.native_config_path().is_none());
        ensure!(verify.into_config().is_err());
        Ok(())
    }

    #[test]
    fn native_selection_has_only_its_bounded_inputs() -> Result<()> {
        let operation = operation_json()?;
        let args = vec![
            "select-native-state".into(),
            "--config".into(),
            "/tmp/native.json".into(),
            "--manifest-operation".into(),
            operation,
            "--receipt".into(),
            "/tmp/receipt.json".into(),
            "--config-digest".into(),
            "44".repeat(32),
        ];
        let selected = SearchdCommand::from_args(args)?;
        let command = selected
            .native_command()
            .ok_or_else(|| anyhow::anyhow!("selection command absent"))?;
        ensure!(command.mode == NativeCommandModeV1::Select);
        ensure!(command.identity.is_none());
        ensure!(command.select_operation.is_some());
        ensure!(command.receipt_path.as_ref() == Some(&PathBuf::from("/tmp/receipt.json")));
        ensure!(command.config_digest.is_some());
        ensure!(selected.native_config_path().is_none());
        Ok(())
    }

    #[test]
    fn public_native_command_fields_cannot_bypass_mode_contract() -> Result<()> {
        let selected_serve = SearchdCommand::from_args(identity_args("serve-native")?)?
            .native_command()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("selected Serve fixture missing"))?;
        let mut verify_args = identity_args("verify-native-state")?;
        verify_args.extend(["--catalog-content-digest".into(), "55".repeat(32)]);
        let verify = SearchdCommand::from_args(verify_args)?
            .native_command()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Verify fixture missing"))?;
        let select = SearchdCommand::from_args(vec![
            "select-native-state".into(),
            "--config".into(),
            "/tmp/native.json".into(),
            "--manifest-operation".into(),
            operation_json()?,
            "--receipt".into(),
            "/tmp/receipt.json".into(),
        ])?
        .native_command()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Select fixture missing"))?;
        selected_serve.validate()?;
        verify.validate()?;
        select.validate()?;

        let mut invalid = Vec::new();
        let mut command = selected_serve.clone();
        command.identity = None;
        invalid.push(command);
        let mut command = selected_serve.clone();
        command.config_digest = None;
        invalid.push(command);
        let mut command = selected_serve.clone();
        command.install_catalog_digest = verify.install_catalog_digest;
        invalid.push(command);
        let mut command = selected_serve.clone();
        command.receipt_path = select.receipt_path.clone();
        invalid.push(command);
        let mut command = selected_serve.clone();
        command.select_operation = select.select_operation;
        invalid.push(command);
        let mut command = selected_serve.clone();
        command
            .identity
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("identity missing"))?
            .root_nonce = [0; 32];
        invalid.push(command);
        let mut command = selected_serve.clone();
        command.config_path = PathBuf::from("relative.json");
        invalid.push(command);

        let mut command = verify.clone();
        command.identity = None;
        invalid.push(command);
        let mut command = verify.clone();
        command.config_digest = None;
        invalid.push(command);
        let mut command = verify.clone();
        command.install_catalog_digest = None;
        invalid.push(command);
        let mut command = verify.clone();
        command.receipt_path = select.receipt_path.clone();
        invalid.push(command);
        let mut command = verify;
        command.select_operation = select.select_operation;
        invalid.push(command);

        let mut command = select.clone();
        command.select_operation = None;
        invalid.push(command);
        let mut command = select.clone();
        command.receipt_path = None;
        invalid.push(command);
        let mut command = select.clone();
        command.receipt_path = Some(PathBuf::from("relative.json"));
        invalid.push(command);
        let mut command = select.clone();
        command.identity = selected_serve.identity;
        invalid.push(command);
        let mut command = select;
        command.install_catalog_digest = selected_serve.config_digest;
        invalid.push(command);

        let mut plain = selected_serve;
        plain.identity = None;
        plain.config_digest = None;
        plain.validate()?;
        plain.config_digest = Some(quanta_index_contract::index::Digest([4; 32]));
        invalid.push(plain);
        for command in invalid {
            ensure!(
                command.validate().is_err(),
                "invalid public Native command accepted: {command:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn incomplete_duplicate_or_mixed_native_inputs_refuse_before_root_access() -> Result<()> {
        let complete = identity_args("serve-native")?;
        let mut invalid = Vec::new();
        for missing in [4, 6, 8, 10, 12] {
            let mut args = complete.clone();
            drop(args.drain(missing - 1..=missing));
            invalid.push(args);
        }
        let mut args = complete.clone();
        args.extend(["--config-digest".into(), "44".repeat(32)]);
        invalid.push(args);
        let mut args = complete.clone();
        args.extend(["--catalog-content-digest".into(), "55".repeat(32)]);
        invalid.push(args);
        let mut args = complete.clone();
        args.extend(["--state-root".into(), "/tmp/root".into()]);
        invalid.push(args);
        let mut args = complete.clone();
        args.extend(["verify-state".into(), "--source".into(), "/tmp/root".into()]);
        invalid.push(args);
        let mut args = complete.clone();
        *identity_fixture_arg_mut(&mut args, 4)? = "00".repeat(32);
        invalid.push(args);
        let mut args = complete.clone();
        *identity_fixture_arg_mut(&mut args, 8)? = "a".repeat(63);
        invalid.push(args);
        let mut args = complete.clone();
        *identity_fixture_arg_mut(&mut args, 12)? = "A".repeat(64);
        invalid.push(args);
        let mut args = complete.clone();
        *identity_fixture_arg_mut(&mut args, 6)? = r#"{"workspace":{},"epoch":5}"#.into();
        invalid.push(args);
        let mut args = complete.clone();
        *identity_fixture_arg_mut(&mut args, 6)? = identity_fixture_arg(&complete, 6)?.replacen(
            "\"epoch\":5",
            "\"epoch\":5,\"epoch\":5",
            1,
        );
        invalid.push(args);
        let mut args = complete.clone();
        *identity_fixture_arg_mut(&mut args, 6)? = identity_fixture_arg(&complete, 6)?.replacen(
            "\"epoch\":5",
            "\"epoch\":5,\"extra\":0",
            1,
        );
        invalid.push(args);
        invalid.push(vec![
            "serve-native".into(),
            "--config".into(),
            "relative.json".into(),
        ]);
        invalid.push(vec![
            "serve-native".into(),
            "--config".into(),
            "/tmp/native.json".into(),
            "--config-digest".into(),
            "44".repeat(32),
        ]);
        invalid.push(vec![
            "verify-native-state".into(),
            "--config".into(),
            "/tmp/native.json".into(),
        ]);
        invalid.push(vec![
            "select-native-state".into(),
            "--config".into(),
            "/tmp/native.json".into(),
            "--manifest-operation".into(),
            operation_json()?,
            "--receipt".into(),
            "relative.json".into(),
        ]);
        invalid.push(vec![
            "select-native-state".into(),
            "--config".into(),
            "/tmp/native.json".into(),
            "--manifest-operation".into(),
            operation_json()?,
            "--receipt".into(),
            "/tmp/receipt.json".into(),
            "--root-nonce".into(),
            "11".repeat(32),
        ]);
        for args in invalid {
            ensure!(
                SearchdCommand::from_args(args.clone()).is_err(),
                "invalid native input accepted: {args:?}"
            );
        }
        Ok(())
    }
}
