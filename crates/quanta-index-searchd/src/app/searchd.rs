//! Composition root for `searchd serve`.
//!
//! Wires the bootstrapped runtime (control plane + lexical/semantic adapters)
//! into a `DomainQueryEngine` and starts the UDS listener on the configured
//! socket path. SIGINT and SIGTERM signal the listener to drain in-flight
//! connections and shut down cleanly.
//!
//! Workspace lint annotations:
//! - `clippy::disallowed_methods` is locally `expect`-ed for the one site
//!   that calls `Runtime::block_on` — the workspace rule targets accidental
//!   sync-over-async at application boundaries, which is exactly what the
//!   composition root *must* do once.
//! - `clippy::print_stdout` / `clippy::print_stderr` are `expect`-ed for the
//!   two startup / shutdown lines. Operators consume these on the terminal;
//!   we don't have a tracing subscriber wired yet (Phase 4).

use std::io;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::runtime::Builder;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::app::UdsListener;
use crate::cli::{SearchdCommand, SearchdReporter, ServeOptions};
use crate::query::DomainQueryEngine;
use crate::runtime::SearchRuntime;

use super::SearchdConfig;

/// Default lexical/semantic top-K cap when the request omits a limit.
const DEFAULT_TOP_K: usize = 25;

pub fn run(command: SearchdCommand) -> Result<()> {
    match command {
        SearchdCommand::Serve(options) => serve(&options),
    }
}

/// Bootstrap the runtime and serve queries over UDS until SIGINT/SIGTERM
/// triggers shutdown.
fn serve(options: &ServeOptions) -> Result<()> {
    let config = SearchdConfig::from_env().with_overrides(options);
    let runtime = SearchRuntime::bootstrap(config)?;
    let mut stdout = io::stdout().lock();
    SearchdReporter::new().write_bootstrap(&mut stdout, runtime.config())?;
    // Drop the stdout lock before tokio takes over so the runtime can write
    // its own startup confirmation line below.
    drop(stdout);

    let tokio = Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| anyhow!("build tokio runtime: {error}"))?;
    #[expect(
        clippy::disallowed_methods,
        reason = "composition root drives the async runtime exactly once at process start"
    )]
    let outcome = tokio.block_on(async move { serve_async(runtime).await });
    outcome
}

async fn serve_async(runtime: SearchRuntime) -> Result<()> {
    let engine = DomainQueryEngine {
        lexical: runtime.lexical(),
        semantic: runtime.semantic(),
        control: runtime.control(),
        embedder: None,
        default_top_k: DEFAULT_TOP_K,
    };
    let socket_path = runtime.config().socket_path.clone();
    let listener = UdsListener::bind(&socket_path).await?;
    let shutdown = listener.shutdown_trigger();
    let signals = install_signal_handlers(&shutdown);
    print_listening(&socket_path);
    let outcome = listener.serve(Arc::new(engine)).await;
    // Stop signal listener once serve loop has unwound.
    drop(signals);
    outcome
}

/// Install SIGINT + SIGTERM handlers that fire the listener's shutdown notify.
fn install_signal_handlers(shutdown: &Arc<Notify>) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::with_capacity(2);
    for (kind, label) in [
        (SignalKind::interrupt(), "SIGINT"),
        (SignalKind::terminate(), "SIGTERM"),
    ] {
        let shutdown = Arc::clone(shutdown);
        match signal(kind) {
            Ok(mut sig) => {
                handles.push(tokio::spawn(async move {
                    if sig.recv().await.is_some() {
                        eprintln_shutdown(label);
                        shutdown.notify_waiters();
                    }
                }));
            }
            Err(error) => {
                eprintln_handler_failure(label, &error);
            }
        }
    }
    handles
}

#[expect(
    clippy::print_stdout,
    reason = "operator-facing startup line; tracing subscriber wiring is a Phase 4 follow-up"
)]
fn print_listening(socket_path: &std::path::Path) {
    println!("searchd: listening on {}", socket_path.display());
}

#[expect(
    clippy::print_stderr,
    reason = "operator-facing shutdown notice; tracing subscriber wiring is a Phase 4 follow-up"
)]
fn eprintln_shutdown(label: &str) {
    eprintln!("searchd: received {label}, draining…");
}

#[expect(
    clippy::print_stderr,
    reason = "operator-facing handler-install failure; tracing subscriber wiring is a Phase 4 follow-up"
)]
fn eprintln_handler_failure(label: &str, error: &std::io::Error) {
    eprintln!("searchd: install {label} handler failed: {error}");
}
