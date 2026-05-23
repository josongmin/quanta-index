use anyhow::Result;
use std::io;

use crate::cli::{SearchdCommand, SearchdReporter};
use crate::runtime::SearchRuntime;

use super::SearchdConfig;

pub fn run(command: SearchdCommand) -> Result<()> {
    match command {
        SearchdCommand::Serve => serve(),
    }
}

fn serve() -> Result<()> {
    let runtime = SearchRuntime::bootstrap(SearchdConfig::from_env())?;
    let mut stdout = io::stdout().lock();
    SearchdReporter::new().write_bootstrap(&mut stdout, runtime.config())?;
    Ok(())
}
