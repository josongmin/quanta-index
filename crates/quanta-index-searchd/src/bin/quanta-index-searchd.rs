#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

use anyhow::Result;
use quanta_index_searchd::{cli::SearchdCommand, run};

fn main() -> Result<()> {
    run(SearchdCommand::from_env()?)
}
