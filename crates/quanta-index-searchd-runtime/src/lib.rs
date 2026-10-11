#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Concrete runtime wiring for the search-plane daemon.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::{
    AuxiliaryAuthorityCatalogPort, DiagnosticContextV1, DiagnosticHealthCodeV1, DiagnosticLaneV1,
    DiagnosticRootKindV1, DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1,
    DiagnosticTraceErrorV1, DoorFindingQuarantinePort, FileContributorIngestPort,
    FileOwnershipIngestPort, GenerationIdentityValidatePort, HistoryTextIndexPort,
    IdempotencyCatalogPort, IncompleteGenerationDiscardPort, IntegrityScrubPort,
    LexicalIndexOpenPort, MetricSourcePort, MutationCoordinatorPort, ProcessMemoryProbePort,
    QuarantinedGenerationDiscardPort, RepoCommitRecencyIngestPort, RepoDescriptionIngestPort,
    RepoMapBundleIngestPort, RepoMapGenerationActivatePort, RepoMapQuarantinePort,
    RepoMapSnapshotAcquirePort, RepoMetaIngestPort, RepoTopicIngestPort,
    ResidentMemoryWriterAdmission, SealedGenerationIdentityProbePort, SealedGenerationReclaimPort,
    SealedGenerationScanPort, SearchCorpusBatchBuildPort, SemanticContentRootsPort,
    SemanticIndexOpenPort, SemanticScopeStreamBuildPort, TrackDiskUsagePort,
    UnboundedWriterAdmission, WriterAdmissionPort, WriterIdleSweepPort,
};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_lexical::history_text_index::HistoryTextIndexAdapter;
use quanta_index_lexical::regex::RegexPolicy;
use quanta_index_repomap::RepoMapGenerationStore;
use quanta_index_search_plane::{
    PairIndexBytesMeasurer, SearchCorpusIndexBytesPort, SearchCorpusLifecycleOwner,
};
use quanta_index_searchd::app::KernelResidentMemoryProbe;
use quanta_index_searchd::app::runtime::{SearchdRuntimeParts, StateRootAccessV1, StateRootLease};
use quanta_index_searchd::app::state_format::refuse_legacy_state_root_v1;
use quanta_index_searchd::{
    DEFAULT_COOPERATIVE_DRAIN_DEADLINE, DiagnosticCaptureLeaseV1, DiagnosticCaptureLimitsV1,
    DiagnosticCaptureV1, HARD_DRAIN_DEADLINE, SearchdCommand, SearchdConfig, SearchdRuntime,
    supervise_runtime,
};
use quanta_index_semantic::SemanticAdapter;

pub mod native;
mod native_worker;
/// The process signal root.
///
/// SIGINT/SIGTERM are latched into [`quanta_index_searchd::CancelRoot`].
pub mod signal;

/// Offline `backup-state` / `restore-state` /
/// `verify-state` composition (SEP-21 P10 / S21-11).
///
/// The only surface that links the legacy parsers.
pub mod state_migration;

/// How long a catalog write waits on a held lock before answering typed.
const CATALOG_BUSY_BUDGET: Duration = Duration::from_secs(2);

fn runtime_diagnostic_span(
    context: &DiagnosticContextV1,
    result: std::result::Result<DiagnosticSpanGuardV1, DiagnosticTraceErrorV1>,
) -> Option<DiagnosticSpanGuardV1> {
    result.map_or_else(
        |error| {
            context.health(match error {
                DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                    DiagnosticHealthCodeV1::CounterOverflow
                }
                DiagnosticTraceErrorV1::ClockRegressed => DiagnosticHealthCodeV1::ClockRegressed,
                DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                    DiagnosticHealthCodeV1::ForeignCaptureSpan
                }
                DiagnosticTraceErrorV1::MissingParentSpan => {
                    DiagnosticHealthCodeV1::MissingParentSpan
                }
            });
            None
        },
        Some,
    )
}

/// Build the runtime over the kernel's own resident-memory accounting.
pub fn build_runtime(config: SearchdConfig) -> Result<SearchdRuntime> {
    build_runtime_with_memory_probe(config, Arc::new(KernelResidentMemoryProbe))
}

/// Build over a capture created before state-root lease and adapter opens.
pub fn build_runtime_with_capture(
    config: SearchdConfig,
    capture: DiagnosticCaptureLeaseV1,
) -> Result<SearchdRuntime> {
    build_runtime_with_memory_probe_and_capture(
        config,
        Arc::new(KernelResidentMemoryProbe),
        capture,
    )
}

/// Harness seam: preserve the configured memory probe while tracing boot.
pub fn build_runtime_with_memory_probe_and_capture(
    config: SearchdConfig,
    memory_probe: Arc<dyn ProcessMemoryProbePort>,
    capture: DiagnosticCaptureLeaseV1,
) -> Result<SearchdRuntime> {
    build_runtime_with_memory_probe_and_capture_under(config, memory_probe, capture, None)
}

/// Build under an explicit capture parent, or a standalone operation root.
/// A foreign parent is refused before state-root lease and adapter opens.
pub fn build_runtime_with_memory_probe_and_capture_under(
    config: SearchdConfig,
    memory_probe: Arc<dyn ProcessMemoryProbePort>,
    capture: DiagnosticCaptureLeaseV1,
    parent: Option<&DiagnosticContextV1>,
) -> Result<SearchdRuntime> {
    if parent.is_some_and(|parent| !capture.context().shares_capture_session(parent)) {
        return Err(anyhow::anyhow!(
            "runtime boot diagnostic parent belongs to a different capture session"
        ));
    }
    let boot = parent
        .is_none()
        .then(|| {
            runtime_diagnostic_span(
                capture.context(),
                capture
                    .context()
                    .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0)),
            )
        })
        .flatten();
    let boot_span = parent.map_or_else(
        || {
            boot.as_ref().and_then(|root| {
                let context = capture.context().child_context(root);
                runtime_diagnostic_span(
                    &context,
                    context.enter(DiagnosticStageV1::RuntimeBoot, DiagnosticLaneV1(0)),
                )
            })
        },
        |parent| {
            runtime_diagnostic_span(
                parent,
                parent.enter(DiagnosticStageV1::RuntimeBoot, DiagnosticLaneV1(0)),
            )
        },
    );
    let build_capture = boot_span.as_ref().and_then(|span| capture.child_of(span));
    let result = build_runtime_with_parts(
        config,
        memory_probe,
        |builder| builder,
        move |parts| {
            parts.diagnostic_capture = build_capture;
        },
    );
    let outcome = DiagnosticTerminalV1::from_result(&result);
    // Clock regression marks the capture unhealthy in the core trace owner.
    // Keep the runtime build's result authoritative after it may have opened
    // live adapters and server threads.
    if let Some(span) = boot_span {
        let _span_finish = span.finish(outcome);
    }
    if let Some(root) = boot {
        let _root_finish = root.finish(outcome);
    }
    // Retain the producer lease until boot work and both terminal events finish.
    drop(capture);
    result
}

/// Build the runtime with an explicit resident-memory probe (QI-BB-016):
/// the kernel's in production, a scripted one where a test needs to drive
/// the writer gate.
pub fn build_runtime_with_memory_probe(
    config: SearchdConfig,
    memory_probe: Arc<dyn ProcessMemoryProbePort>,
) -> Result<SearchdRuntime> {
    build_runtime_with_lexical_builder(config, memory_probe, |builder| builder)
}

// Keep the production composition unchanged. A crate-local test can wrap the
// real lexical build port to hold one admitted UDS publish at a precise point;
// this is not a daemon option, environment switch, or exported API.
fn build_runtime_with_lexical_builder(
    config: SearchdConfig,
    memory_probe: Arc<dyn ProcessMemoryProbePort>,
    wrap_lexical_builder: impl FnOnce(
        Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    ) -> Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
) -> Result<SearchdRuntime> {
    build_runtime_with_parts(config, memory_probe, wrap_lexical_builder, |_| {})
}

// Compose the same production ports before assembly. Crate-local process
// proofs may wrap an existing port while retaining the real state root,
// catalog, adapters, supervisor and IPC servers. Production passes no-op.
fn build_runtime_with_parts(
    config: SearchdConfig,
    memory_probe: Arc<dyn ProcessMemoryProbePort>,
    wrap_lexical_builder: impl FnOnce(
        Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    ) -> Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    wrap_parts: impl FnOnce(&mut SearchdRuntimeParts),
) -> Result<SearchdRuntime> {
    build_runtime_with_assembly(
        config,
        memory_probe,
        wrap_lexical_builder,
        wrap_parts,
        SearchdRuntime::assemble,
    )
}

fn build_runtime_with_assembly(
    config: SearchdConfig,
    memory_probe: Arc<dyn ProcessMemoryProbePort>,
    wrap_lexical_builder: impl FnOnce(
        Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    ) -> Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    wrap_parts: impl FnOnce(&mut SearchdRuntimeParts),
    assemble: impl FnOnce(SearchdConfig, SearchdRuntimeParts) -> Result<SearchdRuntime>,
) -> Result<SearchdRuntime> {
    let search_corpus_history_retention = config.search_corpus_history_retention_policy()?;
    // The one envelope (QI-BB-016) is validated before any adapter exists,
    // and its resident-memory ceiling becomes the lexical writer gate.
    let process_memory_envelope = config.process_memory_envelope()?;
    let configured_state_root = config.state_root().to_path_buf();
    // A known legacy source is refused before even the persistent lease
    // file is created. The second check below closes path changes before
    // adapters or catalog schema can write to the leased identity.
    refuse_legacy_state_root_v1(&configured_state_root)?;
    // Acquire process ownership before any adapter or authority store opens the
    // shared root. No loser may observe or mutate partially initialized state.
    // The root must be exactly private unless a socket is shared, in which
    // case its traverse bits may be open for the admitted peers (QI-BB-014).
    let state_root_lease = StateRootLease::acquire_with_access(
        &configured_state_root,
        StateRootAccessV1::for_sockets(config.socket_access_policies()),
    )?;
    // Every mutable adapter derives from the identity protected by the held
    // lease, not from a path alias that can be retargeted during composition.
    let state_root = state_root_lease.state_root_identity_v1().to_path_buf();
    refuse_legacy_state_root_v1(&state_root)?;
    let (writer_admission, gate_metric_source) =
        writer_gate_for(process_memory_envelope.rss_ceiling, &memory_probe);
    let lex_adapter: Arc<LexicalAdapter> = Arc::new(
        LexicalAdapter::with_state_root_and_policies(
            state_root.join("indexes/lexical"),
            RegexPolicy::defaults(),
            config.lexical_execution_budget(),
            config.regex_match_cache_policy(),
            config.lexical_writer_policy(),
        )
        .with_writer_admission(writer_admission)
        .map_err(anyhow::Error::from)?,
    );
    // The semantic adapter admits every streamed window against the same
    // policy the search plane's derived source cuts them by (QI-BB-021).
    let sem_adapter: Arc<SemanticAdapter> =
        Arc::new(SemanticAdapter::with_state_root_and_window_policy(
            quanta_index_semantic::semantic_state_root(&state_root),
            config.semantic_stream_window_policy(),
        )?);
    // The history text index lives beside the other authorities, one
    // immutable directory per published history epoch.
    let history_text_index: Arc<dyn HistoryTextIndexPort + Send + Sync> = Arc::new(
        HistoryTextIndexAdapter::with_root(state_root.join("authorities/history-text"))
            .map_err(anyhow::Error::from)?,
    );
    // Retention measures its byte limits over the index bytes the two
    // adapters know how to measure (QI-BB-003), never over record sizes.
    let index_bytes: Arc<dyn SearchCorpusIndexBytesPort> = Arc::new(PairIndexBytesMeasurer::new(
        lex_adapter.clone(),
        sem_adapter.clone(),
    ));
    let search_corpus_lifecycle = Arc::new(SearchCorpusLifecycleOwner::open(
        &state_root,
        search_corpus_history_retention,
        index_bytes,
    )?);
    search_corpus_lifecycle
        .activation_catalog()
        .configure_generation_namespace_reads_v1(lex_adapter.clone(), sem_adapter.clone())?;
    // The durable idempotency catalog (QI-BB-032). Its busy budget only
    // matters against a foreign writer, which the state-root lease excludes;
    // it is bounded so a held lock is still a typed answer, never a hang.
    let catalog = Arc::new(
        SqliteCatalog::open(&state_root, CATALOG_BUSY_BUDGET).map_err(anyhow::Error::from)?,
    );
    let shared_catalog = Arc::clone(&catalog);
    let idempotency: Arc<dyn IdempotencyCatalogPort + Send + Sync> = shared_catalog.clone();
    let mutation_coordinator: Arc<dyn MutationCoordinatorPort + Send + Sync> = shared_catalog;

    // The RepoMap store shares the same durable catalog: its candidate,
    // activation, invalidation and quarantine rows are the sole visibility
    // authority, with the filesystem under `repo-map/` holding immutable
    // content-addressed object projections only (SEP-21 P03).
    let opened_repo_map =
        RepoMapGenerationStore::open(state_root.join("repo-map"), Arc::clone(&catalog))
            .map_err(anyhow::Error::from)?;
    let auxiliary_catalog: Arc<dyn AuxiliaryAuthorityCatalogPort + Send + Sync> = catalog;
    let repo_map_store = Arc::new(opened_repo_map.store);
    let repo_map_open_report = opened_repo_map.report;

    let search_corpus_build_port: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync> =
        wrap_lexical_builder(lex_adapter.clone());
    let lexical_generation_scanner: Arc<dyn SealedGenerationScanPort + Send + Sync> =
        lex_adapter.clone();
    let lexical_integrity_scrub: Arc<dyn IntegrityScrubPort + Send + Sync> = lex_adapter.clone();
    let lexical_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync> =
        lex_adapter.clone();
    let lexical_identity_probe: Arc<dyn SealedGenerationIdentityProbePort> = lex_adapter.clone();
    let lexical_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync> =
        lex_adapter.clone();
    let lexical_sealed_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync> =
        lex_adapter.clone();
    let lex_open_port: Arc<dyn LexicalIndexOpenPort + Send + Sync> = lex_adapter.clone();
    let repo_commit_recency_ingest_port: Arc<dyn RepoCommitRecencyIngestPort + Send + Sync> =
        lex_adapter.clone();
    let repo_topic_ingest_port: Arc<dyn RepoTopicIngestPort + Send + Sync> = lex_adapter.clone();
    let repo_description_ingest_port: Arc<dyn RepoDescriptionIngestPort + Send + Sync> =
        lex_adapter.clone();
    let file_ownership_ingest_port: Arc<dyn FileOwnershipIngestPort + Send + Sync> =
        lex_adapter.clone();
    let file_contributor_ingest_port: Arc<dyn FileContributorIngestPort + Send + Sync> =
        lex_adapter.clone();
    let repo_meta_ingest_port: Arc<dyn RepoMetaIngestPort + Send + Sync> = lex_adapter.clone();
    let lexical_quarantine_discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync> =
        lex_adapter.clone();
    let lexical_door_findings: Arc<dyn DoorFindingQuarantinePort + Send + Sync> =
        lex_adapter.clone();
    // The lexical adapter keeps the writer envelope and regex cache tallies,
    // the semantic adapter the dense lanes' query and budget-interruption
    // tallies (QI-BB-015); the writer gate keeps its refusals. The
    // maintenance timer sweeps the lexical writers and measures both
    // tracks' generations through their own walkers (QI-BB-016).
    let writer_idle_sweep: Arc<dyn WriterIdleSweepPort> = lex_adapter.clone();
    let lexical_disk_usage: Arc<dyn TrackDiskUsagePort> = lex_adapter.clone();
    let semantic_disk_usage: Arc<dyn TrackDiskUsagePort> = sem_adapter.clone();
    let lexical_metric_source: Arc<dyn MetricSourcePort> = lex_adapter;
    let semantic_metric_source: Arc<dyn MetricSourcePort> = sem_adapter.clone();
    let sem_build_port: Arc<dyn SemanticScopeStreamBuildPort + Send + Sync> = sem_adapter.clone();
    let semantic_generation_scanner: Arc<dyn SealedGenerationScanPort + Send + Sync> =
        sem_adapter.clone();
    let semantic_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync> =
        sem_adapter.clone();
    let semantic_identity_probe: Arc<dyn SealedGenerationIdentityProbePort> = sem_adapter.clone();
    let semantic_content_roots: Arc<dyn SemanticContentRootsPort + Send + Sync> =
        sem_adapter.clone();
    let semantic_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync> =
        sem_adapter.clone();
    let semantic_sealed_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync> =
        sem_adapter.clone();
    let semantic_quarantine_discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync> =
        sem_adapter.clone();
    let semantic_door_findings: Arc<dyn DoorFindingQuarantinePort + Send + Sync> =
        sem_adapter.clone();
    // Both adapters prove their sealed generations' bytes as maintenance
    // (QI-BB-017), through the one scrub port.
    let semantic_integrity_scrub: Arc<dyn IntegrityScrubPort + Send + Sync> = sem_adapter.clone();
    let sem_open_port: Arc<dyn SemanticIndexOpenPort + Send + Sync> = sem_adapter;
    let repo_map_snapshot_port: Arc<dyn RepoMapSnapshotAcquirePort + Send + Sync> =
        repo_map_store.clone();
    let repo_map_bundle_ingest_port: Arc<dyn RepoMapBundleIngestPort + Send + Sync> =
        repo_map_store.clone();
    let repo_map_generation_activate_port: Arc<dyn RepoMapGenerationActivatePort + Send + Sync> =
        repo_map_store.clone();
    let repo_map_quarantine: Arc<dyn RepoMapQuarantinePort + Send + Sync> = repo_map_store;

    let mut parts = SearchdRuntimeParts {
        diagnostic_capture: None,
        state_root_lease,
        search_corpus_build_port,
        lexical_generation_scanner,
        lexical_generation_validator,
        lexical_identity_probe,
        lexical_incomplete_discard,
        lexical_sealed_reclaim,
        lex_open_port,
        repo_commit_recency_ingest_port,
        repo_topic_ingest_port,
        repo_description_ingest_port,
        file_ownership_ingest_port,
        file_contributor_ingest_port,
        repo_meta_ingest_port,
        sem_build_port,
        semantic_generation_scanner,
        semantic_generation_validator,
        semantic_identity_probe,
        semantic_content_roots,
        semantic_incomplete_discard,
        semantic_sealed_reclaim,
        sem_open_port,
        repo_map_snapshot_port,
        repo_map_bundle_ingest_port,
        repo_map_generation_activate_port,
        lexical_quarantine_discard,
        semantic_quarantine_discard,
        lexical_door_findings,
        semantic_door_findings,
        repo_map_quarantine,
        repo_map_open_report,
        search_corpus_lifecycle,
        idempotency,
        mutation_coordinator,
        auxiliary_catalog,
        history_text_index,
        adapter_metric_sources: vec![
            lexical_metric_source,
            semantic_metric_source,
            gate_metric_source,
        ],
        integrity_scrub_ports: vec![lexical_integrity_scrub, semantic_integrity_scrub],
        writer_idle_sweep,
        lexical_disk_usage,
        semantic_disk_usage,
        memory_probe,
    };
    wrap_parts(&mut parts);
    assemble(config, parts)
}

/// The lexical writer gate for a resident-memory ceiling (QI-BB-016):
/// the probe-backed gate when one is configured, the unbounded one when
/// not, each also as the metric source that reports it.
fn writer_gate_for(
    rss_ceiling: Option<u64>,
    memory_probe: &Arc<dyn ProcessMemoryProbePort>,
) -> (Arc<dyn WriterAdmissionPort>, Arc<dyn MetricSourcePort>) {
    if let Some(ceiling) = rss_ceiling {
        let gate = Arc::new(ResidentMemoryWriterAdmission::new(
            Arc::clone(memory_probe),
            ceiling,
        ));
        let admission: Arc<dyn WriterAdmissionPort> = gate.clone();
        return (admission, gate);
    }
    let gate = Arc::new(UnboundedWriterAdmission);
    let admission: Arc<dyn WriterAdmissionPort> = gate.clone();
    (admission, gate)
}

/// One line of the daemon's boot log, on stderr.
#[expect(
    clippy::print_stderr,
    reason = "the daemon entry is the one place operator-facing boot notices are written; the runtime keeps them as data"
)]
fn boot_log(line: &str) {
    eprintln!("searchd: {line}");
}

/// The daemon entry: the process mask is hardened before any path is
/// created (QI-BB-014), then the config chain runs and the runtime boots
/// under the supervisor.
///
/// The fixed exit code semantics (S21-09): clean operator shutdown `0`;
/// required-child death, startup rollback, or hard-deadline escalation
/// `70`; a second signal `128 + signum`.
pub fn run(command: SearchdCommand) -> Result<()> {
    let outcome = run_supervised(command)?;
    if outcome.is_stopped_clean() {
        return Ok(());
    }
    Err(anyhow::Error::from(
        quanta_index_searchd::SupervisionError { outcome },
    ))
}

/// The daemon entry with the supervision outcome surfaced, for the
/// process entry to map onto the fixed exit codes.
///
/// Signals are installed before the runtime is built, so a signal
/// during startup still reaches the supervisor.
pub fn run_supervised(command: SearchdCommand) -> Result<quanta_index_searchd::SupervisionOutcome> {
    let inherited = quanta_index_searchd::app::harden_umask();
    boot_log(&format!(
        "umask set to {:03o} (inherited {inherited:03o})",
        quanta_index_searchd::app::DAEMON_UMASK
    ));
    let root = quanta_index_searchd::CancelRoot::new();
    // The watcher is held for the whole supervised lifetime; it detaches
    // itself once an abort is latched or the process exits.
    let _signal_watch = signal::install(quanta_index_searchd::CancelRoot::clone(&root))
        .map_err(anyhow::Error::from)?;
    let trace_path = command.diagnostic_trace_path().cloned();
    let config = command.into_config()?;
    let mut capture = trace_path
        .map(|path| DiagnosticCaptureV1::start(path, DiagnosticCaptureLimitsV1::default()))
        .transpose()?;
    let runtime_result = match capture.as_ref() {
        Some(capture) => capture.lease().map_or_else(
            || {
                Err(anyhow::anyhow!(
                    "new diagnostic capture refused producer lease"
                ))
            },
            |lease| build_runtime_with_capture(config, lease),
        ),
        None => build_runtime(config),
    };
    let runtime = match runtime_result {
        Ok(runtime) => runtime,
        Err(error) => {
            if let Some(capture) = capture.as_mut()
                && let Err(close_error) = capture.close(HARD_DRAIN_DEADLINE)
            {
                boot_log(&format!(
                    "diagnostic capture close after boot failure: {close_error}"
                ));
            }
            return Err(error);
        }
    };
    for notice in &runtime.boot_notices {
        boot_log(notice);
    }
    let outcome = supervise_runtime(
        runtime,
        DEFAULT_COOPERATIVE_DRAIN_DEADLINE,
        HARD_DRAIN_DEADLINE,
        &root,
    );
    if let Some(capture) = capture.as_mut() {
        match capture.close(HARD_DRAIN_DEADLINE) {
            Ok(_closed) => {}
            Err(error) if outcome.is_stopped_clean() => {
                return Err(anyhow::anyhow!(
                    "diagnostic capture close failed: {error:?}"
                ));
            }
            Err(error) => boot_log(&format!("diagnostic capture remains incomplete: {error:?}")),
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod admitted_publish_timeout_tests;
#[cfg(test)]
mod process_slow_disk_tests;
#[cfg(test)]
mod public_cache_tests;

#[cfg(test)]
mod diagnostic_runtime_parent_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use quanta_index_core::{
        DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1, DiagnosticEventSinkV1,
    };
    use quanta_index_searchd::{
        DiagnosticCaptureLimitsV1, DiagnosticCaptureStatusV1, DiagnosticCaptureV1, SearchdConfig,
    };

    use super::{KernelResidentMemoryProbe, build_runtime_with_memory_probe_and_capture_under};

    #[derive(Debug)]
    struct FixedClock;

    impl DiagnosticClockV1 for FixedClock {
        fn now_ns(&self) -> u64 {
            0
        }
    }

    #[derive(Debug)]
    struct DiscardSink;

    impl DiagnosticEventSinkV1 for DiscardSink {
        fn try_emit(&self, _event: DiagnosticEmitV1) -> bool {
            true
        }
    }

    #[test]
    fn missing_trace_parent_marks_capture_invalid_without_blocking_runtime_build() {
        let dir = tempfile::tempdir().expect("test directory");
        let mut capture = DiagnosticCaptureV1::start(
            dir.path().join("missing-parent.jsonl"),
            DiagnosticCaptureLimitsV1::default(),
        )
        .expect("capture");
        let lease = capture.lease().expect("producer lease");
        let parentless = lease.context().clone();
        let state_root = dir.path().join("state");
        let runtime = build_runtime_with_memory_probe_and_capture_under(
            SearchdConfig::from_test_state_root(state_root.clone())
                .try_with_search_corpus_history_retention_limits(8, 16 << 20, 128, 256 << 20)
                .expect("test history policy"),
            Arc::new(KernelResidentMemoryProbe),
            lease,
            Some(&parentless),
        )
        .expect("diagnostic failure must not block product boot");
        assert!(state_root.is_dir());
        drop(runtime);
        drop(parentless);
        let closed = capture
            .close(Duration::from_secs(2))
            .expect("capture close");
        assert_eq!(closed.status, DiagnosticCaptureStatusV1::Invalid);
    }

    #[test]
    fn same_public_capture_id_from_foreign_session_is_rejected_before_state_root_access() {
        let dir = tempfile::tempdir().expect("test directory");
        let mut capture = DiagnosticCaptureV1::start(
            dir.path().join("runtime-parent.jsonl"),
            DiagnosticCaptureLimitsV1::default(),
        )
        .expect("capture");
        let lease = capture.lease().expect("producer lease");
        let foreign_parent = DiagnosticContextV1::new(
            lease.context().capture_id(),
            Arc::new(FixedClock),
            Arc::new(DiscardSink),
        );
        assert_eq!(lease.context().capture_id(), foreign_parent.capture_id());
        assert!(!lease.context().shares_capture_session(&foreign_parent));

        let state_root = dir.path().join("unopened-state-root");
        let result = build_runtime_with_memory_probe_and_capture_under(
            SearchdConfig::from_test_state_root(state_root.clone()),
            Arc::new(KernelResidentMemoryProbe),
            lease,
            Some(&foreign_parent),
        );
        let error = match result {
            Ok(_runtime) => panic!("foreign diagnostic parent was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("different capture session"));
        assert!(
            !state_root.exists(),
            "state root was touched before rejection"
        );
        let _closed = capture
            .close(Duration::from_secs(2))
            .expect("capture close");
    }
}
