#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]
#![expect(
    clippy::multiple_crate_versions,
    reason = "binary inherits the lib's transitive dep graph (lance + tantivy + rusqlite + tokio); cargo-deny skip-tree owns the supply-chain verdict"
)]

use anyhow::Result;
use quanta_index_searchd::{cli::SearchdCommand, run};

fn main() -> Result<()> {
    run(SearchdCommand::from_env()?)
}
