use std::path::PathBuf;

use anyhow::Result;

use crate::app::config::SearchdConfig;

/// CLI subcommand surface. Today: `serve [--state-root PATH]`.
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
        if let Some(root) = self.state_root_override {
            return Ok(SearchdConfig::from_state_root(root));
        }
        SearchdConfig::from_env()
    }
}
