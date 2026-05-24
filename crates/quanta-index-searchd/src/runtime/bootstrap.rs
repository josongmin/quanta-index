use anyhow::{Result, anyhow};
use quanta_index_control::ControlPlane;
use quanta_index_lexical::TantivyLexicalAdapter;
use quanta_index_semantic::LanceSemanticAdapter;

use super::SearchRuntime;
use crate::app::SearchdConfig;

impl SearchRuntime {
    /// Open or initialise the on-disk control plane and instantiate the
    /// vendor adapters against the configured state root. The bundle root
    /// defaults to `{state_root}/bundles/` — producer publishes there.
    pub fn bootstrap(config: SearchdConfig) -> Result<Self> {
        let control = ControlPlane::open(&config.control_plane_path)
            .map_err(|error| anyhow!("control-plane bootstrap failed: {error}"))?;
        let lexical = TantivyLexicalAdapter::with_state_root(config.state_root.clone());
        let semantic = LanceSemanticAdapter::with_state_root(config.state_root.clone());
        let bundle_root = config.state_root.join("bundles");
        Ok(Self::new(config, control, lexical, semantic, bundle_root))
    }
}
