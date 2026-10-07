//! QI-BB-016 through the real daemon: one process memory envelope, a
//! resident-memory gate on the lexical writers, and writer release at
//! the complete source-event publication boundary.
//!
//! Oracles are external to the code under test: a scripted memory probe
//! the test moves above and below the ceiling, the typed refusal code on
//! the ingest socket, the writer tallies and process gauge in a metrics
//! scrape, and the daemon's own refusal to boot an envelope whose
//! policies do not fit its ceiling.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use quanta_index_contract::{
    MetricsSnapshotV1, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, TextQuerySyntax,
};
use quanta_index_core::{
    CoreError, LexicalWriterPolicy, PROCESS_MEMORY_ENVELOPE_EXCEEDED_CODE,
    PROCESS_RSS_CEILING_EXCEEDED_CODE, ProcessMemoryEnvelopeV1, ProcessMemoryProbePort,
};
use quanta_index_searchd::app::ProcessMemoryCeilings;
use quanta_index_searchd_harness::E2eRuntime;

use crate::fail_closed_wait::{RealTicker, WaitError, WaitTimeout, wait_for};

type TestResult = Result<(), Box<dyn Error>>;

/// A probe the test drives: what the daemon believes its resident set is.
struct ScriptedProbe(AtomicU64);

impl ProcessMemoryProbePort for ScriptedProbe {
    fn resident_bytes(&self) -> Result<u64, CoreError> {
        Ok(self.0.load(Ordering::Acquire))
    }
}

#[derive(Debug)]
struct Scrape {
    counters: BTreeMap<String, u64>,
    gauges: BTreeMap<String, f64>,
}

impl Scrape {
    fn take(rt: &mut E2eRuntime) -> Result<Self, Box<dyn Error>> {
        let snapshot: MetricsSnapshotV1 = rt.metrics_snapshot()?;
        Ok(Self {
            counters: snapshot
                .counters
                .into_iter()
                .map(|counter| (counter.name, counter.value))
                .collect(),
            gauges: snapshot
                .gauges
                .into_iter()
                .map(|gauge| (gauge.name, gauge.value))
                .collect(),
        })
    }

    fn counter(&self, name: &str) -> Result<u64, Box<dyn Error>> {
        self.counters
            .get(name)
            .copied()
            .ok_or_else(|| format!("counter `{name}` is in the scrape").into())
    }

    fn gauge(&self, name: &str) -> Result<f64, Box<dyn Error>> {
        self.gauges
            .get(name)
            .copied()
            .ok_or_else(|| format!("gauge `{name}` is in the scrape").into())
    }
}

/// Compare through `PartialEq` so a gauge (an `f64` carrying an exact
/// integer count) is checked for the value the daemon put on the wire.
fn expect_eq<T: PartialEq + std::fmt::Debug>(what: &str, observed: &T, expected: &T) -> TestResult {
    if observed == expected {
        Ok(())
    } else {
        Err(format!("{what}: observed {observed:?}, expected {expected:?}").into())
    }
}

fn typed_code(response: &SearchPlaneIngestIpcResponse) -> Option<&str> {
    match response {
        SearchPlaneIngestIpcResponse::Error(error) => Some(error.code.as_wire_str()),
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
        | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(_)
        | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => None,
    }
}

/// Wait, bounded, until `condition` holds against fresh scrapes
///
/// (TOPT-06/TH-2): a spent wait is a typed timeout naming `what` and
/// carrying the last scrape — never the stale scrape as success. A
/// scrape failure itself is terminal and returns at once.
fn wait_for_scrape(
    rt: &mut E2eRuntime,
    bound: Duration,
    what: &str,
    condition: impl Fn(&Scrape) -> bool,
) -> Result<Scrape, Box<dyn Error>> {
    wait_for_scrape_with(|| Scrape::take(rt), bound, what, condition)
}

fn wait_for_scrape_with(
    read: impl FnMut() -> Result<Scrape, Box<dyn Error>>,
    bound: Duration,
    what: &str,
    condition: impl Fn(&Scrape) -> bool,
) -> Result<Scrape, Box<dyn Error>> {
    match wait_for(
        &RealTicker::new(),
        bound,
        Duration::from_millis(10),
        what,
        read,
        condition,
        |_| false,
    ) {
        Ok(scrape) => Ok(scrape),
        // A boxed scrape failure is already the terminal error: return
        // it unwrapped, never rendered-and-reboxed.
        Err(WaitError::Terminal(boxed)) => Err(boxed),
        Err(WaitError::Timeout(timeout)) => Err(Box::new(timeout)),
    }
}

fn stable_disk_refresh_window(
    mut read: impl FnMut() -> Result<Scrape, Box<dyn Error>>,
    bound: Duration,
    initial: &Scrape,
) -> Result<Scrape, Box<dyn Error>> {
    let after = |scrape: &Scrape| -> Result<u64, Box<dyn Error>> {
        scrape
            .counter("maintenance_disk_refreshes_total")?
            .checked_add(2)
            .ok_or_else(|| "disk refresh counter overflow".into())
    };
    // The initial gauges can already match while an older scan is still
    // running. Drain it before choosing the cumulative-failure baseline.
    let drain_target = after(initial)?;
    let drained = wait_for_scrape_with(
        &mut read,
        bound,
        "disk refreshes draining the prior in-flight scan",
        |scrape| {
            scrape
                .counter("maintenance_disk_refreshes_total")
                .is_ok_and(|count| count >= drain_target)
        },
    )?;
    let fresh_target = after(&drained)?;
    let fresh = wait_for_scrape_with(
        read,
        bound,
        "a completed disk refresh after draining prior work",
        |scrape| {
            scrape
                .counter("maintenance_disk_refreshes_total")
                .is_ok_and(|count| count >= fresh_target)
        },
    )?;
    expect_eq(
        "no fresh walk failed after draining prior work",
        &fresh.counter("maintenance_disk_refresh_failures_total")?,
        &drained.counter("maintenance_disk_refresh_failures_total")?,
    )?;
    Ok(fresh)
}

#[test]
fn stable_disk_window_drains_late_history_and_refuses_a_fresh_failure() -> TestResult {
    let snapshot = |starts, failures| Scrape {
        counters: BTreeMap::from([
            ("maintenance_disk_refreshes_total".to_string(), starts),
            (
                "maintenance_disk_refresh_failures_total".to_string(),
                failures,
            ),
        ]),
        gauges: BTreeMap::from([
            (
                "search_corpus_lexical_generation_disk_bytes".to_string(),
                10.0,
            ),
            (
                "search_corpus_semantic_generation_disk_bytes".to_string(),
                20.0,
            ),
        ]),
    };
    // Cached values match before the prior scan's failure arrives. Its
    // failure belongs to the drained history, while the new window is clean.
    let initial = snapshot(2, 0);
    let mut recovered = std::collections::VecDeque::from([snapshot(4, 1), snapshot(6, 1)]);
    let fresh = stable_disk_refresh_window(
        || {
            recovered
                .pop_front()
                .ok_or_else(|| "fixed trace exhausted".into())
        },
        Duration::from_secs(1),
        &initial,
    )?;
    expect_eq(
        "recovered failure history",
        &fresh.counter("maintenance_disk_refresh_failures_total")?,
        &1,
    )?;
    expect_eq(
        "cached lexical bytes",
        &fresh.gauge("search_corpus_lexical_generation_disk_bytes")?,
        &10.0,
    )?;
    let mut failed = std::collections::VecDeque::from([snapshot(4, 1), snapshot(6, 2)]);
    let failure = stable_disk_refresh_window(
        || {
            failed
                .pop_front()
                .ok_or_else(|| "fixed trace exhausted".into())
        },
        Duration::from_secs(1),
        &initial,
    )
    .err()
    .ok_or("a fresh walk failure passed the stable window")?;
    if !failure.to_string().contains("no fresh walk failed") {
        return Err(format!("fresh failure was refused for another reason: {failure}").into());
    }
    Ok(())
}

/// The resident-memory gate: a writer is refused typed while the daemon
/// sees itself above the ceiling, admitted again below it, the refusal is
/// counted, and the process gauge reports what the probe said.
#[test]
fn a_new_writer_is_refused_typed_above_the_rss_ceiling_and_admitted_below_it() -> TestResult {
    let ceiling = ProcessMemoryEnvelopeV1::DEFAULT_CEILING_BYTES;
    let probe = Arc::new(ScriptedProbe(AtomicU64::new(ceiling + 1)));
    let probe_port: Arc<dyn ProcessMemoryProbePort> = probe.clone();
    let mut rt = E2eRuntime::boot_with_memory_probe(
        probe_port,
        ProcessMemoryCeilings::new(ceiling, Some(ceiling))?,
    )?;
    // Above the ceiling: the first batch needs a writer and is refused.
    let batch = rt.text_search_corpus_batch("src/gate.rs", "fn gate_body() { gate_needle }")?;
    let refused = rt.ingest_once(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(
        batch.clone(),
    ))?;
    if typed_code(&refused) != Some(PROCESS_RSS_CEILING_EXCEEDED_CODE.as_wire_str()) {
        return Err(format!(
            "a writer open above the resident-memory ceiling is refused typed, got {refused:?}"
        )
        .into());
    }
    let scrape = Scrape::take(&mut rt)?;
    expect_eq(
        "the gate reports itself enabled",
        &scrape.gauge("lexical_writer_rss_gate_enabled")?,
        &1.0,
    )?;
    expect_eq(
        "the gate reports its ceiling",
        &scrape.gauge("lexical_writer_rss_ceiling_bytes")?,
        &quanta_index_core::count_as_f64(ceiling),
    )?;
    if scrape.counter("lexical_writer_rss_refusals_total")? != 1 {
        return Err("the refusal is counted once".into());
    }
    expect_eq(
        "the process gauge reports what the probe read",
        &scrape.gauge("process_resident_bytes")?,
        &quanta_index_core::count_as_f64(ceiling + 1),
    )?;
    expect_eq(
        "no writer was opened",
        &scrape.gauge("lexical_writers_open")?,
        &0.0,
    )?;

    // The source publication is retryable after this runtime admission
    // failure. Retry the exact same event and body: a different event may
    // not take over the target generation already bound by staged coverage.
    probe.0.store(ceiling - 1, Ordering::Release);
    rt.publish_search_corpus_batch(batch)?;
    rt.activate_last_sealed_generation()?;
    let served = rt.query_text(TextQuerySyntax::Native, "gate_needle", 10);
    if let Some(error) = served.typed_error {
        return Err(format!("the admitted batch serves: {error:?}").into());
    }
    if served.candidate_ids.len() != 1 {
        return Err(format!("one hit, got {:?}", served.candidate_ids).into());
    }
    let scrape = Scrape::take(&mut rt)?;
    if scrape.counter("lexical_writer_rss_refusals_total")? != 1 {
        return Err("no further refusal below the ceiling".into());
    }
    expect_eq(
        "the process gauge follows the probe",
        &scrape.gauge("process_resident_bytes")?,
        &quanta_index_core::count_as_f64(ceiling - 1),
    )?;
    Ok(())
}

/// Producer staging is local. The public source-event door accepts only a
/// complete sealed batch, so it cannot retain a daemon writer between files.
#[test]
fn local_staging_has_no_daemon_writer_and_sealed_publication_releases_heap() -> TestResult {
    let idle = Duration::from_secs(2);
    let mut rt = E2eRuntime::boot_with_lexical_writer_policy(LexicalWriterPolicy::new(
        LexicalWriterPolicy::DEFAULT.envelope_bytes(),
        LexicalWriterPolicy::DEFAULT.writer_heap_bytes(),
        idle,
    )?)?;
    rt.ingest_text("repo-idle", "src/idle.rs", "idle needle")?;
    let staged = Scrape::take(&mut rt)?;
    expect_eq(
        "local staging opens no daemon writer",
        &staged.gauge("lexical_writers_open")?,
        &0.0,
    )?;
    expect_eq(
        "local staging allocates no daemon writer heap",
        &staged.gauge("lexical_writers_allocated_heap_bytes")?,
        &0.0,
    )?;
    expect_eq(
        "staging does not require the timer to close a writer",
        &staged.counter("lexical_writer_idle_releases_total")?,
        &0,
    )?;
    let _generation = rt.seal()?;
    let published = Scrape::take(&mut rt)?;
    expect_eq(
        "sealed publication releases its daemon writer",
        &published.gauge("lexical_writers_open")?,
        &0.0,
    )?;
    expect_eq(
        "sealed publication returns its writer heap",
        &published.gauge("lexical_writers_allocated_heap_bytes")?,
        &0.0,
    )?;
    rt.activate_last_sealed_generation()?;
    let served = rt.query_text(TextQuerySyntax::Native, "needle", 10);
    if let Some(error) = served.typed_error {
        return Err(format!("the sealed event serves after activation: {error:?}").into());
    }
    expect_eq(
        "the sealed source has one match",
        &served.candidate_ids.len(),
        &1,
    )?;
    Ok(())
}

/// The per-track disk gauges follow the daemon's own byte walkers: after a
/// seal, each track's gauge equals an independent walk of the track's
/// directory tree.
#[test]
fn the_generation_disk_gauges_match_an_independent_walk_after_a_seal() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo-disk", "src/disk.rs", "disk needle one")?;
    rt.ingest_text("repo-disk", "src/disk_two.rs", "disk needle two")?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let lexical = walk_bytes(&rt.state_root().join("indexes/lexical"))?;
    let semantic = walk_bytes(&rt.state_root().join("indexes/semantic"))?;
    if lexical == 0 || semantic == 0 {
        return Err(format!(
            "a sealed generation has bytes on both tracks: lexical={lexical} semantic={semantic}"
        )
        .into());
    }
    let bound = rt
        .maintenance_policy()
        .tick()
        .saturating_mul(10)
        .saturating_add(Duration::from_secs(5));
    let initial = wait_for_scrape(
        &mut rt,
        bound,
        "the disk gauges matching the independent walk",
        |scrape| {
            scrape
                .gauge("search_corpus_lexical_generation_disk_bytes")
                .is_ok_and(|bytes| {
                    bytes.to_bits() == quanta_index_core::count_as_f64(lexical).to_bits()
                })
                && scrape
                    .gauge("search_corpus_semantic_generation_disk_bytes")
                    .is_ok_and(|bytes| {
                        bytes.to_bits() == quanta_index_core::count_as_f64(semantic).to_bits()
                    })
        },
    )?;
    // Refreshes count starts. Two subsequent starts on the single meter
    // prove one whole scan completed; drain old work before the new window.
    let scrape = stable_disk_refresh_window(|| Scrape::take(&mut rt), bound, &initial)?;
    // Re-walk at assertion time: the tree is quiescent and the gauge must
    // still match both independent walks after a fresh completed scan.
    let lexical_now = walk_bytes(&rt.state_root().join("indexes/lexical"))?;
    let semantic_now = walk_bytes(&rt.state_root().join("indexes/semantic"))?;
    expect_eq(
        "lexical disk gauge vs an independent walk",
        &scrape.gauge("search_corpus_lexical_generation_disk_bytes")?,
        &quanta_index_core::count_as_f64(lexical_now),
    )?;
    expect_eq(
        "semantic disk gauge vs an independent walk",
        &scrape.gauge("search_corpus_semantic_generation_disk_bytes")?,
        &quanta_index_core::count_as_f64(semantic_now),
    )?;
    Ok(())
}

/// Regular-file bytes under `root`, each `(device, inode)` once — the
/// definition the adapters' gauges use (writer lock files excluded, which
/// a quiescent tree has none of).
fn walk_bytes(root: &std::path::Path) -> Result<u64, Box<dyn Error>> {
    use std::os::unix::fs::MetadataExt as _;
    let mut seen = std::collections::BTreeSet::new();
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with(".tantivy") {
                    continue;
                }
                if seen.insert((metadata.dev(), metadata.ino())) {
                    total = total.saturating_add(metadata.len());
                }
            }
        }
    }
    Ok(total)
}

/// An envelope whose declared policies do not fit its ceiling refuses
/// boot typed, before any socket exists.
#[test]
fn an_envelope_over_its_ceiling_refuses_boot_typed_before_any_socket() -> TestResult {
    let declared = quanta_index_searchd::app::SearchdConfig::from_test_state_root(
        std::env::temp_dir().join("quanta-index-envelope-probe"),
    )
    .process_memory_envelope()?
    .declared_bytes()?;
    let mut rt = E2eRuntime::boot_with_memory_probe(
        Arc::new(ScriptedProbe(AtomicU64::new(0))),
        ProcessMemoryCeilings::new(declared - 1, None)?,
    )?;
    let refused = match rt.start() {
        Ok(()) => return Err("an envelope over its ceiling must refuse boot".into()),
        Err(error) => format!("{error:#}"),
    };
    if !refused.contains(PROCESS_MEMORY_ENVELOPE_EXCEEDED_CODE.as_wire_str()) {
        return Err(format!("the refusal names the code: {refused}").into());
    }
    if rt.socket_paths().is_some() || rt.boot_inventory().is_some() {
        return Err("a refused boot leaves no running driver".into());
    }
    Ok(())
}

/// TH-2 adapter proof (TOPT-06): a scrape predicate that never holds
///
/// returns a typed timeout carrying the last scrape — never the stale
/// scrape as success. The bound is short but the verdict cannot flake:
/// a never-true predicate times out however the bound elapses.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "typed-timeout integration assertions intentionally fail the test while setup uses Result"
)]
fn scrape_wait_never_true_predicate_returns_typed_timeout() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    // Boot the daemon outside the bound: the first scrape lazy-starts
    // it, and boot time must not consume the attempt budget below.
    let _warmed = Scrape::take(&mut rt)?;
    let error = wait_for_scrape(
        &mut rt,
        Duration::from_millis(100),
        "the scrape that never satisfies",
        |_| false,
    )
    .expect_err("a never-true predicate fails");
    let timeout = error
        .downcast_ref::<WaitTimeout>()
        .ok_or_else(|| format!("a spent scrape wait is a typed timeout, got {error:?}"))?;
    assert!(
        timeout.expected.contains("never satisfies"),
        "the timeout names its predicate: {}",
        timeout.expected
    );
    let last = timeout.last.as_ref().ok_or("the last scrape is evidence")?;
    assert!(
        last.contains("Scrape"),
        "the last observation is a scrape, not a render failure: {last}"
    );
    Ok(())
}
