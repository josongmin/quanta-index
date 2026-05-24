//! Daemon entry. Builds the runtime, starts the UDS server thread, drives
//! the channel dispatcher in the current thread, and joins on shutdown.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;

use crate::app::runtime::SearchdRuntime;
use crate::cli::SearchdCommand;

const DEFAULT_ACCEPT_IDLE: Duration = Duration::from_millis(50);

/// Build the runtime and run the daemon. Blocks until shutdown is signaled.
pub fn run(command: SearchdCommand) -> Result<()> {
    let config = command.into_config()?;
    let runtime = SearchdRuntime::build(config)?;
    let shutdown = Arc::new(AtomicBool::new(false));
    drive(runtime, shutdown)
}

/// Run a fully-assembled runtime with an externally-driven shutdown flag.
pub fn drive(runtime: SearchdRuntime, shutdown: Arc<AtomicBool>) -> Result<()> {
    let SearchdRuntime {
        config: _,
        mut dispatcher,
        query_server,
    } = runtime;
    let server_shutdown = query_server.shutdown_handle();
    let join = query_server.spawn(DEFAULT_ACCEPT_IDLE)?;

    let shutdown_for_loop = Arc::clone(&shutdown);
    let loop_result = dispatcher.run_until(move || shutdown_for_loop.load(Ordering::Acquire));

    server_shutdown.trigger();
    let server_join_result = match join.join() {
        Ok(inner) => inner.map_err(anyhow::Error::from),
        Err(panic) => Err(anyhow::anyhow!("uds thread panicked: {panic:?}")),
    };
    loop_result?;
    server_join_result?;
    Ok(())
}
