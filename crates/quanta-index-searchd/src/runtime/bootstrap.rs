use anyhow::{Result, anyhow};
use quanta_index_control_sqlite::SqliteControlPlane;

use super::SearchRuntime;
use crate::app::SearchdConfig;

impl SearchRuntime {
    pub fn bootstrap(config: SearchdConfig) -> Result<Self> {
        let control_plane = SqliteControlPlane::open(&config.control_plane_path)
            .map_err(|error| anyhow!("control-plane bootstrap failed: {error}"))?;
        Ok(Self::new(config, control_plane))
    }
}
