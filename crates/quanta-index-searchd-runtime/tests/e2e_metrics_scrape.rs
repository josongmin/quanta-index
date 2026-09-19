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
/// engine fan-out, examined candidates, route latency, route outcome.
const SAMPLES_PER_SERVED_LEXICAL_QUERY: u64 = 7;

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

    // The dispatch slots (QI-BB-015): every one-shot query took a slot
    // without waiting, nothing is in flight now, and every answer's frame
    // bytes were counted. The response-byte oracle is a lower bound: five
    // answers of two candidates each cannot be fewer than five frame
    // headers plus one byte of body apiece.
    expect_eq(
        "query dispatch queue waits",
        &second.counter_delta(&first, "ipc_query_dispatch_queue_wait_total")?,
        &0,
    )?;
    expect_eq(
        "query dispatch in flight",
        &second.gauge("ipc_query_dispatch_in_flight")?,
        &0.0,
    )?;
    let response_bytes = second.counter_delta(&first, "ipc_query_response_bytes_total")?;
    if response_bytes < SERVED_QUERIES.saturating_mul(5) {
        return Err(format!("five answers carried bytes: {response_bytes}").into());
    }
    expect_eq(
        "repo-scoped overloads",
        &second.counter_delta(&first, "ipc_query_requests_overloaded_repo_total")?,
        &0,
    )?;
    // Each served query examined exactly the two fixture candidates
    // (QI-BB-015): the per-route examined counter moves by two per query.
    expect_eq(
        "lexical examined candidates",
        &second.counter_delta(&first, "lq_route_lexical_examined_candidates_total")?,
        &SERVED_QUERIES.saturating_mul(2),
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
    // A plan-limit timeout is neither a request deadline nor a peer
    // cancellation (QI-BB-002): both of those counters stay put, globally
    // and per route.
    for name in [
        "lq_typed_error_deadline_exceeded_total",
        "lq_typed_error_cancelled_total",
        "lq_route_lexical_deadline_exceeded_total",
        "lq_route_lexical_cancelled_total",
    ] {
        expect_eq(name, &third.counter_delta(&second, name)?, &0)?;
    }
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

    let expected_boot: [(&str, u64); 13] = [
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
            "boot_half_sealed_pairs",
            u64::try_from(inventory.half_sealed_pairs.len())?,
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
    // no text authority and rebuilt from its one document, the second
    // updated it in place with its one chunk; nothing was retired, and
    // both documents live in the one shard each write produced — nothing
    // could be inherited within a single generation (QI-BB-006).
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
    expect_eq(
        "text-authority shards written",
        &scrape.counter("lexical_text_authority_shards_written_total")?,
        &2,
    )?;
    expect_eq(
        "text-authority shards inherited",
        &scrape.counter("lexical_text_authority_shards_inherited_total")?,
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

    // Every socket server reports; nothing was refused or overloaded, and
    // the dispatch-slot points are present on every plane (QI-BB-015).
    for plane in ["query", "control", "ingest"] {
        for suffix in [
            "connections_refused_total",
            "requests_overloaded_total",
            "requests_overloaded_repo_total",
        ] {
            let name = format!("ipc_{plane}_{suffix}");
            expect_eq(&name, &scrape.counter(&name)?, &0)?;
        }
        let _present = scrape.counter(&format!("ipc_{plane}_dispatch_queue_wait_total"))?;
        let _present = scrape.counter(&format!("ipc_{plane}_response_bytes_total"))?;
        // The control dispatch carrying this scrape is the one in flight.
        let expected_in_flight = if plane == "control" { 1.0 } else { 0.0 };
        expect_eq(
            &format!("ipc_{plane}_dispatch_in_flight"),
            &scrape.gauge(&format!("ipc_{plane}_dispatch_in_flight"))?,
            &expected_in_flight,
        )?;
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

    // The development embedder label (QI-BB-007): the harness boots the
    // hash profile, and the scrape says so.
    expect_eq(
        "dev semantic profile",
        &scrape.gauge("boot_semantic_profile_is_dev")?,
        &1.0,
    )?;
    if !inventory.semantic_profile_is_dev {
        return Err("the boot inventory labels the hash profile as dev".into());
    }

    // The maintenance timer and process gauge (QI-BB-016): the timer
    // measured both tracks at boot, the writer gate is disabled by
    // default and says so, and the process reports a resident set.
    let _present = scrape.counter("maintenance_ticks_total")?;
    expect_eq(
        "sweep failures",
        &scrape.counter("maintenance_sweep_failures_total")?,
        &0,
    )?;
    expect_eq(
        "disk refresh failures",
        &scrape.counter("maintenance_disk_refresh_failures_total")?,
        &0,
    )?;
    let _present = scrape.gauge("search_corpus_lexical_generation_disk_bytes")?;
    let _present = scrape.gauge("search_corpus_semantic_generation_disk_bytes")?;
    expect_eq(
        "rss gate disabled by default",
        &scrape.gauge("lexical_writer_rss_gate_enabled")?,
        &0.0,
    )?;
    expect_eq(
        "rss refusals",
        &scrape.counter("lexical_writer_rss_refusals_total")?,
        &0,
    )?;
    if scrape.gauge("process_resident_bytes")? <= 0.0 {
        return Err("a running daemon has resident pages".into());
    }
    Ok(())
}

/// The embedding provider's counters reach the scrape under the `OpenAI`
/// profile (QI-BB-009 #5, QI-BB-015): the provider telemetry and the
/// cache's open report are present from boot, before any request.
#[test]
fn the_provider_and_cache_open_metrics_are_scraped_under_the_openai_profile() -> TestResult {
    let mut rt = E2eRuntime::boot_with_embedder_profile(
        quanta_index_searchd::app::SemanticEmbedderProfile::OpenAi {
            model: "text-embedding-3-small".to_string(),
            model_revision: "scrape-test".to_string(),
            dimension: 1536,
            api_key: "sk-scrape-test".to_string(),
            tuning: quanta_index_searchd::app::config::OpenAiEmbedderTuning::default(),
        },
    )?;
    rt.start()?;
    let scrape = Scrape::take(&mut rt)?;
    for name in [
        "embed_provider_http_requests_total",
        "embed_provider_retries_total",
        "embed_provider_http_failures_total",
        "embed_provider_transport_failures_total",
        "embedding_cache_hits_total",
        "embedding_cache_misses_total",
        "embedding_cache_expirations_total",
        "embedding_cache_manifest_flushes_total",
    ] {
        let _present = scrape.counter(name)?;
    }
    for name in [
        "embedding_cache_entries",
        "embedding_cache_open_stat_calls",
        "embedding_cache_retained_foreign_bytes",
    ] {
        let _present = scrape.gauge(name)?;
    }
    expect_eq(
        "a fresh cache reads no entry metadata at open",
        &scrape.gauge("embedding_cache_open_stat_calls")?,
        &0.0,
    )?;
    expect_eq(
        "the open flushed the first manifest",
        &scrape.counter("embedding_cache_manifest_flushes_total")?,
        &1,
    )?;
    expect_eq(
        "not a dev profile",
        &scrape.gauge("boot_semantic_profile_is_dev")?,
        &0.0,
    )?;
    Ok(())
}

/// The regex match cache's tallies reach the scrape and move by exactly
/// the regex traffic sent (QI-BB-024 보완 #4).
///
/// The first regex query builds one match set holding the one document it
/// matches and keeps it; its repeat is a hit that builds nothing and
/// leaves the resident bytes where they were; nothing is refused or
/// evicted.
#[test]
fn regex_match_cache_tallies_move_by_exactly_the_regex_traffic() -> TestResult {
    const REGEX: &str = "patterntype:regexp alph[a-z]";
    let mut rt = E2eRuntime::boot()?;
    seed(&mut rt)?;
    let before = Scrape::take(&mut rt)?;
    let serve = |rt: &mut E2eRuntime| -> TestResult {
        let result = rt.query_text(TextQuerySyntax::Sourcegraph, REGEX, 10);
        if let Some(error) = result.typed_error {
            return Err(format!("the regex query failed typed: {error:?}").into());
        }
        if result.candidate_ids.len() != 1 {
            return Err(format!("only alpha matches: {:?}", result.candidate_ids).into());
        }
        Ok(())
    };

    serve(&mut rt)?;
    let first = Scrape::take(&mut rt)?;
    for (name, expected) in [
        ("lexical_regex_cache_misses_total", 1),
        ("lexical_regex_cache_hits_total", 0),
        ("lexical_regex_match_sets_built_total", 1),
        ("lexical_regex_match_set_members_built_total", 1),
        ("lexical_regex_cache_evictions_total", 0),
        ("lexical_regex_cache_refused_cardinality_total", 0),
        ("lexical_regex_cache_refused_bytes_total", 0),
    ] {
        expect_eq(name, &first.counter_delta(&before, name)?, &expected)?;
    }
    if first.counter_delta(&before, "lexical_regex_match_set_bytes_built_total")? == 0 {
        return Err("a built set occupies bytes".into());
    }
    // The seed ran no regex, so the cache held nothing before.
    expect_eq(
        "entries before",
        &before.gauge("lexical_regex_cache_entries")?,
        &0.0,
    )?;
    expect_eq(
        "entries",
        &first.gauge("lexical_regex_cache_entries")?,
        &1.0,
    )?;
    let resident = first.gauge("lexical_regex_cache_resident_bytes")?;
    if resident <= before.gauge("lexical_regex_cache_resident_bytes")? {
        return Err("the kept set is resident".into());
    }

    serve(&mut rt)?;
    let repeat = Scrape::take(&mut rt)?;
    for (name, expected) in [
        ("lexical_regex_cache_misses_total", 0),
        ("lexical_regex_cache_hits_total", 1),
        ("lexical_regex_match_sets_built_total", 0),
        ("lexical_regex_match_set_members_built_total", 0),
        ("lexical_regex_match_set_bytes_built_total", 0),
    ] {
        expect_eq(name, &repeat.counter_delta(&first, name)?, &expected)?;
    }
    expect_eq(
        "resident bytes after a hit",
        &repeat.gauge("lexical_regex_cache_resident_bytes")?,
        &resident,
    )?;
    Ok(())
}
