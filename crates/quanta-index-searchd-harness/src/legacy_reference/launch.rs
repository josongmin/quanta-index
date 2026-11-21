//! Joins pinned executable custody with the process lifecycle owner.

use std::error::Error;
use std::path::Path;

use super::binary::LegacySearchdBinary;
use super::process::SearchdBinaryProcess;

impl LegacySearchdBinary {
    pub fn start(
        &self,
        state_root: &Path,
        history_max_generations: usize,
    ) -> Result<SearchdBinaryProcess, Box<dyn Error>> {
        let mut command = self.command()?;
        let _configured = command.arg("serve").arg("--state-root").arg(state_root);
        let _configured = command.apply_searchd_env(history_max_generations);
        let process = SearchdBinaryProcess::start_with_pinned_command(state_root, command)?;
        self.verify_launch_custody()?;
        Ok(process)
    }
}
