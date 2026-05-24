use anyhow::Result;
use std::io;

use crate::cli::{SearchdCommand, SearchdReporter, ServeOptions};
use crate::runtime::SearchRuntime;

use super::SearchdConfig;

pub fn run(command: SearchdCommand) -> Result<()> {
    match command {
        SearchdCommand::Serve(options) => serve(&options),
    }
}

fn serve(options: &ServeOptions) -> Result<()> {
    let config = SearchdConfig::from_env().with_overrides(options);
    let runtime = SearchRuntime::bootstrap(config)?;
    let mut stdout = io::stdout().lock();
    SearchdReporter::new().write_bootstrap(&mut stdout, runtime.config())?;
    Ok(())
}
