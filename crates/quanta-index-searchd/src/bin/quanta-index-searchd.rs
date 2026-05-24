use anyhow::Result;
use quanta_index_searchd::{SearchdCommand, run};

fn main() -> Result<()> {
    let command = SearchdCommand::from_env()?;
    run(command)
}
