use anyhow::Result;
use quanta_index_searchd::SearchdCommand;
use quanta_index_searchd_runtime::run;

fn main() -> Result<()> {
    let command = SearchdCommand::from_env()?;
    run(command)
}
