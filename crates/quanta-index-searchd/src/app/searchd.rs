//! Daemon entry. Builds the runtime, starts the UDS server thread, drives
//! the channel dispatcher in the current thread, and joins on shutdown.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;

use crate::app::runtime::SearchdRuntime;

const DEFAULT_ACCEPT_IDLE: Duration = Duration::from_millis(50);

/// Run a fully-assembled runtime with an externally-driven shutdown flag.
#[expect(
    clippy::needless_pass_by_value,
    reason = "drive keeps an owned shutdown handle and clones it into the dispatcher loop"
)]
pub fn drive(runtime: SearchdRuntime, shutdown: Arc<AtomicBool>) -> Result<()> {
    let SearchdRuntime {
        mut dispatcher,
        query_server,
        control_server,
        ..
    } = runtime;
    let query_shutdown = query_server.shutdown_handle();
    let control_shutdown = control_server.shutdown_handle();
    let query_join = query_server.spawn(DEFAULT_ACCEPT_IDLE)?;
    let control_join = control_server.spawn(DEFAULT_ACCEPT_IDLE)?;

    let shutdown_for_loop = Arc::clone(&shutdown);
    let loop_result = dispatcher.run_until(move || shutdown_for_loop.load(Ordering::Acquire));

    query_shutdown.trigger();
    control_shutdown.trigger();
    let query_join_result = match query_join.join() {
        Ok(inner) => inner.map_err(anyhow::Error::from),
        Err(panic) => Err(anyhow::anyhow!("query uds thread panicked: {panic:?}")),
    };
    let control_join_result = match control_join.join() {
        Ok(inner) => inner.map_err(anyhow::Error::from),
        Err(panic) => Err(anyhow::anyhow!("control uds thread panicked: {panic:?}")),
    };
    loop_result?;
    query_join_result?;
    control_join_result?;
    Ok(())
}
