use quanta_index_control::ControlPlane;

use crate::app::SearchdConfig;

pub struct SearchRuntime {
    config: SearchdConfig,
    _control_plane: ControlPlane,
}

impl SearchRuntime {
    pub const fn new(config: SearchdConfig, control_plane: ControlPlane) -> Self {
        Self {
            config,
            _control_plane: control_plane,
        }
    }

    #[must_use]
    pub const fn config(&self) -> &SearchdConfig {
        &self.config
    }
}
