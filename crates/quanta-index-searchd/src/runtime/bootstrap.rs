use anyhow::{Result, anyhow};
use quanta_index_control::ControlPlane;

use super::SearchRuntime;
use crate::app::SearchdConfig;

impl SearchRuntime {
    pub fn bootstrap(config: SearchdConfig) -> Result<Self> {
        let control_plane = ControlPlane::open(&config.control_plane_path)
            .map_err(|error| anyhow!("control-plane bootstrap failed: {error}"))?;
        Ok(Self::new(config, control_plane))
    }
}
