use std::path::PathBuf;

use anyhow::Result;

use crate::app::config::SearchdConfig;
use crate::app::state_migration::{OfflineStateCommandV1, OfflineStateOperationV1};

/// CLI subcommand surface: `serve [--state-root PATH]` plus the offline
/// state commands `migrate-state`, `backup-state`, `restore-state` and
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
    offline: Option<OfflineStateCommandV1>,
}

impl SearchdCommand {
    pub fn from_env() -> Result<Self> {
        let mut state_root_override: Option<PathBuf> = None;
        let mut source_root: Option<PathBuf> = None;
        let mut destination_root: Option<PathBuf> = None;
        let mut operation: Option<OfflineStateOperationV1> = None;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "serve" => {}
                "migrate-state" => operation = Some(OfflineStateOperationV1::Migrate),
                "backup-state" => operation = Some(OfflineStateOperationV1::Backup),
                "restore-state" => operation = Some(OfflineStateOperationV1::Restore),
                "verify-state" => operation = Some(OfflineStateOperationV1::Verify),
                "--state-root" => {
                    let next = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--state-root requires a path"))?;
                    state_root_override = Some(PathBuf::from(next));
                }
                "--source" => {
                    let next = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--source requires a path"))?;
                    source_root = Some(PathBuf::from(next));
                }
                "--destination" => {
                    let next = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--destination requires a path"))?;
                    destination_root = Some(PathBuf::from(next));
                }
                other => return Err(anyhow::anyhow!("unknown argument: {other}")),
            }
        }
        let offline = match operation {
            None => {
                if source_root.is_some() || destination_root.is_some() {
                    return Err(anyhow::anyhow!(
                        "--source/--destination require one of: migrate-state, backup-state, restore-state, verify-state"
                    ));
                }
                None
            }
            Some(operation) => {
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
        Ok(Self {
            state_root_override,
            offline,
        })
    }

    /// The offline state command this invocation names, if any.
    #[must_use]
    pub fn offline_operation(&self) -> Option<&OfflineStateCommandV1> {
        self.offline.as_ref()
    }

    pub fn into_config(self) -> Result<SearchdConfig> {
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
