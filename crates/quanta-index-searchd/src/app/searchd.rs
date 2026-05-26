//! Daemon entry. Builds the runtime, starts the UDS server threads, and joins
//! on shutdown.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;

use crate::app::runtime::SearchdRuntime;

const DEFAULT_ACCEPT_IDLE: Duration = Duration::from_millis(50);

/// Run a fully-assembled runtime with an externally-driven shutdown flag.
pub fn drive(runtime: SearchdRuntime, shutdown: &Arc<AtomicBool>) -> Result<()> {
    let SearchdRuntime {
        query_server,
        control_server,
        ingest_server,
        ..
    } = runtime;
    let query_shutdown = query_server.shutdown_handle();
    let control_shutdown = control_server.shutdown_handle();
    let ingest_shutdown = ingest_server.shutdown_handle();
    let query_join = query_server.spawn(DEFAULT_ACCEPT_IDLE)?;
    let control_join = control_server.spawn(DEFAULT_ACCEPT_IDLE)?;
    // QI-RT-01: ingest server runs alongside query / control.
    let ingest_join = ingest_server.spawn(DEFAULT_ACCEPT_IDLE)?;

    while !shutdown.load(Ordering::Acquire) {
        std::thread::sleep(DEFAULT_ACCEPT_IDLE);
    }

    query_shutdown.trigger();
    control_shutdown.trigger();
    ingest_shutdown.trigger();
    let query_join_result = match query_join.join() {
        Ok(inner) => inner.map_err(anyhow::Error::from),
        Err(panic) => Err(anyhow::anyhow!("query uds thread panicked: {panic:?}")),
    };
    let control_join_result = match control_join.join() {
        Ok(inner) => inner.map_err(anyhow::Error::from),
        Err(panic) => Err(anyhow::anyhow!("control uds thread panicked: {panic:?}")),
    };
    let ingest_join_result = match ingest_join.join() {
        Ok(inner) => inner.map_err(anyhow::Error::from),
        Err(panic) => Err(anyhow::anyhow!("ingest uds thread panicked: {panic:?}")),
    };
    query_join_result?;
    control_join_result?;
    ingest_join_result?;
    Ok(())
}
