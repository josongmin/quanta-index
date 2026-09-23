//! Daemon entry (SEP-21 P08 / S21-09): one `SearchdSupervisor` owns the
//! accept loops, the maintenance timer, the provider attempt drain, and
//! the runtime guards for the whole serving interval.
//!
//! The old partial destructure — which moved the three servers out of
//! `SearchdRuntime` and dropped the maintenance timer, corpus lifecycle
//! and state-root lease before serving began — is gone:
//! `into_servers_maintenance_guards_and_boot_notices` hands the
//! supervisor every piece, and the guards drop only after every child
//! has joined or been explicitly escalated.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;

use quanta_index_embed::ProviderAttemptPool;
use quanta_index_ipc::{IpcDispatcher, RequestEnvelope, ResponseEnvelope};

use crate::app::maintenance::MaintenanceTimer;
use crate::app::runtime::{RuntimeGuards, RuntimeServers, SearchdRuntime};
use crate::app::server::QueryServer;
use crate::app::supervisor::{
    CancelRoot, ChildContext, ChildExitKind, ChildSpawnFailure, DEFAULT_COOPERATIVE_DRAIN_DEADLINE,
    HARD_DRAIN_DEADLINE, SearchdSupervisor, SupervisionError, SupervisionOutcome,
};

// Query clients use one-shot UDS connections, so the accept-loop idle cadence
// is directly observable in warm p95. Keep query polling tight; control/ingest
// can stay looser because they are not on the steady-state query hot path.
const QUERY_ACCEPT_IDLE: Duration = Duration::from_millis(1);
const CONTROL_ACCEPT_IDLE: Duration = Duration::from_millis(5);
const INGEST_ACCEPT_IDLE: Duration = Duration::from_millis(5);
const SHUTDOWN_POLL_IDLE: Duration = Duration::from_millis(10);

/// Run a fully-assembled runtime with an externally-driven shutdown
/// flag.
///
/// Compatibility wrapper around the supervised lifecycle: the outcome
/// is derived the same way, a non-clean stop surfaces as a typed
/// [`SupervisionError`].
pub fn drive(runtime: SearchdRuntime, shutdown: &Arc<AtomicBool>) -> Result<()> {
    let root = CancelRoot::new();
    let bridge = bridge_external_shutdown(Arc::clone(shutdown), Arc::new(CancelRoot::clone(&root)))
        .map_err(anyhow::Error::from)?;
    let outcome = supervise_runtime(
        runtime,
        DEFAULT_COOPERATIVE_DRAIN_DEADLINE,
        HARD_DRAIN_DEADLINE,
        &root,
    );
    let _joined = bridge.join();
    if outcome.is_stopped_clean() {
        return Ok(());
    }
    Err(anyhow::Error::from(SupervisionError { outcome }))
}

/// Watch an external shutdown flag and latch it into a [`CancelRoot`].
/// The watcher exits as soon as it fires; it owns no resource.
fn bridge_external_shutdown(
    shutdown: Arc<AtomicBool>,
    root: Arc<CancelRoot>,
) -> Result<JoinHandle<()>, ChildSpawnFailure> {
    let bridged = std::thread::Builder::new()
        .name("searchd-shutdown-bridge".to_string())
        .spawn(move || {
            loop {
                if shutdown.load(Ordering::Acquire) {
                    root.request_shutdown();
                    return;
                }
                std::thread::sleep(SHUTDOWN_POLL_IDLE);
            }
        });
    bridged.map_err(|_refused| ChildSpawnFailure {
        name: "shutdown-bridge",
    })
}

/// Supervise one fully-assembled runtime: spawn every child, serve
/// until the cancellation root fires or a child is lost, then drain
/// under the two-deadline policy.
///
/// The runtime guards (corpus lifecycle, state-root lease) are held by
/// the supervisor until every child has joined or been explicitly
/// escalated.
#[must_use]
pub fn supervise_runtime(
    runtime: SearchdRuntime,
    cooperative_deadline: Duration,
    hard_deadline: Duration,
    root: &CancelRoot,
) -> SupervisionOutcome {
    // The supervised provider pool, when this composition has one: an
    // `openai` runtime drains its attempt threads as a supervised child
    // (S21-09); local-only compositions have nothing to drain.
    let attempt_pool = runtime.provider_attempt_pool.clone();
    let (servers, maintenance, guards, _boot_notices) =
        runtime.into_servers_maintenance_guards_and_boot_notices();
    let RuntimeServers {
        query: query_server,
        control: control_server,
        ingest: ingest_server,
    } = servers;
    let mut supervisor = SearchdSupervisor::new(
        cooperative_deadline,
        hard_deadline,
        guards,
        CancelRoot::clone(root),
    );

    // Startup is all-or-rollback: any spawn failure tears down exactly
    // what was started, in reverse order, before the guards drop.
    let mut startups = vec![
        spawn_accept_child(
            &mut supervisor,
            "query-accept",
            query_server,
            QUERY_ACCEPT_IDLE,
        ),
        spawn_accept_child(
            &mut supervisor,
            "control-accept",
            control_server,
            CONTROL_ACCEPT_IDLE,
        ),
        spawn_accept_child(
            &mut supervisor,
            "ingest-accept",
            ingest_server,
            INGEST_ACCEPT_IDLE,
        ),
        spawn_maintenance_child(&mut supervisor, maintenance),
    ];
    if let Some(pool) = attempt_pool {
        startups.push(spawn_provider_child(&mut supervisor, pool, hard_deadline));
    }
    for startup in startups {
        if let Err(failure) = startup {
            return supervisor.rollback(failure.name);
        }
    }
    supervisor.run(root)
}

/// Register one bound IPC server as a supervised child. The stop closure
/// triggers the server's own shutdown handle; the adapted body joins the
/// accept loop and reports its terminal kind.
fn spawn_accept_child<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>(
    supervisor: &mut SearchdSupervisor<RuntimeGuards>,
    name: &'static str,
    server: QueryServer<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>,
    accept_idle: Duration,
) -> Result<(), ChildSpawnFailure>
where
    RequestEnvelopeT: RequestEnvelope<Request>,
    ResponseEnvelopeT: ResponseEnvelope<Response>,
    D: IpcDispatcher<Request, Response> + ?Sized + 'static,
{
    let stop_handle = server.shutdown_handle();
    let stop_handle_for_error = stop_handle.clone();
    supervisor.spawn_child(
        name,
        Box::new(move || stop_handle.trigger()),
        move |context: ChildContext| {
            let inner = server.spawn(accept_idle)?;
            let adapter = std::thread::Builder::new()
                .name(format!("supervised-{name}"))
                .spawn(move || {
                    let kind = match inner.join() {
                        Ok(Ok(())) => ChildExitKind::Completed,
                        Ok(Err(_ipc)) => ChildExitKind::Failed,
                        Err(_panic) => ChildExitKind::Panicked,
                    };
                    context.report_exit(kind);
                });
            match adapter {
                Ok(handle) => Ok(handle),
                // The accept thread exists but the registry could not take
                // it: trigger the server's shutdown so the acceptor
                // self-terminates, and answer with the typed spawn failure
                // so startup rolls the siblings back.
                Err(error) => {
                    stop_handle_for_error.trigger();
                    Err(anyhow::Error::new(error))
                }
            }
        },
    )
}

/// Register the provider attempt drain as a supervised child (S21-09).
///
/// The stop closure shuts the pool down, the adapted body drains it
/// under the hard deadline and reports `Failed` when attempts are still
/// running — the receipt then names `provider-attempts` instead of
/// claiming a graceful shutdown it did not perform.
fn spawn_provider_child(
    supervisor: &mut SearchdSupervisor<RuntimeGuards>,
    pool: Arc<ProviderAttemptPool>,
    hard_deadline: Duration,
) -> Result<(), ChildSpawnFailure> {
    let pool_for_stop = Arc::clone(&pool);
    supervisor.spawn_child(
        "provider-attempts",
        Box::new(move || pool_for_stop.shutdown()),
        move |context: ChildContext| {
            let adapter = std::thread::Builder::new()
                .name("supervised-provider-drain".to_string())
                .spawn(move || {
                    // Park until the supervisor asks for shutdown, then
                    // drain: attempts keep running while serving.
                    while !context.shutdown().load(Ordering::Acquire) {
                        std::thread::sleep(SHUTDOWN_POLL_IDLE);
                    }
                    let report = pool.drain(hard_deadline);
                    let kind = if report.unfinished.is_empty() {
                        ChildExitKind::Completed
                    } else {
                        ChildExitKind::Failed
                    };
                    context.report_exit(kind);
                });
            adapter.map_err(anyhow::Error::new)
        },
    )
}

/// Register the maintenance timer as a supervised child (S21-09): the
/// stop closure sends the timer's stop signal, the adapted body joins
/// the timer thread and reports its terminal kind.
fn spawn_maintenance_child(
    supervisor: &mut SearchdSupervisor<RuntimeGuards>,
    maintenance: MaintenanceTimer,
) -> Result<(), ChildSpawnFailure> {
    let (stop, join) = maintenance
        .into_supervised_parts()
        .map_err(|_handed_over_twice| ChildSpawnFailure {
            name: "maintenance-timer",
        })?;
    supervisor.spawn_child(
        "maintenance-timer",
        Box::new(move || stop.stop()),
        move |context: ChildContext| {
            let adapter = std::thread::Builder::new()
                .name("supervised-maintenance".to_string())
                .spawn(move || {
                    let kind = if join.join().is_ok() {
                        ChildExitKind::Completed
                    } else {
                        ChildExitKind::Panicked
                    };
                    context.report_exit(kind);
                });
            adapter.map_err(anyhow::Error::new)
        },
    )
}
