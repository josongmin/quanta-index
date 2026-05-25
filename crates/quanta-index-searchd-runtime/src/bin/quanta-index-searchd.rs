#![expect(
    clippy::multiple_crate_versions,
    reason = "transitive duplicates from tantivy + tokio; tracked at workspace cargo graph level"
)]

use anyhow::Result;
use quanta_index_searchd::SearchdCommand;
use quanta_index_searchd_runtime::run;

fn main() -> Result<()> {
    let command = SearchdCommand::from_env()?;
    run(command)
}
