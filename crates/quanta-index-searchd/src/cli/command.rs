use std::path::PathBuf;

use anyhow::Result;

use crate::app::config::SearchdConfig;

/// CLI subcommand surface. Today: `serve [--state-root PATH]`.
///
/// `--state-root` replaces only how the state root is resolved; every
/// policy family still comes from env through the one config chain
/// ([`SearchdConfig::from_env_with_state_root`]), so an explicit root never
/// silently drops a socket-access, envelope or admission knob (QI-BB-014,
/// QI-BB-016).
#[derive(Clone, Debug)]
pub struct SearchdCommand {
    state_root_override: Option<PathBuf>,
}

impl SearchdCommand {
    pub fn from_env() -> Result<Self> {
        let mut state_root_override: Option<PathBuf> = None;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "serve" => {}
                "--state-root" => {
                    let next = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--state-root requires a path"))?;
                    state_root_override = Some(PathBuf::from(next));
                }
                other => return Err(anyhow::anyhow!("unknown argument: {other}")),
            }
        }
        Ok(Self {
            state_root_override,
        })
    }

    pub fn into_config(self) -> Result<SearchdConfig> {
        self.state_root_override.map_or_else(
            SearchdConfig::from_env,
            SearchdConfig::from_env_with_state_root,
        )
    }
}
