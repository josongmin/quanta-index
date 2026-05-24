use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use quanta_index_control::ControlPlane;
use quanta_index_lexical::TantivyLexicalAdapter;
use quanta_index_semantic::LanceSemanticAdapter;

use crate::app::SearchdConfig;

/// Fully assembled runtime: control plane, lexical and semantic adapters
/// wrapped for safe sharing across the tokio runtime that the UDS listener
/// spawns connection tasks on.
pub struct SearchRuntime {
    config: SearchdConfig,
    pub(crate) control: Arc<Mutex<ControlPlane>>,
    pub(crate) lexical: Arc<TantivyLexicalAdapter>,
    pub(crate) semantic: Arc<LanceSemanticAdapter>,
    pub(crate) bundle_root: PathBuf,
}

impl SearchRuntime {
    #[must_use]
    pub fn new(
        config: SearchdConfig,
        control: ControlPlane,
        lexical: TantivyLexicalAdapter,
        semantic: LanceSemanticAdapter,
        bundle_root: PathBuf,
    ) -> Self {
        Self {
            config,
            control: Arc::new(Mutex::new(control)),
            lexical: Arc::new(lexical),
            semantic: Arc::new(semantic),
            bundle_root,
        }
    }

    #[must_use]
    pub const fn config(&self) -> &SearchdConfig {
        &self.config
    }

    #[must_use]
    pub fn control(&self) -> Arc<Mutex<ControlPlane>> {
        Arc::clone(&self.control)
    }

    #[must_use]
    pub fn lexical(&self) -> Arc<TantivyLexicalAdapter> {
        Arc::clone(&self.lexical)
    }

    #[must_use]
    pub fn semantic(&self) -> Arc<LanceSemanticAdapter> {
        Arc::clone(&self.semantic)
    }

    #[must_use]
    pub fn bundle_root(&self) -> &std::path::Path {
        &self.bundle_root
    }
}
