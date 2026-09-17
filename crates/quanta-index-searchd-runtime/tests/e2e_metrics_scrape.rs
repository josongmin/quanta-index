//! QI-BB-015 — the daemon's metrics scrape over its control socket.
//!
//! The oracle is the traffic the test itself generates: between two scrapes
//! it sends an exact number of lexical queries, one of which is a typed
//! error, and the route counters, the route latency histogram, the socket
//! server counters, the snapshot registry tallies and the diagnostic
//! tallies must all move by exactly that traffic. Boot gauges are checked
//! against the boot inventory the harness holds, and the writer envelope
//! against the one seal the fixture performed.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;

use quanta_index_contract::{MetricHistogramV1, MetricsSnapshotV1, TextQuerySyntax};
use quanta_index_core::count_as_f64;
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::E2eRuntime;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// Served queries between the first and second scrape.
const SERVED_QUERIES: u64 = 5;

/// Samples one served lexical query emits: intake, snapshot hit, planner,
/// engine fan-out, route latency, route outcome.
const SAMPLES_PER_SERVED_LEXICAL_QUERY: u64 = 6;

/// Samples one lexical query that times out in execution emits: intake,
/// snapshot hit (the handle is acquired before execution), the typed-error
/// bucket, route latency, route outcome.
const SAMPLES_PER_TIMEOUT_ERROR: u64 = 5;

struct Scrape {
    counters: BTreeMap<String, u64>,
    gauges: BTreeMap<String, f64>,
    histograms: BTreeMap<String, MetricHistogramV1>,
    snapshot: MetricsSnapshotV1,
}

impl Scrape {
    fn take(rt: &mut E2eRuntime) -> TestResult<Self> {
        let snapshot = rt.metrics_snapshot()?;
        Ok(Self {
            counters: snapshot
                .counters
                .iter()
                .map(|counter| (counter.name.clone(), counter.value))
                .collect(),
            gauges: snapshot
                .gauges
                .iter()
                .map(|gauge| (gauge.name.clone(), gauge.value))
                .collect(),
            histograms: snapshot
                .histograms
                .iter()
                .map(|histogram| (histogram.name.clone(), histogram.clone()))
                .collect(),
            snapshot,
        })
    }

    fn counter(&self, name: &str) -> TestResult<u64> {
        self.counters
            .get(name)
            .copied()
            .ok_or_else(|| format!("counter `{name}` is in the scrape: {:?}", self.counters).into())
    }

    fn gauge(&self, name: &str) -> TestResult<f64> {
        self.gauges
            .get(name)
            .copied()
            .ok_or_else(|| format!("gauge `{name}` is in the scrape: {:?}", self.gauges).into())
    }

    fn histogram(&self, name: &str) -> TestResult<&MetricHistogramV1> {
        self.histograms
            .get(name)
            .ok_or_else(|| format!("histogram `{name}` is in the scrape").into())
    }

    /// How much `name` grew since `earlier`.
    ///
    /// A counter appears in a scrape once it has been emitted, so one that
    /// is absent counts as zero on either side; an expectation above zero
    /// still catches a misspelled name.
    fn counter_delta(&self, earlier: &Self, name: &str) -> TestResult<u64> {
        let now = self.counters.get(name).copied().map_or(0, |value| value);
        let before = earlier.counters.get(name).copied().map_or(0, |value| value);
        now.checked_sub(before)
            .ok_or_else(|| format!("counter `{name}` went backwards: {before} -> {now}").into())
    }
}

fn expect_eq<T: PartialEq + std::fmt::Debug>(what: &str, observed: &T, expected: &T) -> TestResult {
    if observed == expected {
        Ok(())
    } else {
        Err(format!("{what}: observed {observed:?}, expected {expected:?}").into())
    }
}

fn seed(rt: &mut E2eRuntime) -> TestResult {
    rt.ingest_text("repo-metrics", "src/alpha.rs", "needle alpha")?;
    rt.ingest_text("repo-metrics", "src/beta.rs", "needle beta")?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    // One query absorbs readiness so later queries are served on their
    // first attempt and the counters move by exactly the traffic sent.
    let warm = rt.query_text(TextQuerySyntax::Native, "needle", 10);
    if let Some(error) = warm.typed_error {
        return Err(format!("warm-up query failed typed: {error:?}").into());
    }
    Ok(())
}

fn serve_queries(rt: &mut E2eRuntime, count: u64) -> TestResult {
    for _ in 0..count {
        let result = rt.query_text(TextQuerySyntax::Native, "needle", 10);
        if let Some(error) = result.typed_error {
            return Err(format!("served query failed typed: {error:?}").into());
        }
        if result.candidate_ids.len() != 2 {
            return Err(format!("both fixtures match: {:?}", result.candidate_ids).into());
        }
    }
    Ok(())
}

#[test]
fn route_socket_registry_and_diagnostic_tallies_move_by_exactly_the_traffic_sent() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    seed(&mut rt)?;

    let first = Scrape::take(&mut rt)?;
    serve_queries(&mut rt, SERVED_QUERIES)?;
    let second = Scrape::take(&mut rt)?;

    // Route outcome and latency.
    expect_eq(
        "lexical served",
        &second.counter_delta(&first, "lq_route_lexical_served_total")?,
        &SERVED_QUERIES,
    )?;
    expect_eq(
        "lexical errors",
        &second.counter_delta(&first, "lq_route_lexical_errors_total")?,
        &0,
    )?;
    expect_eq(
        "intake",
        &second.counter_delta(&first, "lq_query_intake_total")?,
        &SERVED_QUERIES,
    )?;
    let latency_before = first.histogram("lq_route_lexical_latency_ms")?;
    let latency_after = second.histogram("lq_route_lexical_latency_ms")?;
    expect_eq(
        "latency observations",
        &latency_after.count.checked_sub(latency_before.count),
        &Some(SERVED_QUERIES),
    )?;
    let last_bucket = latency_after
        .buckets
        .last()
        .ok_or("the histogram has buckets")?;
    if !last_bucket.le.is_finite() || last_bucket.count > latency_after.count {
        return Err(format!(
            "buckets are finite and never exceed the count: {last_bucket:?} vs {}",
            latency_after.count
        )
        .into());
    }
    if latency_after.sum < latency_before.sum || latency_after.max < latency_before.max {
        return Err(format!(
            "latency sum and max never shrink: {latency_before:?} -> {latency_after:?}"
        )
        .into());
    }

    // The query socket saw exactly those requests, each on its own
    // connection; the control socket saw the first scrape.
    expect_eq(
        "query requests dispatched",
        &second.counter_delta(&first, "ipc_query_requests_dispatched_total")?,
        &SERVED_QUERIES,
    )?;
    expect_eq(
        "query connections accepted",
        &second.counter_delta(&first, "ipc_query_connections_accepted_total")?,
        &SERVED_QUERIES,
    )?;
    expect_eq(
        "query connections refused",
        &second.counter_delta(&first, "ipc_query_connections_refused_total")?,
        &0,
    )?;
    expect_eq(
        "control requests dispatched",
        &second.counter_delta(&first, "ipc_control_requests_dispatched_total")?,
        &1,
    )?;
    expect_eq(
        "query peer hang-ups",
        &second.counter_delta(&first, "ipc_query_peer_hangups_total")?,
        &0,
    )?;

    // The registry served every query from the resident handle, and its
    // own tally agrees with the dispatcher's snapshot-hit counter.
    let registry_hits = second.counter_delta(&first, "snapshot_registry_lexical_hits_total")?;
    expect_eq("registry hits", &registry_hits, &SERVED_QUERIES)?;
    expect_eq(
        "dispatcher snapshot hits agree with the registry",
        &second.counter_delta(&first, "lq_snapshot_lexical_hit_total")?,
        &registry_hits,
    )?;
    expect_eq(
        "registry misses",
        &second.counter_delta(&first, "snapshot_registry_lexical_misses_total")?,
        &0,
    )?;
    expect_eq(
        "one lexical handle resident",
        &second.gauge("snapshot_registry_lexical_entries")?,
        &1.0,
    )?;

    // The diagnostic tails: every sample was recorded, nothing was refused.
    let diagnostics_before = first.snapshot.diagnostics;
    let diagnostics_after = second.snapshot.diagnostics;
    expect_eq(
        "samples recorded",
        &diagnostics_after
            .samples_recorded
            .checked_sub(diagnostics_before.samples_recorded),
        &Some(SERVED_QUERIES.saturating_mul(SAMPLES_PER_SERVED_LEXICAL_QUERY)),
    )?;
    expect_eq(
        "no obs errors",
        &(
            diagnostics_after.errors_recorded,
            diagnostics_after.errors_dropped,
            diagnostics_after.samples_dropped,
        ),
        &(0, 0, 0),
    )?;

    // A typed error is an error outcome on the same route, with the same
    // latency histogram observing it.
    let failed = rt.query_text(TextQuerySyntax::Native, "timeout:0ms /needle\\b/", 10);
    let error = failed.typed_error.ok_or("a zero timeout fails typed")?;
    expect_eq("typed error code", &error.code.as_str(), &"QUERY_TIMEOUT")?;
    let third = Scrape::take(&mut rt)?;
    expect_eq(
        "lexical errors after the typed error",
        &third.counter_delta(&second, "lq_route_lexical_errors_total")?,
        &1,
    )?;
    expect_eq(
        "lexical served after the typed error",
        &third.counter_delta(&second, "lq_route_lexical_served_total")?,
        &0,
    )?;
    expect_eq(
        "plan-limit bucket",
        &third.counter_delta(&second, "lq_typed_error_plan_limit_total")?,
        &1,
    )?;
    expect_eq(
        "latency observed the error too",
        &third
            .histogram("lq_route_lexical_latency_ms")?
            .count
            .checked_sub(latency_after.count),
        &Some(1),
    )?;
    expect_eq(
        "samples recorded by the error",
        &third
            .snapshot
            .diagnostics
            .samples_recorded
            .checked_sub(diagnostics_after.samples_recorded),
        &Some(SAMPLES_PER_TIMEOUT_ERROR),
    )?;
    expect_eq(
        "control requests dispatched by the second scrape",
        &third.counter_delta(&second, "ipc_control_requests_dispatched_total")?,
        &1,
    )?;
    Ok(())
}

#[test]
fn boot_gauges_match_the_boot_inventory_and_the_writer_envelope_reflects_the_seal() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    seed(&mut rt)?;
    let scrape = Scrape::take(&mut rt)?;
    let inventory = rt
        .boot_inventory()
        .ok_or("the harness holds the boot inventory while the driver runs")?
        .clone();

    let expected_boot: [(&str, u64); 12] = [
        (
            "boot_lexical_sealed_generations",
            u64::try_from(inventory.lexical.sealed_generations)?,
        ),
        (
            "boot_lexical_quarantined_generations",
            u64::try_from(inventory.lexical.quarantined.len())?,
        ),
        (
            "boot_semantic_sealed_generations",
            u64::try_from(inventory.semantic.sealed_generations)?,
        ),
        (
            "boot_semantic_quarantined_generations",
            u64::try_from(inventory.semantic.quarantined.len())?,
        ),
        (
            "boot_active_pairs_validated",
            u64::try_from(inventory.active_pairs_validated)?,
        ),
        (
            "boot_auxiliary_rows_restored",
            inventory.auxiliary_rows_restored,
        ),
        (
            "boot_repomap_snapshots_loaded",
            inventory.repo_map.snapshots_loaded,
        ),
        (
            "boot_repomap_snapshots_migrated",
            inventory.repo_map.snapshots_migrated,
        ),
        (
            "boot_repomap_activations_loaded",
            inventory.repo_map.activations_loaded,
        ),
        (
            "boot_repomap_stale_temporaries_removed",
            inventory.repo_map.stale_temporaries_removed,
        ),
        (
            "boot_repomap_quarantined_files",
            u64::try_from(inventory.repo_map.quarantined.len())?,
        ),
        (
            "boot_repomap_activations_without_snapshot",
            u64::try_from(inventory.repo_map.activations_without_snapshot.len())?,
        ),
    ];
    for (name, expected) in expected_boot {
        let observed = scrape.gauge(name)?;
        expect_eq(name, &observed, &count_as_f64(expected))?;
    }

    // One generation was written and sealed: its writer was released by
    // the seal, and nothing is open now. The envelope's ceiling is the
    // policy's, never zero.
    expect_eq(
        "seal releases",
        &scrape.counter("lexical_writer_seal_releases_total")?,
        &1,
    )?;
    expect_eq("open writers", &scrape.gauge("lexical_writers_open")?, &0.0)?;
    // Two chunks were ingested into one generation: the first batch found
    // no sidecars and rebuilt from its one document, the second updated
    // them in place with its one chunk; nothing was retired (QI-BB-006).
    expect_eq(
        "text-authority rebuilds",
        &scrape.counter("lexical_text_authority_rebuilds_total")?,
        &1,
    )?;
    expect_eq(
        "text-authority incremental updates",
        &scrape.counter("lexical_text_authority_incremental_updates_total")?,
        &1,
    )?;
    expect_eq(
        "text-authority docs derived",
        &scrape.counter("lexical_text_authority_docs_derived_total")?,
        &2,
    )?;
    expect_eq(
        "text-authority docs retired",
        &scrape.counter("lexical_text_authority_docs_retired_total")?,
        &0,
    )?;
    if scrape.gauge("lexical_writers_max")? < 1.0 {
        return Err("the writer ceiling is at least one".into());
    }
    expect_eq(
        "allocated heap follows open writers",
        &scrape.gauge("lexical_writers_allocated_heap_bytes")?,
        &0.0,
    )?;

    // Every socket server reports; nothing was refused or overloaded.
    for plane in ["query", "control", "ingest"] {
        for suffix in ["connections_refused_total", "requests_overloaded_total"] {
            let name = format!("ipc_{plane}_{suffix}");
            expect_eq(&name, &scrape.counter(&name)?, &0)?;
        }
        let live = scrape.gauge(&format!("ipc_{plane}_connections_live"))?;
        // The control connection carrying this very scrape is live.
        let expected_live = if plane == "control" { 1.0 } else { 0.0 };
        expect_eq(
            &format!("ipc_{plane}_connections_live"),
            &live,
            &expected_live,
        )?;
    }
    // The ingest envelope admitted the fixture's batches and refused none.
    if scrape.counter("ingest_batches_admitted_total")? == 0 {
        return Err("the fixture's ingest batches were admitted".into());
    }
    expect_eq(
        "ingest refusals",
        &scrape.counter("ingest_batches_refused_total")?,
        &0,
    )?;
    Ok(())
}
