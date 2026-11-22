//! One executable and an explicit environment for each Legacy crash scenario.

#![forbid(unsafe_code)]

use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, SearchdBinaryProcess};
use std::error::Error;
use std::path::Path;

pub(super) struct CrashDaemon {
    reference: LegacySearchdBinary,
}

impl CrashDaemon {
    pub(super) fn new() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            reference: LegacySearchdBinary::from_environment()?,
        })
    }

    pub(super) fn start(
        &self,
        state_root: &Path,
        max_generations: usize,
        overrides: impl IntoIterator<Item = (&'static str, String)>,
    ) -> Result<SearchdBinaryProcess, Box<dyn Error>> {
        let mut command = self
            .reference
            .benchmark_command(state_root, max_generations)?;
        let _configured = command.envs(overrides);
        SearchdBinaryProcess::start_with_pinned_command(state_root, command)
    }
}
